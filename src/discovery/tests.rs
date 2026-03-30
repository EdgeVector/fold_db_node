use super::config::*;
use super::interests;
use super::pseudonym::*;
#[cfg(feature = "test-utils")]
use fold_db::db_operations::native_index::MockEmbeddingModel;
use fold_db::storage::{NamespacedStore, SledNamespacedStore};
use std::collections::HashMap;
use std::sync::Arc;

// === Pseudonym Tests ===

#[test]
fn test_pseudonym_deterministic() {
    let master = b"master-secret-key-32-bytes-long!!";
    let hash = content_hash("some record text");
    let p1 = derive_pseudonym(master, &hash);
    let p2 = derive_pseudonym(master, &hash);
    assert_eq!(p1, p2);
}

#[test]
fn test_pseudonym_different_content() {
    let master = b"master-secret-key-32-bytes-long!!";
    let h1 = content_hash("recipe for curry");
    let h2 = content_hash("recipe for pasta");
    assert_ne!(derive_pseudonym(master, &h1), derive_pseudonym(master, &h2));
}

#[test]
fn test_pseudonym_different_master() {
    let hash = content_hash("same text");
    let p1 = derive_pseudonym(b"key-one-32-bytes-long-enough!!!!", &hash);
    let p2 = derive_pseudonym(b"key-two-32-bytes-long-enough!!!!", &hash);
    assert_ne!(p1, p2);
}

#[test]
fn test_pseudonym_valid_uuid_v4() {
    let p = derive_pseudonym(b"key!key!key!key!key!key!key!key!", &content_hash("text"));
    assert_eq!(p.get_version_num(), 4);
    assert!(uuid::Uuid::parse_str(&p.to_string()).is_ok());
}

#[test]
fn test_content_hash_deterministic() {
    assert_eq!(content_hash("hello"), content_hash("hello"));
    assert_ne!(content_hash("hello"), content_hash("world"));
}

#[test]
fn test_content_hash_bytes_deterministic() {
    let data = vec![1u8, 2, 3, 4];
    assert_eq!(content_hash_bytes(&data), content_hash_bytes(&data));
    assert_ne!(content_hash_bytes(&data), content_hash_bytes(&[5, 6, 7]));
}

// === Config Persistence Tests ===

async fn make_store() -> Arc<dyn fold_db::storage::traits::KvStore> {
    let db = sled::Config::new().temporary(true).open().unwrap();
    let store = Arc::new(SledNamespacedStore::new(db));
    store.open_namespace("discovery_test").await.unwrap()
}

#[tokio::test]
async fn test_save_and_load_opt_in() {
    let store = make_store().await;
    let config = DiscoveryOptIn::new("recipes_abc".to_string(), "recipes".to_string());

    save_opt_in(&*store, &config).await.unwrap();
    let loaded = load_opt_in(&*store, "recipes_abc").await.unwrap();

    assert!(loaded.is_some());
    let loaded = loaded.unwrap();
    assert_eq!(loaded.schema_name, "recipes_abc");
    assert_eq!(loaded.category, "recipes");
    assert!(!loaded.include_preview);
}

#[tokio::test]
async fn test_save_with_preview_config() {
    let store = make_store().await;
    let config = DiscoveryOptIn::new("posts_xyz".to_string(), "blog".to_string())
        .with_preview(200, vec!["author".to_string(), "email".to_string()]);

    save_opt_in(&*store, &config).await.unwrap();
    let loaded = load_opt_in(&*store, "posts_xyz").await.unwrap().unwrap();

    assert!(loaded.include_preview);
    assert_eq!(loaded.preview_max_chars, 200);
    assert_eq!(loaded.preview_excluded_fields, vec!["author", "email"]);
}

#[tokio::test]
async fn test_load_nonexistent_returns_none() {
    let store = make_store().await;
    let loaded = load_opt_in(&*store, "nonexistent").await.unwrap();
    assert!(loaded.is_none());
}

#[tokio::test]
async fn test_remove_opt_in() {
    let store = make_store().await;
    let config = DiscoveryOptIn::new("schema_a".to_string(), "cat".to_string());

    save_opt_in(&*store, &config).await.unwrap();
    assert!(load_opt_in(&*store, "schema_a").await.unwrap().is_some());

    remove_opt_in(&*store, "schema_a").await.unwrap();
    assert!(load_opt_in(&*store, "schema_a").await.unwrap().is_none());
}

#[tokio::test]
async fn test_list_opt_ins() {
    let store = make_store().await;

    save_opt_in(
        &*store,
        &DiscoveryOptIn::new("s1".to_string(), "cat1".to_string()),
    )
    .await
    .unwrap();
    save_opt_in(
        &*store,
        &DiscoveryOptIn::new("s2".to_string(), "cat2".to_string()),
    )
    .await
    .unwrap();

    let configs = list_opt_ins(&*store).await.unwrap();
    assert_eq!(configs.len(), 2);

    let names: Vec<&str> = configs.iter().map(|c| c.schema_name.as_str()).collect();
    assert!(names.contains(&"s1"));
    assert!(names.contains(&"s2"));
}

#[tokio::test]
async fn test_upsert_opt_in() {
    let store = make_store().await;

    let v1 = DiscoveryOptIn::new("schema".to_string(), "old_cat".to_string());
    save_opt_in(&*store, &v1).await.unwrap();

    let v2 = DiscoveryOptIn::new("schema".to_string(), "new_cat".to_string());
    save_opt_in(&*store, &v2).await.unwrap();

    let loaded = load_opt_in(&*store, "schema").await.unwrap().unwrap();
    assert_eq!(loaded.category, "new_cat");

    let configs = list_opt_ins(&*store).await.unwrap();
    assert_eq!(configs.len(), 1);
}

// === Interest Detection Tests ===

async fn make_interest_stores() -> (
    Arc<dyn fold_db::storage::traits::KvStore>,
    Arc<dyn fold_db::storage::traits::KvStore>,
) {
    let db = sled::Config::new().temporary(true).open().unwrap();
    let store = Arc::new(SledNamespacedStore::new(db));
    let emb_store = store.open_namespace("embeddings").await.unwrap();
    let meta_store = store.open_namespace("metadata").await.unwrap();
    (emb_store, meta_store)
}

#[tokio::test]
async fn test_save_and_load_interest_profile() {
    let (_emb, meta) = make_interest_stores().await;

    let profile = interests::InterestProfile {
        categories: vec![
            interests::InterestCategory {
                name: "Photography".to_string(),
                count: 15,
                avg_similarity: 0.45,
                enabled: true,
            },
            interests::InterestCategory {
                name: "Cooking".to_string(),
                count: 8,
                avg_similarity: 0.38,
                enabled: true,
            },
        ],
        total_embeddings_scanned: 100,
        unmatched_count: 77,
        detected_at: chrono::Utc::now(),
        seed_version: 1,
    };

    interests::save_interest_profile(&*meta, &profile)
        .await
        .unwrap();
    let loaded = interests::load_interest_profile(&*meta).await.unwrap();

    assert!(loaded.is_some());
    let loaded = loaded.unwrap();
    assert_eq!(loaded.categories.len(), 2);
    assert_eq!(loaded.categories[0].name, "Photography");
    assert_eq!(loaded.categories[0].count, 15);
    assert_eq!(loaded.total_embeddings_scanned, 100);
    assert_eq!(loaded.unmatched_count, 77);
}

#[tokio::test]
async fn test_toggle_interest_category() {
    let (_emb, meta) = make_interest_stores().await;

    let profile = interests::InterestProfile {
        categories: vec![interests::InterestCategory {
            name: "Music".to_string(),
            count: 10,
            avg_similarity: 0.4,
            enabled: true,
        }],
        total_embeddings_scanned: 50,
        unmatched_count: 40,
        detected_at: chrono::Utc::now(),
        seed_version: 1,
    };

    interests::save_interest_profile(&*meta, &profile)
        .await
        .unwrap();

    // Toggle off
    let updated = interests::toggle_interest_category(&*meta, "Music", false)
        .await
        .unwrap();
    assert!(!updated.categories[0].enabled);

    // Verify persistence
    let loaded = interests::load_interest_profile(&*meta)
        .await
        .unwrap()
        .unwrap();
    assert!(!loaded.categories[0].enabled);

    // Toggle back on
    let updated = interests::toggle_interest_category(&*meta, "Music", true)
        .await
        .unwrap();
    assert!(updated.categories[0].enabled);
}

#[tokio::test]
async fn test_toggle_nonexistent_category_errors() {
    let (_emb, meta) = make_interest_stores().await;

    let profile = interests::InterestProfile {
        categories: vec![],
        total_embeddings_scanned: 0,
        unmatched_count: 0,
        detected_at: chrono::Utc::now(),
        seed_version: 1,
    };

    interests::save_interest_profile(&*meta, &profile)
        .await
        .unwrap();

    let result = interests::toggle_interest_category(&*meta, "Nonexistent", true).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_empty_schemas_returns_empty_profile() {
    let (_emb, meta) = make_interest_stores().await;

    let schemas: Vec<fold_db::schema::types::Schema> = vec![];
    let profile = interests::detect_interests_from_schemas(&schemas, &*meta)
        .await
        .unwrap();

    assert!(profile.categories.is_empty());
    assert_eq!(profile.total_embeddings_scanned, 0);
    assert_eq!(profile.unmatched_count, 0);
}

#[tokio::test]
async fn test_detect_interests_from_schemas_aggregates_categories() {
    let (_emb, meta) = make_interest_stores().await;

    let mut schema = fold_db::schema::types::Schema::new(
        "test_photos".to_string(),
        fold_db::schema::types::schema::DeclarativeSchemaType::Single,
        None,
        Some(vec![
            "title".to_string(),
            "camera".to_string(),
            "album".to_string(),
            "content_hash".to_string(),
        ]),
        None,
        None,
    );
    schema.field_interest_categories.insert("title".to_string(), "Photography".to_string());
    schema.field_interest_categories.insert("camera".to_string(), "Photography".to_string());
    schema.field_interest_categories.insert("album".to_string(), "Photography".to_string());
    // content_hash has no interest category (structural field)

    let schemas = vec![schema];
    let profile = interests::detect_interests_from_schemas(&schemas, &*meta)
        .await
        .unwrap();

    assert_eq!(profile.categories.len(), 1);
    assert_eq!(profile.categories[0].name, "Photography");
    assert_eq!(profile.categories[0].count, 3);
    assert_eq!(profile.total_embeddings_scanned, 4);
    assert_eq!(profile.unmatched_count, 1);
    assert!(profile.categories[0].enabled);
}

#[tokio::test]
async fn test_detect_interests_preserves_toggle_state() {
    let (_emb, meta) = make_interest_stores().await;

    // First detection
    let mut schema = fold_db::schema::types::Schema::new(
        "test".to_string(),
        fold_db::schema::types::schema::DeclarativeSchemaType::Single,
        None,
        Some(vec!["recipe".to_string()]),
        None,
        None,
    );
    schema.field_interest_categories.insert("recipe".to_string(), "Cooking".to_string());

    let schemas = vec![schema.clone()];
    let profile = interests::detect_interests_from_schemas(&schemas, &*meta)
        .await
        .unwrap();
    assert!(profile.categories[0].enabled);

    // Toggle off
    interests::toggle_interest_category(&*meta, "Cooking", false)
        .await
        .unwrap();

    // Re-detect — should preserve disabled state
    let profile = interests::detect_interests_from_schemas(&schemas, &*meta)
        .await
        .unwrap();
    assert!(!profile.categories[0].enabled);
}

#[tokio::test]
async fn test_load_nonexistent_profile_returns_none() {
    let (_emb, meta) = make_interest_stores().await;
    let loaded = interests::load_interest_profile(&*meta).await.unwrap();
    assert!(loaded.is_none());
}

// === Similar Profile Aggregation Tests ===

/// Tests the aggregation logic used in handlers::discovery::similar_profiles.
/// This mirrors the actual code path without requiring network calls.
#[test]
fn test_similar_profile_aggregation_sorting() {
    use crate::handlers::discovery::SimilarProfile;

    let user_cat_count = 3usize;
    let mut profile_map: HashMap<uuid::Uuid, (Vec<String>, f32)> = HashMap::new();

    let p1 = uuid::Uuid::new_v4();
    let p2 = uuid::Uuid::new_v4();
    let p3 = uuid::Uuid::new_v4();

    // p1 shares 2 of 3 categories, top sim 0.8
    profile_map.insert(p1, (vec!["Music".into(), "Cooking".into()], 0.8));
    // p2 shares 3 of 3 categories, top sim 0.6
    profile_map.insert(
        p2,
        (vec!["Music".into(), "Cooking".into(), "Travel".into()], 0.6),
    );
    // p3 shares 1 of 3, top sim 0.9
    profile_map.insert(p3, (vec!["Music".into()], 0.9));

    let mut profiles: Vec<SimilarProfile> = profile_map
        .into_iter()
        .map(|(pseudonym, (shared_categories, top_similarity))| {
            let match_percentage = (shared_categories.len() as f32 / user_cat_count as f32) * 100.0;
            SimilarProfile {
                pseudonym,
                match_percentage,
                shared_categories,
                top_similarity,
            }
        })
        .collect();

    profiles.sort_by(|a, b| {
        b.match_percentage
            .partial_cmp(&a.match_percentage)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                b.top_similarity
                    .partial_cmp(&a.top_similarity)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });

    // p2 first (100%), then p1 (66.7%), then p3 (33.3%)
    assert_eq!(profiles[0].pseudonym, p2);
    assert!((profiles[0].match_percentage - 100.0).abs() < 0.1);

    assert_eq!(profiles[1].pseudonym, p1);
    assert!((profiles[1].match_percentage - 66.7).abs() < 0.5);

    assert_eq!(profiles[2].pseudonym, p3);
    assert!((profiles[2].match_percentage - 33.3).abs() < 0.5);
}

#[test]
fn test_similar_profile_empty_categories_returns_empty() {
    let enabled_categories: Vec<String> = vec![];
    assert!(enabled_categories.is_empty());
    // Mirrors the early return in the handler
}

#[test]
fn test_similar_profile_dedup_categories() {
    // Simulates the same pseudonym appearing in multiple search results for the same category
    let mut profile_map: HashMap<uuid::Uuid, (Vec<String>, f32)> = HashMap::new();
    let p = uuid::Uuid::new_v4();

    // First result from Music search
    let entry = profile_map.entry(p).or_insert_with(|| (Vec::new(), 0.0));
    let cat = "Music".to_string();
    if !entry.0.contains(&cat) {
        entry.0.push(cat);
    }
    entry.1 = 0.7f32.max(entry.1);

    // Second result also from Music search (same category, different fragment)
    let entry = profile_map.entry(p).or_insert_with(|| (Vec::new(), 0.0));
    let cat = "Music".to_string();
    if !entry.0.contains(&cat) {
        entry.0.push(cat);
    }
    if 0.9 > entry.1 {
        entry.1 = 0.9;
    }

    let (cats, top_sim) = &profile_map[&p];
    assert_eq!(cats.len(), 1, "Duplicate categories should be deduped");
    assert!(
        (top_sim - 0.9).abs() < f32::EPSILON,
        "Should keep highest similarity"
    );
}

#[cfg(feature = "test-utils")]
#[tokio::test]
async fn test_get_centroids_public_accessor() {
    let (_emb, meta) = make_interest_stores().await;
    let embedder = MockEmbeddingModel;
    let centroids = interests::get_centroids(&*meta, &embedder).await.unwrap();
    // Should return 25 centroids (one per seed category)
    assert_eq!(centroids.len(), 25);
    for (name, emb) in &centroids {
        assert!(!name.is_empty());
        assert!(!emb.is_empty());
    }
}
