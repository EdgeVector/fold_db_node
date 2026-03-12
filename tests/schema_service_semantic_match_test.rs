#![cfg(feature = "test-utils")]

use fold_db::db_operations::native_index::MockEmbeddingModel;
use fold_db_node::schema_service::server::{SchemaAddOutcome, SchemaServiceState};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::tempdir;

/// Helper function to convert JSON to Schema
fn json_to_schema(value: serde_json::Value) -> fold_db::schema::types::Schema {
    serde_json::from_value(value).expect("failed to deserialize schema from JSON")
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
            let fields = expanded_schema.fields.as_ref().expect("expanded schema must have fields");
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
        matches!(outcome, SchemaAddOutcome::AlreadyExists(_)),
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
            let fields = expanded_schema.fields.as_ref().expect("expanded schema must have fields");
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

    // Schema without descriptive_name — should be added independently
    let schema2 = json_to_schema(json!({
            "name": "Schema2",
            "fields": ["tweet_id", "content", "likes"]
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
        matches!(outcome, SchemaAddOutcome::AlreadyExists(_)),
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
            assert!(fields.contains(&"price".to_string()), "must keep existing-only field");
            assert!(fields.contains(&"shipping_address".to_string()), "must include new field");
            assert_eq!(fields.len(), 6, "superset should have all 6 unique fields");
        }
        other => panic!("80% field overlap + same descriptive name should expand, got {:?}", other),
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
        other => panic!("same descriptive_name should always expand, got {:?}", other),
    }
}
