use fold_db::db_operations::native_index::cosine_similarity;
use fold_db::db_operations::native_index::Embedder;
use fold_db::storage::traits::KvStore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

const PROFILE_KEY: &str = "discovery:interests:profile";
const CENTROID_KEY_PREFIX: &str = "discovery:interests:centroids:v";
const SEED_VERSION: u32 = 1;
const MIN_CATEGORY_COUNT: usize = 3;
const SIMILARITY_THRESHOLD: f32 = 0.25;
const EMB_PREFIX: &str = "emb:";

/// A detected interest category with match statistics.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InterestCategory {
    pub name: String,
    pub count: usize,
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

/// Deserialized embedding from the KvStore (mirrors fold_db's StoredEmbedding).
#[derive(Deserialize)]
struct StoredEmbedding {
    #[serde(default)]
    pub embedding: Vec<f32>,
}

// ~25 seed categories with descriptive phrases for embedding-based classification.
const SEED_CATEGORIES: &[(&str, &[&str])] = &[
    (
        "Photography",
        &[
            "camera settings aperture shutter speed ISO",
            "photo composition lighting portrait landscape",
            "editing photos retouching lightroom",
        ],
    ),
    (
        "Cooking",
        &[
            "recipe ingredients cooking instructions kitchen",
            "baking bread pastry dessert cake",
            "meal prep dinner planning seasoning spices",
        ],
    ),
    (
        "Running",
        &[
            "marathon training pace miles distance",
            "running shoes jogging trail 5K 10K",
            "race time splits cadence stride",
        ],
    ),
    (
        "Software Engineering",
        &[
            "programming code function variable algorithm",
            "software architecture API design patterns",
            "debugging testing deployment git pull request",
        ],
    ),
    (
        "Music",
        &[
            "guitar piano drums instrument playing",
            "song lyrics melody chord progression",
            "concert band album recording studio",
        ],
    ),
    (
        "Travel",
        &[
            "flight hotel vacation itinerary trip",
            "passport visa international travel abroad",
            "sightseeing tourist destination explore city",
        ],
    ),
    (
        "Fitness",
        &[
            "workout exercise gym strength training",
            "yoga stretching flexibility mobility",
            "weightlifting bench press squat deadlift",
        ],
    ),
    (
        "Reading",
        &[
            "book novel author fiction nonfiction",
            "reading list recommendation review chapter",
            "literature story narrative plot character",
        ],
    ),
    (
        "Gaming",
        &[
            "video game console controller gameplay",
            "strategy RPG multiplayer online match",
            "game level quest achievement score",
        ],
    ),
    (
        "Finance",
        &[
            "investment stock portfolio dividend market",
            "budget savings retirement planning funds",
            "crypto bitcoin blockchain trading exchange",
        ],
    ),
    (
        "Gardening",
        &[
            "plants seeds soil compost mulch",
            "flowers garden vegetables herbs growing",
            "pruning watering fertilizer harvest season",
        ],
    ),
    (
        "Art & Design",
        &[
            "painting drawing sketch illustration canvas",
            "graphic design typography color palette",
            "sculpture ceramics creative artwork gallery",
        ],
    ),
    (
        "Parenting",
        &[
            "children kids family parenting childcare",
            "baby toddler school education milestone",
            "activities playtime bedtime routine",
        ],
    ),
    (
        "Health & Wellness",
        &[
            "meditation mindfulness mental health therapy",
            "nutrition diet vitamins supplements",
            "sleep hygiene stress management self care",
        ],
    ),
    (
        "Sports",
        &[
            "basketball football soccer baseball hockey",
            "team score game championship league",
            "athlete competition season playoffs tournament",
        ],
    ),
    (
        "Movies & TV",
        &[
            "film movie director actor scene cinema",
            "TV series episode streaming show season",
            "documentary thriller comedy drama genre",
        ],
    ),
    (
        "Science",
        &[
            "experiment hypothesis research data analysis",
            "physics chemistry biology evolution",
            "scientific journal discovery laboratory",
        ],
    ),
    (
        "Writing",
        &[
            "essay article blog post draft editing",
            "creative writing story narrative prose",
            "publishing manuscript journal memoir",
        ],
    ),
    (
        "Fashion",
        &[
            "clothing outfit style wardrobe trend",
            "fashion designer brand collection runway",
            "accessories shoes jewelry handbag",
        ],
    ),
    (
        "Home Improvement",
        &[
            "renovation remodel paint flooring tile",
            "DIY tools hardware plumbing electrical",
            "furniture decor interior design layout",
        ],
    ),
    (
        "Pets",
        &[
            "dog cat pet veterinarian animal",
            "pet food training grooming health",
            "puppy kitten adoption shelter care",
        ],
    ),
    (
        "Automotive",
        &[
            "car engine maintenance oil change repair",
            "driving road trip vehicle mileage",
            "electric vehicle hybrid fuel efficiency",
        ],
    ),
    (
        "Productivity",
        &[
            "task management calendar scheduling planning",
            "goal setting time management priorities",
            "workflow automation efficiency habits",
        ],
    ),
    (
        "Social Media",
        &[
            "post followers engagement likes comments",
            "content creator influencer platform brand",
            "tweet instagram tiktok share viral",
        ],
    ),
    (
        "Education",
        &[
            "course lecture homework assignment exam",
            "university college degree major class",
            "learning tutorial certification study",
        ],
    ),
];

/// Compute or load cached category centroids.
///
/// Each centroid is the average of the embedded seed phrases for that category.
async fn get_or_compute_centroids(
    metadata_store: &dyn KvStore,
    embedder: &dyn Embedder,
) -> Result<Vec<(String, Vec<f32>)>, String> {
    let cache_key = format!("{}{}", CENTROID_KEY_PREFIX, SEED_VERSION);

    // Try loading cached centroids
    if let Ok(Some(bytes)) = metadata_store.get(cache_key.as_bytes()).await {
        if let Ok(cached) = serde_json::from_slice::<CachedCentroids>(&bytes) {
            if !cached.centroids.is_empty() {
                return Ok(cached.centroids);
            }
        }
    }

    // Compute centroids from seed phrases
    let mut centroids = Vec::with_capacity(SEED_CATEGORIES.len());

    for (name, phrases) in SEED_CATEGORIES {
        let mut phrase_embeddings = Vec::with_capacity(phrases.len());
        for phrase in *phrases {
            let emb = embedder
                .embed_text(phrase)
                .map_err(|e| format!("Failed to embed seed phrase: {}", e))?;
            phrase_embeddings.push(emb);
        }

        let centroid = average_vectors(&phrase_embeddings);
        centroids.push((name.to_string(), centroid));
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

/// Average a set of vectors element-wise.
fn average_vectors(vecs: &[Vec<f32>]) -> Vec<f32> {
    if vecs.is_empty() {
        return Vec::new();
    }
    let dim = vecs[0].len();
    let n = vecs.len() as f32;
    let mut avg = vec![0.0f32; dim];
    for v in vecs {
        for (i, val) in v.iter().enumerate() {
            avg[i] += val;
        }
    }
    for val in &mut avg {
        *val /= n;
    }
    avg
}

/// Core detection: classify all embeddings in the store against seed centroids.
pub async fn detect_interests(
    embedding_store: &dyn KvStore,
    metadata_store: &dyn KvStore,
    embedder: &dyn Embedder,
) -> Result<InterestProfile, String> {
    let centroids = get_or_compute_centroids(metadata_store, embedder).await?;

    // Scan all embeddings
    let raw_entries = embedding_store
        .scan_prefix(EMB_PREFIX.as_bytes())
        .await
        .map_err(|e| format!("Failed to scan embeddings: {}", e))?;

    let mut category_stats: HashMap<String, (usize, f32)> = HashMap::new();
    let mut total_scanned = 0usize;
    let mut unmatched = 0usize;

    for (_key, value) in &raw_entries {
        let stored: StoredEmbedding = match serde_json::from_slice(value) {
            Ok(s) => s,
            Err(_) => continue,
        };

        if stored.embedding.is_empty() {
            continue;
        }

        total_scanned += 1;

        // Find best matching centroid
        let mut best_name: Option<&str> = None;
        let mut best_sim = SIMILARITY_THRESHOLD;

        for (name, centroid) in &centroids {
            let sim = cosine_similarity(&stored.embedding, centroid);
            if sim > best_sim {
                best_sim = sim;
                best_name = Some(name);
            }
        }

        match best_name {
            Some(name) => {
                let entry = category_stats
                    .entry(name.to_string())
                    .or_insert((0, 0.0));
                entry.0 += 1;
                entry.1 += best_sim;
            }
            None => {
                unmatched += 1;
            }
        }
    }

    // Build categories, filtering by minimum count
    let mut categories: Vec<InterestCategory> = category_stats
        .into_iter()
        .filter(|(_, (count, _))| *count >= MIN_CATEGORY_COUNT)
        .map(|(name, (count, total_sim))| InterestCategory {
            name,
            count,
            avg_similarity: total_sim / count as f32,
            enabled: true,
        })
        .collect();

    // Sort by count descending
    categories.sort_by(|a, b| b.count.cmp(&a.count));

    let profile = InterestProfile {
        categories,
        total_embeddings_scanned: total_scanned,
        unmatched_count: unmatched,
        detected_at: chrono::Utc::now(),
        seed_version: SEED_VERSION,
    };

    // Save the profile
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
pub async fn load_interest_profile(
    store: &dyn KvStore,
) -> Result<Option<InterestProfile>, String> {
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
/// Acquires FoldNode locks, runs detection, saves result.
pub async fn run_interest_detection(
    node: &Arc<tokio::sync::RwLock<crate::fold_node::FoldNode>>,
) -> Result<InterestProfile, String> {
    let node_guard = node.read().await;
    let db = node_guard
        .get_fold_db()
        .await
        .map_err(|e| format!("Failed to access database: {}", e))?;

    let db_ops = db.get_db_ops();
    let metadata_store = db_ops.metadata_store().inner().clone();

    let native_index_mgr = db_ops
        .native_index_manager()
        .ok_or_else(|| "Native index not available".to_string())?;

    let embedding_store = native_index_mgr.store().clone();
    let embedder = native_index_mgr.embedder().clone();

    // Drop the DB lock before doing the heavy work
    drop(db);
    drop(node_guard);

    detect_interests(&*embedding_store, &*metadata_store, &*embedder).await
}
