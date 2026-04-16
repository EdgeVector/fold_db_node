use fold_db::db_operations::native_index::Embedder;
use fold_db::llm_registry::prompts::classification::INTEREST_CATEGORIES;
use fold_db::storage::traits::KvStore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

const PROFILE_KEY: &str = "discovery:interests:profile";
const CENTROID_KEY_PREFIX: &str = "discovery:interests:centroids:v";

/// Centroid cache version — bump when INTEREST_CATEGORIES changes to invalidate cache.
const CENTROID_VERSION: u32 = 2;

/// A detected interest category with match statistics.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InterestCategory {
    pub name: String,
    pub count: usize,
    /// Average similarity is no longer computed from embeddings.
    /// Retained for API compatibility; set to 1.0 for schema-sourced categories.
    pub avg_similarity: f32,
    pub enabled: bool,
}

/// The full interest profile detected from a user's data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterestProfile {
    pub categories: Vec<InterestCategory>,
    pub total_embeddings_scanned: usize,
    pub unmatched_count: usize,
    pub detected_at: chrono::DateTime<chrono::Utc>,
    pub seed_version: u32,
}

/// Stored centroid entry for cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedCentroids {
    centroids: Vec<(String, Vec<f32>)>,
}

/// Public access to cached category centroids for use by similar-profiles matching.
/// Centroids are computed from the canonical INTEREST_CATEGORIES vocabulary.
pub async fn get_centroids(
    metadata_store: &dyn KvStore,
    embedder: &dyn Embedder,
) -> Result<Vec<(String, Vec<f32>)>, String> {
    get_or_compute_centroids(metadata_store, embedder).await
}

/// Compute or load cached category centroids.
///
/// Each centroid is the embedding of the category name itself.
/// Used by similar_profiles to search the discovery network per category.
async fn get_or_compute_centroids(
    metadata_store: &dyn KvStore,
    embedder: &dyn Embedder,
) -> Result<Vec<(String, Vec<f32>)>, String> {
    let cache_key = format!("{}{}", CENTROID_KEY_PREFIX, CENTROID_VERSION);

    // Try loading cached centroids
    if let Ok(Some(bytes)) = metadata_store.get(cache_key.as_bytes()).await {
        if let Ok(cached) = serde_json::from_slice::<CachedCentroids>(&bytes) {
            if !cached.centroids.is_empty() {
                return Ok(cached.centroids);
            }
        }
    }

    // Compute centroids from category names
    let mut centroids = Vec::with_capacity(INTEREST_CATEGORIES.len());

    for name in INTEREST_CATEGORIES {
        let emb = embedder
            .embed_text(name)
            .map_err(|e| format!("Failed to embed category '{}': {}", name, e))?;
        centroids.push((name.to_string(), emb));
    }

    // Cache for future runs
    let cached = CachedCentroids {
        centroids: centroids.clone(),
    };
    if let Ok(bytes) = serde_json::to_vec(&cached) {
        if let Err(e) = metadata_store.put(cache_key.as_bytes(), bytes).await {
            log::warn!("Failed to cache interest centroids: {}", e);
        }
    }

    Ok(centroids)
}

/// Detect interests from approved schemas' `field_interest_categories`.
///
/// Instead of scanning embeddings and classifying against seed centroids,
/// this reads the schema-service-assigned interest categories from each
/// approved schema and aggregates field counts per category.
pub async fn detect_interests_from_schemas(
    schemas: &[fold_db::schema::types::Schema],
    metadata_store: &dyn KvStore,
) -> Result<InterestProfile, String> {
    let mut category_counts: HashMap<String, usize> = HashMap::new();
    let mut total_fields = 0usize;
    let mut unmatched = 0usize;

    for schema in schemas {
        // Skip superseded schemas
        if schema.superseded_by.is_some() {
            continue;
        }

        for field_name in schema.fields.as_deref().unwrap_or(&[]) {
            total_fields += 1;
            if let Some(category) = schema.field_interest_categories.get(field_name) {
                *category_counts.entry(category.clone()).or_insert(0) += 1;
            } else {
                unmatched += 1;
            }
        }
    }

    // Preserve existing enabled/disabled state from previous profile
    let existing_profile = load_interest_profile(metadata_store).await?;
    let existing_enabled: HashMap<String, bool> = existing_profile
        .map(|p| {
            p.categories
                .into_iter()
                .map(|c| (c.name, c.enabled))
                .collect()
        })
        .unwrap_or_default();

    // Build categories from field counts (no minimum threshold — schema service already filtered)
    let mut categories: Vec<InterestCategory> = category_counts
        .into_iter()
        .map(|(name, count)| {
            let enabled = existing_enabled.get(&name).copied().unwrap_or(true);
            InterestCategory {
                name,
                count,
                avg_similarity: 1.0,
                enabled,
            }
        })
        .collect();

    // Sort by count descending
    categories.sort_by_key(|b| std::cmp::Reverse(b.count));

    let profile = InterestProfile {
        categories,
        total_embeddings_scanned: total_fields,
        unmatched_count: unmatched,
        detected_at: chrono::Utc::now(),
        seed_version: CENTROID_VERSION,
    };

    save_interest_profile(metadata_store, &profile).await?;

    Ok(profile)
}

/// Save the interest profile to the metadata store.
pub async fn save_interest_profile(
    store: &dyn KvStore,
    profile: &InterestProfile,
) -> Result<(), String> {
    let bytes = serde_json::to_vec(profile)
        .map_err(|e| format!("Failed to serialize interest profile: {}", e))?;
    store
        .put(PROFILE_KEY.as_bytes(), bytes)
        .await
        .map_err(|e| format!("Failed to save interest profile: {}", e))
}

/// Load the interest profile from the metadata store.
pub async fn load_interest_profile(store: &dyn KvStore) -> Result<Option<InterestProfile>, String> {
    let bytes = store
        .get(PROFILE_KEY.as_bytes())
        .await
        .map_err(|e| format!("Failed to load interest profile: {}", e))?;
    match bytes {
        Some(b) => {
            let profile: InterestProfile = serde_json::from_slice(&b)
                .map_err(|e| format!("Failed to deserialize interest profile: {}", e))?;
            Ok(Some(profile))
        }
        None => Ok(None),
    }
}

/// Toggle a category's enabled flag and persist.
pub async fn toggle_interest_category(
    store: &dyn KvStore,
    category_name: &str,
    enabled: bool,
) -> Result<InterestProfile, String> {
    let mut profile = load_interest_profile(store)
        .await?
        .ok_or_else(|| "No interest profile found".to_string())?;

    let cat = profile
        .categories
        .iter_mut()
        .find(|c| c.name == category_name)
        .ok_or_else(|| format!("Category '{}' not found", category_name))?;

    cat.enabled = enabled;
    save_interest_profile(store, &profile).await?;
    Ok(profile)
}

/// Top-level entry point called from the batch completion hook.
/// Reads schemas from FoldNode and detects interests from their
/// field_interest_categories.
pub async fn run_interest_detection(
    node: &Arc<crate::fold_node::FoldNode>,
) -> Result<InterestProfile, String> {
    let db = node
        .get_fold_db()
        .map_err(|e| format!("Failed to access database: {}", e))?;

    let db_ops = db.get_db_ops();
    let metadata_store = db_ops.raw_metadata_store();

    // Get all schemas
    let schemas: Vec<_> = db
        .schema_manager()
        .get_schemas()
        .map_err(|e| format!("Failed to get schemas: {}", e))?
        .into_values()
        .collect();

    // Drop the DB lock before doing the work
    drop(db);

    detect_interests_from_schemas(&schemas, &*metadata_store).await
}
