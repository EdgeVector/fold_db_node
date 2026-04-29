#![cfg(feature = "test-utils")]

use fold_db::schema::types::data_classification::DataClassification;
use schema_service_core::embedder::{MockEmbeddingModel, ScriptedEmbeddingModel};
use schema_service_core::state::SchemaServiceState;
use schema_service_core::types::SchemaAddOutcome;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::tempdir;

/// Helper function to convert JSON to Schema. Auto-fills field
/// descriptions and data classifications for any field that doesn't
/// have them explicitly set in the JSON. Descriptions are filled
/// with the literal `"{field} field"`, which determines the format
/// of [`scripted_field_key`].
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

/// Build the exact text that
/// `SchemaServiceState::get_field_embedding` will pass to the
/// embedder for `(field_name, descriptive_name)` when the field has
/// the auto-filled description from [`json_to_schema`]. Scripted
/// tests use this to register vectors under the right key.
fn scripted_field_key(field_name: &str, descriptive_name: &str) -> String {
    format!(
        "the {} of the {}: {} field",
        field_name, descriptive_name, field_name
    )
}

/// Build the description-only embed text that
/// `SchemaServiceState::canonicalize_fields` and
/// `register_canonical_fields` pass to the embedder. With
/// [`json_to_schema`]'s auto-filled descriptions, this is just
/// `"{field_name} field"`. Scripted tests must register a vector
/// under this key so the global canonical-field registry doesn't
/// fall through to the byte-sum fallback (which gives spurious high
/// similarity for any two strings sharing the `" field"` suffix).
fn scripted_field_description_key(field_name: &str) -> String {
    format!("{} field", field_name)
}

/// Insert both the canonicalize_fields embed key (description only)
/// and the semantic_field_rename_map embed key (full context format)
/// for `(field_name, descriptive_name)`, both pointing at `vec`.
/// This lets a scripted test control similarity at both layers
/// consistently. Use whenever a field needs to be matched (or
/// distinguished) by the matcher.
fn insert_field_vector(
    responses: &mut HashMap<String, Vec<f32>>,
    field_name: &str,
    descriptive_name: &str,
    vec: Vec<f32>,
) {
    responses.insert(scripted_field_description_key(field_name), vec.clone());
    responses.insert(scripted_field_key(field_name, descriptive_name), vec);
}

/// Insert orthogonal description-only embedding so the global
/// canonical-field registry's bidirectional matcher
/// (`canonicalize_fields`, threshold 0.88) sees the field as
/// non-matching, and only the schema-level matcher
/// (`semantic_field_rename_map`, threshold 0.84) gets a vote on
/// whether the field overlaps with an existing schema's field.
///
/// Use when the test specifically targets bidirectional/threshold
/// behavior in `semantic_field_rename_map` and you need
/// `canonicalize_fields` to stay out of the way. The
/// `desc_orthogonal_dir` should be a unique per-field direction in
/// the 384-dim space — values >=20 stay clear of the
/// `unit_vec(0..=10)` band the rest of the tests use.
fn insert_field_vector_canonicalize_orthogonal(
    responses: &mut HashMap<String, Vec<f32>>,
    field_name: &str,
    descriptive_name: &str,
    semantic_vec: Vec<f32>,
    desc_orthogonal_dir: usize,
) {
    responses.insert(
        scripted_field_description_key(field_name),
        ScriptedEmbeddingModel::unit_vec(desc_orthogonal_dir),
    );
    responses.insert(
        scripted_field_key(field_name, descriptive_name),
        semantic_vec,
    );
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

    SchemaServiceState::new(db_path, Arc::new(MockEmbeddingModel))
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
    // Originally written assuming MockEmbeddingModel produces high
    // cosine similarity for case-only differences ("Twitter Posts"
    // ≈ "twitter posts"). It does not — MockEmbeddingModel hashes
    // the input string and any character change yields a near-
    // orthogonal vector. Rewritten to use ScriptedEmbeddingModel so
    // the descriptive_name semantic-match path is exercised
    // deterministically, regardless of how the mock embeds.
    let mut responses = HashMap::new();
    // "Twitter Posts" and "twitter posts" embed to nearly the same
    // vector — that's what "case difference is semantically
    // equivalent" means in this test.
    let posts_vec = ScriptedEmbeddingModel::blended_vec(0, 1, 0.0);
    let posts_lower_vec = ScriptedEmbeddingModel::blended_vec(0, 1, 0.05);
    responses.insert("Twitter Posts".to_string(), posts_vec);
    responses.insert("twitter posts".to_string(), posts_lower_vec);
    insert_field_vector_canonicalize_orthogonal(
        &mut responses,
        "tweet_id",
        "Twitter Posts",
        ScriptedEmbeddingModel::unit_vec(50),
        51,
    );
    insert_field_vector_canonicalize_orthogonal(
        &mut responses,
        "content",
        "Twitter Posts",
        ScriptedEmbeddingModel::unit_vec(52),
        53,
    );
    insert_field_vector_canonicalize_orthogonal(
        &mut responses,
        "author",
        "Twitter Posts",
        ScriptedEmbeddingModel::unit_vec(54),
        55,
    );
    // The matcher also embeds context for each existing field
    // against the *incoming* descriptive_name during expansion.
    insert_field_vector_canonicalize_orthogonal(
        &mut responses,
        "tweet_id",
        "twitter posts",
        ScriptedEmbeddingModel::unit_vec(50),
        51,
    );
    insert_field_vector_canonicalize_orthogonal(
        &mut responses,
        "content",
        "twitter posts",
        ScriptedEmbeddingModel::unit_vec(52),
        53,
    );
    insert_field_vector_canonicalize_orthogonal(
        &mut responses,
        "author",
        "twitter posts",
        ScriptedEmbeddingModel::unit_vec(54),
        55,
    );
    let state = create_scripted_state(responses);

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

    // Add a schema with a semantically similar descriptive_name
    // (only case difference). The scripted similarity between
    // "Twitter Posts" and "twitter posts" is above the 0.8
    // descriptive-name match threshold, triggering expansion.
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
    // See note on `semantic_match_similar_descriptive_name_triggers_expansion`
    // for why this uses ScriptedEmbeddingModel rather than MockEmbeddingModel.
    let mut responses = HashMap::new();
    responses.insert(
        "User Profile Data".to_string(),
        ScriptedEmbeddingModel::blended_vec(0, 1, 0.0),
    );
    responses.insert(
        "user profile data".to_string(),
        ScriptedEmbeddingModel::blended_vec(0, 1, 0.05),
    );
    for (idx, field) in ["user_id", "name", "email"].iter().enumerate() {
        for desc in ["User Profile Data", "user profile data"] {
            insert_field_vector_canonicalize_orthogonal(
                &mut responses,
                field,
                desc,
                ScriptedEmbeddingModel::unit_vec(60 + idx * 2),
                61 + idx * 2,
            );
        }
    }
    let state = create_scripted_state(responses);

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
    // See note on `semantic_match_similar_descriptive_name_triggers_expansion`
    // for why this uses ScriptedEmbeddingModel rather than MockEmbeddingModel.
    let mut responses = HashMap::new();
    responses.insert(
        "Employee Records".to_string(),
        ScriptedEmbeddingModel::blended_vec(0, 1, 0.0),
    );
    responses.insert(
        "employee records".to_string(),
        ScriptedEmbeddingModel::blended_vec(0, 1, 0.05),
    );
    for (idx, field) in ["emp_id", "name", "department", "salary"]
        .iter()
        .enumerate()
    {
        for desc in ["Employee Records", "employee records"] {
            insert_field_vector_canonicalize_orthogonal(
                &mut responses,
                field,
                desc,
                ScriptedEmbeddingModel::unit_vec(80 + idx * 2),
                81 + idx * 2,
            );
        }
    }
    let state = create_scripted_state(responses);

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
/// Local adapter for this test: wrap fold_db's `FastEmbedModel` so it
/// satisfies `schema_service_core::Embedder`. The production equivalent
/// lives in `schema_service_server_shared::FoldDbFastEmbedder`; we inline
/// it here to keep fold_db_node off that dependency.
struct FastEmbedAdapter(fold_db::db_operations::native_index::FastEmbedModel);

impl schema_service_core::Embedder for FastEmbedAdapter {
    fn embed_text(&self, text: &str) -> Result<Vec<f32>, schema_service_core::EmbedError> {
        use fold_db::db_operations::native_index::Embedder as _;
        self.0
            .embed_text(text)
            .map_err(|e| schema_service_core::EmbedError::EmbedFailed(e.to_string()))
    }
}

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

    let state = SchemaServiceState::new(db_path, Arc::new(FastEmbedAdapter(FastEmbedModel::new())))
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
    SchemaServiceState::new(db_path, Arc::new(embedder))
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

    // Two layers script the same vectors so canonicalize_fields and
    // semantic_field_rename_map agree on similarity:
    //   1. canonicalize_fields embeds just the description.
    //   2. semantic_field_rename_map embeds the full
    //      "the {field} of the {desc}: {description}" string.
    let desc = "Artwork Collection";
    let mut responses = HashMap::new();
    // Descriptive-name embedding (used by find_matching_descriptive_name).
    responses.insert(desc.to_string(), ScriptedEmbeddingModel::unit_vec(10));
    insert_field_vector(&mut responses, "artist", desc, artist_vec);
    insert_field_vector(&mut responses, "creator", desc, creator_vec);
    insert_field_vector(&mut responses, "medium", desc, medium_vec);
    insert_field_vector(
        &mut responses,
        "title",
        desc,
        ScriptedEmbeddingModel::unit_vec(2),
    );
    insert_field_vector(
        &mut responses,
        "year",
        desc,
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

    // Make descriptions orthogonal in canonical-registry space so
    // canonicalize_fields stays out of the way and the bidirectional
    // check in semantic_field_rename_map is the actual unit under
    // test (fields are still nearly-identical at the
    // schema-context level via the `*_vec` semantic embeddings).
    let mut responses = HashMap::new();
    responses.insert(desc.to_string(), ScriptedEmbeddingModel::unit_vec(10));
    insert_field_vector_canonicalize_orthogonal(&mut responses, "field_a", desc, a_vec, 20);
    insert_field_vector_canonicalize_orthogonal(&mut responses, "field_x", desc, x_vec, 21);
    insert_field_vector_canonicalize_orthogonal(&mut responses, "field_y", desc, y_vec, 22);
    insert_field_vector_canonicalize_orthogonal(
        &mut responses,
        "shared",
        desc,
        ScriptedEmbeddingModel::unit_vec(5),
        23,
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
    insert_field_vector(&mut responses, "field_a", desc, a_vec);
    insert_field_vector(&mut responses, "field_b", desc, b_vec);

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

    // Same canonicalize-orthogonal trick as
    // bidirectional_rejects_non_mutual_best_match: keep
    // canonicalize_fields out of it so the schema-level matcher's
    // many-to-one prevention (the `claimed` set in
    // semantic_field_rename_map) is exercised directly.
    let mut responses = HashMap::new();
    responses.insert(desc.to_string(), ScriptedEmbeddingModel::unit_vec(10));
    insert_field_vector_canonicalize_orthogonal(&mut responses, "field_a", desc, a_vec, 30);
    insert_field_vector_canonicalize_orthogonal(&mut responses, "synonym1", desc, syn1_vec, 31);
    insert_field_vector_canonicalize_orthogonal(&mut responses, "synonym2", desc, syn2_vec, 32);

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
