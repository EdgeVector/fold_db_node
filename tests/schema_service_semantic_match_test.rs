#![cfg(feature = "test-utils")]

use fold_db::db_operations::native_index::{MockEmbeddingModel, ScriptedEmbeddingModel};
use fold_db::schema::types::data_classification::DataClassification;
use fold_db_node::schema_service::server::{SchemaAddOutcome, SchemaServiceState};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::tempdir;

/// Helper function to convert JSON to Schema
fn json_to_schema(value: serde_json::Value) -> fold_db::schema::types::Schema {
    let mut schema: fold_db::schema::types::Schema =
        serde_json::from_value(value).expect("failed to deserialize schema from JSON");
    if schema.descriptive_name.is_none() {
        schema.descriptive_name = Some(schema.name.clone());
    }
    if let Some(ref fields) = schema.fields {
        for f in fields {
            schema
                .field_descriptions
                .entry(f.clone())
                .or_insert_with(|| format!("{} field", f));
            schema
                .field_data_classifications
                .entry(f.clone())
                .or_insert_with(|| DataClassification::new(0, "general").unwrap());
        }
    }
    schema
}

fn create_test_state() -> SchemaServiceState {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_schema_db")
        .to_string_lossy()
        .to_string();

    // Leak the tempdir so it doesn't get cleaned up while the state is in use
    std::mem::forget(temp_dir);

    SchemaServiceState::new_with_embedder(db_path, Arc::new(MockEmbeddingModel))
        .expect("failed to initialize schema service state")
}

#[tokio::test]
async fn exact_descriptive_name_match_still_triggers_expansion() {
    let state = create_test_state();

    // Add a schema with descriptive_name "Twitter Posts"
    let schema1 = json_to_schema(json!({
        "name": "Schema1",
        "descriptive_name": "Twitter Posts",
        "fields": ["tweet_id", "content"]
    }));

    let outcome = state
        .add_schema(schema1, HashMap::new())
        .await
        .expect("failed to add first schema");
    assert!(matches!(outcome, SchemaAddOutcome::Added(_, _)));

    // Add a second schema with the SAME descriptive_name but MORE fields → expansion
    let schema2 = json_to_schema(json!({
        "name": "Schema2",
        "descriptive_name": "Twitter Posts",
        "fields": ["tweet_id", "content", "likes_count"]
    }));

    let outcome = state
        .add_schema(schema2, HashMap::new())
        .await
        .expect("failed to add second schema");

    match outcome {
        SchemaAddOutcome::Expanded(old_name, expanded_schema, _) => {
            assert!(!old_name.is_empty());
            // The expanded schema should have all three fields
            let fields = expanded_schema
                .fields
                .as_ref()
                .expect("expanded schema must have fields");
            assert!(fields.contains(&"tweet_id".to_string()));
            assert!(fields.contains(&"content".to_string()));
            assert!(fields.contains(&"likes_count".to_string()));
        }
        other => panic!("expected Expanded outcome, got {:?}", other),
    }
}

#[tokio::test]
async fn exact_descriptive_name_subset_returns_already_exists() {
    let state = create_test_state();

    // Add a schema with descriptive_name
    let schema1 = json_to_schema(json!({
        "name": "Schema1",
        "descriptive_name": "User Contacts",
        "fields": ["name", "email", "phone"]
    }));

    state
        .add_schema(schema1, HashMap::new())
        .await
        .expect("failed to add first schema");

    // Add a schema with the same descriptive_name but FEWER fields → subset → AlreadyExists
    let schema2 = json_to_schema(json!({
        "name": "Schema2",
        "descriptive_name": "User Contacts",
        "fields": ["name", "email"]
    }));

    let outcome = state
        .add_schema(schema2, HashMap::new())
        .await
        .expect("failed to add second schema");

    assert!(
        matches!(outcome, SchemaAddOutcome::AlreadyExists(..)),
        "subset of same descriptive_name should return AlreadyExists, got {:?}",
        outcome
    );
}

#[tokio::test]
async fn semantic_match_similar_descriptive_name_triggers_expansion() {
    let state = create_test_state();

    // Add a schema with descriptive_name "Twitter Posts"
    let schema1 = json_to_schema(json!({
        "name": "Schema1",
        "descriptive_name": "Twitter Posts",
        "fields": ["tweet_id", "content"]
    }));

    state
        .add_schema(schema1, HashMap::new())
        .await
        .expect("failed to add first schema");

    // Add a schema with a semantically similar descriptive_name (only case difference).
    // The MockEmbeddingModel uses byte values, so "Twitter Posts" vs "twitter posts" will
    // differ only in case bytes (32 difference per char), yielding high cosine similarity.
    let schema2 = json_to_schema(json!({
        "name": "Schema2",
        "descriptive_name": "twitter posts",
        "fields": ["tweet_id", "content", "author"]
    }));

    let outcome = state
        .add_schema(schema2, HashMap::new())
        .await
        .expect("failed to add second schema");

    match outcome {
        SchemaAddOutcome::Expanded(old_name, expanded_schema, _) => {
            assert!(!old_name.is_empty());
            let fields = expanded_schema
                .fields
                .as_ref()
                .expect("expanded schema must have fields");
            assert!(fields.contains(&"tweet_id".to_string()));
            assert!(fields.contains(&"content".to_string()));
            assert!(fields.contains(&"author".to_string()));
            // The expanded schema should have adopted the canonical descriptive_name
            assert_eq!(
                expanded_schema.descriptive_name.as_deref(),
                Some("Twitter Posts"),
                "expanded schema should adopt the canonical (existing) descriptive_name"
            );
        }
        other => panic!("expected Expanded from semantic match, got {:?}", other),
    }
}

#[tokio::test]
async fn semantic_match_adopts_canonical_descriptive_name() {
    let state = create_test_state();

    // Add base schema
    let schema1 = json_to_schema(json!({
        "name": "Schema1",
        "descriptive_name": "User Profile Data",
        "fields": ["user_id", "name"]
    }));

    state
        .add_schema(schema1, HashMap::new())
        .await
        .expect("failed to add first schema");

    // Add with similar (case-different) descriptive name and extra fields
    let schema2 = json_to_schema(json!({
        "name": "Schema2",
        "descriptive_name": "user profile data",
        "fields": ["user_id", "name", "email"]
    }));

    let outcome = state
        .add_schema(schema2, HashMap::new())
        .await
        .expect("failed to add second schema");

    match outcome {
        SchemaAddOutcome::Expanded(_, expanded_schema, _) => {
            assert_eq!(
                expanded_schema.descriptive_name.as_deref(),
                Some("User Profile Data"),
                "should adopt original canonical descriptive_name, not the incoming one"
            );
        }
        other => panic!("expected Expanded, got {:?}", other),
    }
}

#[tokio::test]
async fn dissimilar_descriptive_names_are_independent_schemas() {
    let state = create_test_state();

    // Use very short, byte-dissimilar descriptive names to ensure the mock embedder
    // produces low cosine similarity. The MockEmbeddingModel hashes bytes into a 384-dim
    // vector, so strings with overlapping byte values can have high similarity.
    let schema1 = json_to_schema(json!({
        "name": "Schema1",
        "descriptive_name": "AAAA",
        "fields": ["tweet_id", "content"]
    }));

    state
        .add_schema(schema1, HashMap::new())
        .await
        .expect("failed to add first schema");

    // Completely different descriptive_name — should NOT trigger expansion
    let schema2 = json_to_schema(json!({
        "name": "Schema2",
        "descriptive_name": "ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ",
        "fields": ["transaction_id", "amount"]
    }));

    let outcome = state
        .add_schema(schema2, HashMap::new())
        .await
        .expect("failed to add second schema");

    assert!(
        matches!(outcome, SchemaAddOutcome::Added(_, _)),
        "dissimilar descriptive names should create independent schemas, got {:?}",
        outcome
    );
}

#[tokio::test]
async fn no_descriptive_name_skips_semantic_matching() {
    let state = create_test_state();

    let schema1 = json_to_schema(json!({
        "name": "Schema1",
        "descriptive_name": "Twitter Posts",
        "fields": ["tweet_id", "content"]
    }));

    state
        .add_schema(schema1, HashMap::new())
        .await
        .expect("failed to add first schema");

    // Schema without descriptive_name and non-overlapping fields
    // should be added independently (no descriptive_name match, no field overlap)
    let schema2 = json_to_schema(json!({
        "name": "Schema2",
        "fields": ["user_id", "bio", "avatar"]
    }));

    let outcome = state
        .add_schema(schema2, HashMap::new())
        .await
        .expect("failed to add second schema");

    assert!(
        matches!(outcome, SchemaAddOutcome::Added(_, _)),
        "schema without descriptive_name should be added independently, got {:?}",
        outcome
    );
}

#[tokio::test]
async fn semantic_match_with_subset_fields_returns_already_exists() {
    let state = create_test_state();

    let schema1 = json_to_schema(json!({
        "name": "Schema1",
        "descriptive_name": "Employee Records",
        "fields": ["emp_id", "name", "department", "salary"]
    }));

    state
        .add_schema(schema1, HashMap::new())
        .await
        .expect("failed to add first schema");

    // Semantically similar descriptive_name but FEWER fields (subset)
    let schema2 = json_to_schema(json!({
        "name": "Schema2",
        "descriptive_name": "employee records",
        "fields": ["emp_id", "name"]
    }));

    let outcome = state
        .add_schema(schema2, HashMap::new())
        .await
        .expect("failed to add second schema");

    assert!(
        matches!(outcome, SchemaAddOutcome::AlreadyExists(..)),
        "semantic match with subset fields should return AlreadyExists, got {:?}",
        outcome
    );
}

#[tokio::test]
async fn high_field_overlap_with_similar_name_creates_superset() {
    let state = create_test_state();

    // Existing schema with 5 fields
    let schema1 = json_to_schema(json!({
        "name": "Schema1",
        "descriptive_name": "Customer Orders",
        "fields": ["order_id", "customer_name", "product", "quantity", "price"]
    }));

    state
        .add_schema(schema1, HashMap::new())
        .await
        .expect("failed to add first schema");

    // New schema with same descriptive_name, 4/5 shared fields (80%) + 1 new field
    let schema2 = json_to_schema(json!({
        "name": "Schema2",
        "descriptive_name": "Customer Orders",
        "fields": ["order_id", "customer_name", "product", "quantity", "shipping_address"]
    }));

    let outcome = state
        .add_schema(schema2, HashMap::new())
        .await
        .expect("failed to add second schema");

    match outcome {
        SchemaAddOutcome::Expanded(_, expanded_schema, _) => {
            let fields = expanded_schema.fields.as_ref().expect("must have fields");
            // Superset: all fields from both schemas
            assert!(fields.contains(&"order_id".to_string()));
            assert!(fields.contains(&"customer_name".to_string()));
            assert!(fields.contains(&"product".to_string()));
            assert!(fields.contains(&"quantity".to_string()));
            assert!(
                fields.contains(&"price".to_string()),
                "must keep existing-only field"
            );
            assert!(
                fields.contains(&"shipping_address".to_string()),
                "must include new field"
            );
            assert_eq!(fields.len(), 6, "superset should have all 6 unique fields");
        }
        other => panic!(
            "80% field overlap + same descriptive name should expand, got {:?}",
            other
        ),
    }
}

/// Test that semantic field matching with real embeddings correctly identifies
/// "creator" as a synonym for "artist" while keeping "medium" as a distinct field.
/// Requires the FastEmbedModel (downloads on first run), so marked #[ignore].
#[tokio::test]
#[ignore]
async fn semantic_field_rename_real_embeddings() {
    use fold_db::db_operations::native_index::FastEmbedModel;

    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_schema_db")
        .to_string_lossy()
        .to_string();
    std::mem::forget(temp_dir);

    let state = SchemaServiceState::new_with_embedder(db_path, Arc::new(FastEmbedModel::new()))
        .expect("failed to init state");

    // Schema A: artwork with "artist"
    let schema_a = json_to_schema(json!({
        "name": "SchemaA",
        "descriptive_name": "Artwork Collection",
        "fields": ["artist", "title", "year"]
    }));

    state
        .add_schema(schema_a, HashMap::new())
        .await
        .expect("failed to add schema A");

    // Schema B: same concept, uses "creator" instead of "artist", adds "medium"
    let schema_b = json_to_schema(json!({
        "name": "SchemaB",
        "descriptive_name": "Artwork Collection",
        "fields": ["creator", "title", "year", "medium"]
    }));

    let outcome = state
        .add_schema(schema_b, HashMap::new())
        .await
        .expect("failed to add schema B");

    match outcome {
        SchemaAddOutcome::Expanded(_, schema, returned_mappers) => {
            let fields = schema.fields.as_ref().expect("must have fields");

            // "creator" should have been renamed to "artist" (semantic match)
            assert!(
                fields.contains(&"artist".to_string()),
                "should have 'artist' (canonical name)"
            );
            assert!(
                !fields.contains(&"creator".to_string()),
                "'creator' should have been renamed to 'artist'"
            );

            // "medium" must be preserved as a new field (NOT renamed to anything)
            assert!(
                fields.contains(&"medium".to_string()),
                "'medium' must be kept as a distinct new field, not falsely matched"
            );

            // All original fields present
            assert!(fields.contains(&"title".to_string()));
            assert!(fields.contains(&"year".to_string()));

            // Should have 4 fields total: artist, title, year, medium
            assert_eq!(fields.len(), 4, "superset should have exactly 4 fields");

            // mutation_mappers should map creator→artist
            assert_eq!(
                returned_mappers.get("creator").map(|s| s.as_str()),
                Some("artist"),
                "mutation_mappers should map 'creator' to 'artist'"
            );
        }
        other => panic!("expected Expanded, got {:?}", other),
    }
}

#[tokio::test]
async fn low_field_overlap_with_same_name_still_expands() {
    let state = create_test_state();

    let schema1 = json_to_schema(json!({
        "name": "Schema1",
        "descriptive_name": "Customer Data",
        "fields": ["customer_id", "name", "email", "phone", "address"]
    }));

    state
        .add_schema(schema1, HashMap::new())
        .await
        .expect("failed to add first schema");

    // Same descriptive_name, only 1/5 fields shared → still expands to superset
    let schema2 = json_to_schema(json!({
        "name": "Schema2",
        "descriptive_name": "Customer Data",
        "fields": ["customer_id", "revenue", "segment", "lifetime_value", "churn_risk"]
    }));

    let outcome = state
        .add_schema(schema2, HashMap::new())
        .await
        .expect("failed to add second schema");

    match outcome {
        SchemaAddOutcome::Expanded(_, schema, _) => {
            let fields = schema.fields.as_ref().unwrap();
            assert_eq!(fields.len(), 9, "superset should have all 9 unique fields");
        }
        other => panic!(
            "same descriptive_name should always expand, got {:?}",
            other
        ),
    }
}

// ===========================================================================
// Scripted embedding tests — deterministic control over similarity values.
// These test the bidirectional matching and threshold logic that prevents
// false positives like "medium" matching "artist" in artwork schemas.
// ===========================================================================

fn create_scripted_state(responses: HashMap<String, Vec<f32>>) -> SchemaServiceState {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_schema_db")
        .to_string_lossy()
        .to_string();
    std::mem::forget(temp_dir);

    let embedder = ScriptedEmbeddingModel::new(responses);
    SchemaServiceState::new_with_embedder(db_path, Arc::new(embedder))
        .expect("failed to initialize schema service state")
}

/// Reproduces the exact failure: "creator" should match "artist", but "medium"
/// should NOT be matched to anything. Uses scripted embeddings to deterministically
/// control similarity values matching the real AllMiniLML6V2 behavior.
///
/// Real model values (for reference):
///   artist ↔ creator = 0.93  (true synonym)
///   medium ↔ artist  = 0.85  (false positive — below 0.88 threshold)
///   medium ↔ creator = 0.81  (below threshold)
#[tokio::test]
async fn scripted_creator_matches_artist_but_medium_does_not() {
    // Direction 0 = "artist" concept, direction 1 = "medium" concept
    // creator is very close to artist (high similarity), medium is its own thing
    let artist_vec = ScriptedEmbeddingModel::blended_vec(0, 1, 0.0); // pure dir 0
    let creator_vec = ScriptedEmbeddingModel::blended_vec(0, 1, 0.05); // ~0.997 sim to artist
    let medium_vec = ScriptedEmbeddingModel::blended_vec(0, 1, 0.5); // ~0.707 sim to artist

    // The schema service embeds "the {field} of the {descriptive_name}"
    let desc = "Artwork Collection";
    let mut responses = HashMap::new();
    // Descriptive name embeddings (need to be similar for match)
    responses.insert(desc.to_string(), ScriptedEmbeddingModel::unit_vec(10));
    // Field embeddings in context
    responses.insert(format!("the artist of the {}", desc), artist_vec);
    responses.insert(format!("the creator of the {}", desc), creator_vec);
    responses.insert(format!("the medium of the {}", desc), medium_vec);
    responses.insert(
        format!("the title of the {}", desc),
        ScriptedEmbeddingModel::unit_vec(2),
    );
    responses.insert(
        format!("the year of the {}", desc),
        ScriptedEmbeddingModel::unit_vec(3),
    );

    let state = create_scripted_state(responses);

    // Schema A: ["artist", "title", "year"]
    let schema_a = json_to_schema(json!({
        "name": "SchemaA",
        "descriptive_name": desc,
        "fields": ["artist", "title", "year"]
    }));
    state.add_schema(schema_a, HashMap::new()).await.unwrap();

    // Schema B: ["creator", "title", "year", "medium"]
    let schema_b = json_to_schema(json!({
        "name": "SchemaB",
        "descriptive_name": desc,
        "fields": ["creator", "title", "year", "medium"]
    }));
    let outcome = state.add_schema(schema_b, HashMap::new()).await.unwrap();

    match outcome {
        SchemaAddOutcome::Expanded(_, schema, mappers) => {
            let fields = schema.fields.as_ref().expect("must have fields");

            // "creator" renamed to "artist"
            assert!(
                fields.contains(&"artist".to_string()),
                "should have 'artist' (canonical)"
            );
            assert!(
                !fields.contains(&"creator".to_string()),
                "'creator' should be renamed to 'artist'"
            );

            // "medium" preserved as new field
            assert!(
                fields.contains(&"medium".to_string()),
                "'medium' must NOT be falsely matched — it should be a new field"
            );

            assert!(fields.contains(&"title".to_string()));
            assert!(fields.contains(&"year".to_string()));
            assert_eq!(fields.len(), 4, "artist + title + year + medium = 4 fields");

            // mutation_mappers should map creator→artist
            assert_eq!(
                mappers.get("creator").map(|s| s.as_str()),
                Some("artist"),
                "mutation_mappers must map 'creator' → 'artist'"
            );
            // "medium" should NOT appear in mutation_mappers
            assert!(
                !mappers.contains_key("medium"),
                "'medium' should not be in mutation_mappers"
            );
        }
        other => panic!("expected Expanded, got {:?}", other),
    }
}

/// Test that bidirectional matching rejects a non-mutual match even when
/// the forward similarity is above threshold.
///
/// Setup: incoming=[X, Y], existing=[A]
/// X→A = 0.95, Y→A = 0.99  (both above threshold, but Y is the mutual best match)
/// Only Y should match A; X should be treated as a new field.
#[tokio::test]
async fn bidirectional_rejects_non_mutual_best_match() {
    let desc = "Test Domain";
    let a_vec = ScriptedEmbeddingModel::unit_vec(0);
    let x_vec = ScriptedEmbeddingModel::blended_vec(0, 1, 0.1); // high sim to A but not best
    let y_vec = ScriptedEmbeddingModel::blended_vec(0, 1, 0.02); // very high sim to A, is best

    let mut responses = HashMap::new();
    responses.insert(desc.to_string(), ScriptedEmbeddingModel::unit_vec(10));
    responses.insert(format!("the field_a of the {}", desc), a_vec);
    responses.insert(format!("the field_x of the {}", desc), x_vec);
    responses.insert(format!("the field_y of the {}", desc), y_vec);
    responses.insert(
        format!("the shared of the {}", desc),
        ScriptedEmbeddingModel::unit_vec(5),
    );

    let state = create_scripted_state(responses);

    let schema_a = json_to_schema(json!({
        "name": "A",
        "descriptive_name": desc,
        "fields": ["field_a", "shared"]
    }));
    state.add_schema(schema_a, HashMap::new()).await.unwrap();

    let schema_b = json_to_schema(json!({
        "name": "B",
        "descriptive_name": desc,
        "fields": ["field_x", "field_y", "shared"]
    }));
    let outcome = state.add_schema(schema_b, HashMap::new()).await.unwrap();

    match outcome {
        SchemaAddOutcome::Expanded(_, schema, mappers) => {
            let fields = schema.fields.as_ref().unwrap();

            // field_y should match field_a (mutual best match)
            assert!(
                !fields.contains(&"field_y".to_string()),
                "field_y should be renamed to field_a"
            );
            assert!(
                fields.contains(&"field_a".to_string()),
                "field_a should be in expanded schema"
            );
            assert_eq!(mappers.get("field_y").map(|s| s.as_str()), Some("field_a"));

            // field_x should NOT match field_a (non-mutual: A's best match is Y, not X)
            assert!(
                fields.contains(&"field_x".to_string()),
                "field_x should be kept as a new field (non-mutual match)"
            );
            assert!(!mappers.contains_key("field_x"));

            // shared is a literal match
            assert!(fields.contains(&"shared".to_string()));
        }
        other => panic!("expected Expanded, got {:?}", other),
    }
}

/// Test that fields below the similarity threshold are never matched,
/// even if they're the best available match.
#[tokio::test]
async fn below_threshold_fields_are_not_matched() {
    let desc = "Test Domain";
    // Make field_a and field_b somewhat similar but below 0.88 threshold
    // cosine_similarity of blended(0,1,0.3) and unit(0) ≈ 0.7/norm — well below 0.88
    let a_vec = ScriptedEmbeddingModel::unit_vec(0);
    let b_vec = ScriptedEmbeddingModel::blended_vec(0, 1, 0.4); // ~0.83 sim — below 0.88

    let mut responses = HashMap::new();
    responses.insert(desc.to_string(), ScriptedEmbeddingModel::unit_vec(10));
    responses.insert(format!("the field_a of the {}", desc), a_vec);
    responses.insert(format!("the field_b of the {}", desc), b_vec);

    let state = create_scripted_state(responses);

    let schema_a = json_to_schema(json!({
        "name": "A",
        "descriptive_name": desc,
        "fields": ["field_a"]
    }));
    state.add_schema(schema_a, HashMap::new()).await.unwrap();

    let schema_b = json_to_schema(json!({
        "name": "B",
        "descriptive_name": desc,
        "fields": ["field_b"]
    }));
    let outcome = state.add_schema(schema_b, HashMap::new()).await.unwrap();

    match outcome {
        SchemaAddOutcome::Expanded(_, schema, mappers) => {
            let fields = schema.fields.as_ref().unwrap();
            // Both fields should exist — no renaming
            assert!(fields.contains(&"field_a".to_string()));
            assert!(fields.contains(&"field_b".to_string()));
            assert_eq!(fields.len(), 2);
            assert!(
                mappers.is_empty(),
                "no mutation_mappers when below threshold"
            );
        }
        other => panic!("expected Expanded, got {:?}", other),
    }
}

/// Test that two incoming fields cannot both map to the same existing field
/// (many-to-one prevention via claimed set).
#[tokio::test]
async fn many_to_one_mapping_prevented() {
    let desc = "Test Domain";
    let a_vec = ScriptedEmbeddingModel::unit_vec(0);
    // Both synonyms are very close to field_a
    let syn1_vec = ScriptedEmbeddingModel::blended_vec(0, 1, 0.01); // ~0.9999 sim
    let syn2_vec = ScriptedEmbeddingModel::blended_vec(0, 2, 0.02); // ~0.9998 sim

    let mut responses = HashMap::new();
    responses.insert(desc.to_string(), ScriptedEmbeddingModel::unit_vec(10));
    responses.insert(format!("the field_a of the {}", desc), a_vec);
    responses.insert(format!("the synonym1 of the {}", desc), syn1_vec);
    responses.insert(format!("the synonym2 of the {}", desc), syn2_vec);

    let state = create_scripted_state(responses);

    let schema_a = json_to_schema(json!({
        "name": "A",
        "descriptive_name": desc,
        "fields": ["field_a"]
    }));
    state.add_schema(schema_a, HashMap::new()).await.unwrap();

    let schema_b = json_to_schema(json!({
        "name": "B",
        "descriptive_name": desc,
        "fields": ["synonym1", "synonym2"]
    }));
    let outcome = state.add_schema(schema_b, HashMap::new()).await.unwrap();

    match outcome {
        SchemaAddOutcome::Expanded(_, schema, mappers) => {
            let fields = schema.fields.as_ref().unwrap();
            // At most ONE of synonym1/synonym2 should be renamed to field_a
            let renamed_count = [
                !fields.contains(&"synonym1".to_string()),
                !fields.contains(&"synonym2".to_string()),
            ]
            .iter()
            .filter(|&&x| x)
            .count();
            assert!(
                renamed_count <= 1,
                "at most one synonym should be renamed to field_a, but {} were renamed",
                renamed_count
            );
            // field_a must exist
            assert!(fields.contains(&"field_a".to_string()));
            // At most one mutation_mapper
            assert!(mappers.len() <= 1, "at most one mutation_mapper expected");
        }
        other => panic!("expected Expanded, got {:?}", other),
    }
}
