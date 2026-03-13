#![cfg(feature = "test-utils")]

//! Tests for schema service expansion lifecycle:
//! - Overlapping schemas with same/similar descriptive_name trigger expansion
//! - Expanded schemas have field_mappers pointing to the previous version
//! - Chain expansions (A → B → C) produce correct field_mapper lineage
//! - Old schema name is returned in the Expanded variant for blocking

use fold_db::db_operations::native_index::MockEmbeddingModel;
use fold_db_node::schema_service::server::{SchemaAddOutcome, SchemaServiceState};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::tempdir;

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
    std::mem::forget(temp_dir);

    SchemaServiceState::new_with_embedder(db_path, Arc::new(MockEmbeddingModel))
        .expect("failed to initialize schema service state")
}

// ---------------------------------------------------------------------------
// Basic expansion: same descriptive_name, superset fields
// ---------------------------------------------------------------------------

#[tokio::test]
async fn expansion_returns_old_schema_name_for_blocking() {
    let state = create_test_state();

    let schema_a = json_to_schema(json!({
            "name": "SchemaA",
            "descriptive_name": "Contact Records",
            "fields": ["name", "email", "phone"]
        }));

    let outcome_a = state
        .add_schema(schema_a, HashMap::new())
        .await
        .expect("failed to add schema A");

    let old_hash = match &outcome_a {
        SchemaAddOutcome::Added(s, _) => s.name.clone(),
        other => panic!("expected Added, got {:?}", other),
    };

    // Schema B: same descriptive_name, superset fields (3/3 overlap = 100%)
    let schema_b = json_to_schema(json!({
            "name": "SchemaB",
            "descriptive_name": "Contact Records",
            "fields": ["name", "email", "phone", "address"]
        }));

    let outcome_b = state
        .add_schema(schema_b, HashMap::new())
        .await
        .expect("failed to add schema B");

    match outcome_b {
        SchemaAddOutcome::Expanded(replaced, schema, _) => {
            assert_eq!(
                replaced, old_hash,
                "Expanded must return old schema hash for blocking"
            );
            assert_ne!(
                schema.name, old_hash,
                "expanded schema should have a new identity hash"
            );
        }
        other => panic!("expected Expanded, got {:?}", other),
    }
}

#[tokio::test]
async fn expanded_schema_has_field_mappers_to_predecessor() {
    let state = create_test_state();

    let schema_a = json_to_schema(json!({
            "name": "SchemaA",
            "descriptive_name": "Employee Info",
            "fields": ["emp_id", "name", "department"]
        }));

    let old_hash = match state.add_schema(schema_a, HashMap::new()).await.unwrap() {
        SchemaAddOutcome::Added(s, _) => s.name.clone(),
        other => panic!("expected Added, got {:?}", other),
    };

    // Expand with 1 new field (3/3 shared = 100% overlap)
    let schema_b = json_to_schema(json!({
            "name": "SchemaB",
            "descriptive_name": "Employee Info",
            "fields": ["emp_id", "name", "department", "salary"]
        }));

    let outcome = state.add_schema(schema_b, HashMap::new()).await.unwrap();

    match outcome {
        SchemaAddOutcome::Expanded(_, schema, _) => {
            let mappers = schema
                .field_mappers()
                .expect("expanded schema must have field_mappers");

            // All 3 shared fields should have mappers pointing to old schema
            for field in &["emp_id", "name", "department"] {
                let mapper = mappers
                    .get(*field)
                    .unwrap_or_else(|| panic!("missing field_mapper for '{}'", field));
                assert_eq!(
                    mapper.source_schema(),
                    old_hash,
                    "field_mapper for '{}' should point to old schema",
                    field
                );
                assert_eq!(
                    mapper.source_field(),
                    *field,
                    "field_mapper source_field should match"
                );
            }

            // New field should NOT have a mapper (gets fresh molecule)
            assert!(
                !mappers.contains_key("salary"),
                "new field 'salary' should not have a field_mapper"
            );
        }
        other => panic!("expected Expanded, got {:?}", other),
    }
}

#[tokio::test]
async fn expanded_schema_contains_superset_of_fields() {
    let state = create_test_state();

    let schema_a = json_to_schema(json!({
            "name": "SchemaA",
            "descriptive_name": "Product Catalog",
            "fields": ["sku", "name", "price", "category"]
        }));

    state.add_schema(schema_a, HashMap::new()).await.unwrap();

    // Schema B shares 3/4 fields (75%? no, 3 out of min(4,4)=4 → 75%). Need 80%.
    // Let's make it 4/4 shared + 1 new = 100% overlap
    let schema_b = json_to_schema(json!({
            "name": "SchemaB",
            "descriptive_name": "Product Catalog",
            "fields": ["sku", "name", "price", "category", "weight", "dimensions"]
        }));

    let outcome = state.add_schema(schema_b, HashMap::new()).await.unwrap();

    match outcome {
        SchemaAddOutcome::Expanded(_, schema, _) => {
            let fields = schema.fields.as_ref().expect("must have fields");
            // Superset: all original + new
            for f in &["sku", "name", "price", "category", "weight", "dimensions"] {
                assert!(
                    fields.contains(&f.to_string()),
                    "expanded schema must contain field '{}'",
                    f
                );
            }
            assert_eq!(fields.len(), 6, "should have exactly 6 fields in superset");
        }
        other => panic!("expected Expanded, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Both schemas contribute unique fields → superset includes all
// ---------------------------------------------------------------------------

#[tokio::test]
async fn expansion_merges_fields_from_both_schemas() {
    let state = create_test_state();

    // Schema A: [a, b, c, d, e]
    let schema_a = json_to_schema(json!({
            "name": "SchemaA",
            "descriptive_name": "Sensor Readings",
            "fields": ["sensor_id", "timestamp", "temperature", "humidity", "pressure"]
        }));

    state.add_schema(schema_a, HashMap::new()).await.unwrap();

    // Schema B: shares 4/5 fields (80%) but drops "pressure", adds "wind_speed"
    let schema_b = json_to_schema(json!({
            "name": "SchemaB",
            "descriptive_name": "Sensor Readings",
            "fields": ["sensor_id", "timestamp", "temperature", "humidity", "wind_speed"]
        }));

    let outcome = state.add_schema(schema_b, HashMap::new()).await.unwrap();

    match outcome {
        SchemaAddOutcome::Expanded(_, schema, _) => {
            let fields = schema.fields.as_ref().expect("must have fields");
            // Must be superset of both: all 6 unique fields
            for f in &[
                "sensor_id",
                "timestamp",
                "temperature",
                "humidity",
                "pressure",
                "wind_speed",
            ] {
                assert!(
                    fields.contains(&f.to_string()),
                    "superset must contain '{}' from either schema",
                    f
                );
            }
            assert_eq!(fields.len(), 6);
        }
        other => panic!("expected Expanded, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Chain expansion: A → B → C, each with field_mappers to predecessor
// ---------------------------------------------------------------------------

#[tokio::test]
async fn chain_expansion_produces_correct_field_mapper_lineage() {
    let state = create_test_state();

    // A: [id, name, email]
    let schema_a = json_to_schema(json!({
            "name": "A",
            "descriptive_name": "User Profiles",
            "fields": ["id", "name", "email"]
        }));

    let hash_a = match state.add_schema(schema_a, HashMap::new()).await.unwrap() {
        SchemaAddOutcome::Added(s, _) => s.name.clone(),
        other => panic!("expected Added for A, got {:?}", other),
    };

    // B: superset of A + phone → expands A
    let schema_b = json_to_schema(json!({
            "name": "B",
            "descriptive_name": "User Profiles",
            "fields": ["id", "name", "email", "phone"]
        }));

    let (hash_b, mappers_b) = match state.add_schema(schema_b, HashMap::new()).await.unwrap() {
        SchemaAddOutcome::Expanded(old, schema, _) => {
            assert_eq!(old, hash_a, "B should replace A");
            let m = schema.field_mappers().cloned().unwrap();
            (schema.name.clone(), m)
        }
        other => panic!("expected Expanded for B, got {:?}", other),
    };

    // B's field_mappers point to A for shared fields
    assert_eq!(mappers_b["id"].source_schema(), hash_a);
    assert_eq!(mappers_b["name"].source_schema(), hash_a);
    assert_eq!(mappers_b["email"].source_schema(), hash_a);
    assert!(!mappers_b.contains_key("phone"), "phone is new in B");

    // C: superset of B + avatar → expands B
    let schema_c = json_to_schema(json!({
            "name": "C",
            "descriptive_name": "User Profiles",
            "fields": ["id", "name", "email", "phone", "avatar"]
        }));

    let (_, mappers_c) = match state.add_schema(schema_c, HashMap::new()).await.unwrap() {
        SchemaAddOutcome::Expanded(old, schema, _) => {
            assert_eq!(old, hash_b, "C should replace B");
            let m = schema.field_mappers().cloned().unwrap();
            (schema.name.clone(), m)
        }
        other => panic!("expected Expanded for C, got {:?}", other),
    };

    // C's field_mappers point to B (the most recent predecessor)
    for field in &["id", "name", "email", "phone"] {
        assert_eq!(
            mappers_c[*field].source_schema(),
            hash_b,
            "C's mapper for '{}' should point to B",
            field
        );
    }
    assert!(!mappers_c.contains_key("avatar"), "avatar is new in C");

    // Verify C has all 5 fields
    let all_schemas = state.get_all_schemas_cached().unwrap();
    let c_schemas: Vec<_> = all_schemas
        .iter()
        .filter(|s| s.descriptive_name.as_deref() == Some("User Profiles"))
        .collect();

    // The latest "User Profiles" schema should have 5 fields
    let latest = c_schemas
        .iter()
        .max_by_key(|s| s.fields.as_ref().map(|f| f.len()).unwrap_or(0))
        .expect("should have at least one User Profiles schema");
    assert_eq!(latest.fields.as_ref().unwrap().len(), 5);
}

// ---------------------------------------------------------------------------
// Semantic match + expansion with field_mappers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn semantic_match_expansion_has_field_mappers() {
    let state = create_test_state();

    let schema_a = json_to_schema(json!({
            "name": "A",
            "descriptive_name": "Medical Records",
            "fields": ["patient_id", "diagnosis", "date"]
        }));

    let hash_a = match state.add_schema(schema_a, HashMap::new()).await.unwrap() {
        SchemaAddOutcome::Added(s, _) => s.name.clone(),
        other => panic!("expected Added, got {:?}", other),
    };

    // Similar descriptive name (case change triggers semantic match via MockEmbeddingModel)
    let schema_b = json_to_schema(json!({
            "name": "B",
            "descriptive_name": "medical records",
            "fields": ["patient_id", "diagnosis", "date", "treatment"]
        }));

    let outcome = state.add_schema(schema_b, HashMap::new()).await.unwrap();

    match outcome {
        SchemaAddOutcome::Expanded(old, schema, _) => {
            assert_eq!(old, hash_a);

            // Canonical descriptive_name adopted
            assert_eq!(
                schema.descriptive_name.as_deref(),
                Some("Medical Records"),
                "should adopt original canonical name"
            );

            // field_mappers present for shared fields
            let mappers = schema.field_mappers().expect("must have field_mappers");
            for field in &["patient_id", "diagnosis", "date"] {
                assert_eq!(
                    mappers[*field].source_schema(),
                    hash_a,
                    "mapper for '{}' should point to old schema",
                    field
                );
            }
            assert!(
                !mappers.contains_key("treatment"),
                "new field should not have mapper"
            );
        }
        other => panic!("expected Expanded, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Subset → AlreadyExists (no new schema created, suggests existing)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subset_fields_suggests_existing_schema() {
    let state = create_test_state();

    let schema_a = json_to_schema(json!({
            "name": "A",
            "descriptive_name": "Order Records",
            "fields": ["order_id", "customer", "total", "status", "shipped_at"]
        }));

    let hash_a = match state.add_schema(schema_a, HashMap::new()).await.unwrap() {
        SchemaAddOutcome::Added(s, _) => s.name.clone(),
        other => panic!("expected Added, got {:?}", other),
    };

    // Schema with fewer fields (subset) + same descriptive_name
    let schema_b = json_to_schema(json!({
            "name": "B",
            "descriptive_name": "Order Records",
            "fields": ["order_id", "customer", "total"]
        }));

    let outcome = state.add_schema(schema_b, HashMap::new()).await.unwrap();

    match outcome {
        SchemaAddOutcome::AlreadyExists(schema, _) => {
            assert_eq!(
                schema.name, hash_a,
                "should return the existing superset schema"
            );
            assert_eq!(
                schema.fields.as_ref().unwrap().len(),
                5,
                "returned schema should have all 5 original fields"
            );
        }
        other => panic!("subset should return AlreadyExists, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Low field overlap + same name → still expands (similar name = same concept)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn low_overlap_same_name_still_expands() {
    let state = create_test_state();

    let schema_a = json_to_schema(json!({
            "name": "A",
            "descriptive_name": "Activity Log",
            "fields": ["id", "timestamp", "event_type", "user_id", "details"]
        }));

    let hash_a = match state.add_schema(schema_a, HashMap::new()).await.unwrap() {
        SchemaAddOutcome::Added(s, _) => s.name.clone(),
        other => panic!("expected Added, got {:?}", other),
    };

    // Only 1/5 fields shared, but same descriptive_name → expand to superset
    let schema_b = json_to_schema(json!({
            "name": "B",
            "descriptive_name": "Activity Log",
            "fields": ["id", "calories", "distance", "heart_rate", "duration"]
        }));

    let outcome = state.add_schema(schema_b, HashMap::new()).await.unwrap();

    match outcome {
        SchemaAddOutcome::Expanded(old, schema, _) => {
            assert_eq!(old, hash_a);
            let fields = schema.fields.as_ref().unwrap();
            // Superset of both: 5 + 4 new = 9 unique fields
            assert_eq!(fields.len(), 9);
        }
        other => panic!("same descriptive_name should always expand, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Zero field overlap + same name → still expands to superset
// ---------------------------------------------------------------------------

#[tokio::test]
async fn zero_overlap_same_name_still_expands() {
    let state = create_test_state();

    let schema_a = json_to_schema(json!({
            "name": "A",
            "descriptive_name": "Weather Data",
            "fields": ["temperature", "humidity", "pressure"]
        }));

    let hash_a = match state.add_schema(schema_a, HashMap::new()).await.unwrap() {
        SchemaAddOutcome::Added(s, _) => s.name.clone(),
        other => panic!("expected Added, got {:?}", other),
    };

    // Completely disjoint fields, same descriptive_name → must still expand
    let schema_b = json_to_schema(json!({
            "name": "B",
            "descriptive_name": "Weather Data",
            "fields": ["wind_speed", "visibility", "uv_index"]
        }));

    let outcome = state.add_schema(schema_b, HashMap::new()).await.unwrap();

    match outcome {
        SchemaAddOutcome::Expanded(old, schema, _) => {
            assert_eq!(old, hash_a);
            let fields = schema.fields.as_ref().unwrap();
            assert_eq!(fields.len(), 6, "superset of 3 + 3 disjoint fields = 6");

            // All A-only fields should have mappers
            let mappers = schema.field_mappers().unwrap();
            for f in &["temperature", "humidity", "pressure"] {
                assert_eq!(mappers[*f].source_schema(), hash_a);
            }
            // B-only fields should NOT have mappers
            for f in &["wind_speed", "visibility", "uv_index"] {
                assert!(!mappers.contains_key(*f), "'{}' should not have a mapper", f);
            }
        }
        other => panic!("zero overlap + same name should still expand, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Superseded schema: re-adding old fields returns the active successor
// ---------------------------------------------------------------------------

#[tokio::test]
async fn superseded_schema_returns_active_successor() {
    let state = create_test_state();

    // A: [x, y, z]
    let schema_a = json_to_schema(json!({
            "name": "A",
            "descriptive_name": "Metric Data",
            "fields": ["x", "y", "z"]
        }));

    let hash_a = match state.add_schema(schema_a.clone(), HashMap::new()).await.unwrap() {
        SchemaAddOutcome::Added(s, _) => s.name.clone(),
        other => panic!("expected Added, got {:?}", other),
    };

    // B: superset [x, y, z, w] → expands A
    let schema_b = json_to_schema(json!({
            "name": "B",
            "descriptive_name": "Metric Data",
            "fields": ["x", "y", "z", "w"]
        }));

    let hash_b = match state.add_schema(schema_b, HashMap::new()).await.unwrap() {
        SchemaAddOutcome::Expanded(old, schema, _) => {
            assert_eq!(old, hash_a);
            schema.name.clone()
        }
        other => panic!("expected Expanded, got {:?}", other),
    };

    // Re-add schema with same fields as A (exact identity hash match)
    // Should return the active successor (B), not the superseded A
    let outcome = state.add_schema(schema_a, HashMap::new()).await.unwrap();

    match outcome {
        SchemaAddOutcome::AlreadyExists(schema, _) => {
            assert_eq!(
                schema.name, hash_b,
                "should return the active successor schema, not the superseded one"
            );
            assert_eq!(
                schema.fields.as_ref().unwrap().len(),
                4,
                "returned schema should have all 4 fields from the expanded schema"
            );
        }
        other => panic!("expected AlreadyExists with successor, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Partial overlap + same name → expands with correct field_mappers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn partial_overlap_same_name_expands_with_mappers() {
    let state = create_test_state();

    let schema_a = json_to_schema(json!({
            "name": "A",
            "descriptive_name": "Invoice Records",
            "fields": ["invoice_id", "vendor", "amount", "due_date", "currency"]
        }));

    let hash_a = match state.add_schema(schema_a, HashMap::new()).await.unwrap() {
        SchemaAddOutcome::Added(s, _) => s.name.clone(),
        other => panic!("expected Added, got {:?}", other),
    };

    // 4/5 shared fields + 1 new field
    let schema_b = json_to_schema(json!({
            "name": "B",
            "descriptive_name": "Invoice Records",
            "fields": ["invoice_id", "vendor", "amount", "due_date", "tax"]
        }));

    let outcome = state.add_schema(schema_b, HashMap::new()).await.unwrap();

    match outcome {
        SchemaAddOutcome::Expanded(old, schema, _) => {
            assert_eq!(old, hash_a);
            let fields = schema.fields.as_ref().unwrap();
            // Superset: 5 original + 1 new = 6
            assert_eq!(fields.len(), 6);
            assert!(fields.contains(&"currency".to_string()), "must keep A-only field");
            assert!(fields.contains(&"tax".to_string()), "must include B-only field");

            // field_mappers for all existing fields (from A)
            let mappers = schema.field_mappers().unwrap();
            for f in &["invoice_id", "vendor", "amount", "due_date", "currency"] {
                assert_eq!(mappers[*f].source_schema(), hash_a);
            }
            // B-only field (tax) has no mapper
            assert!(!mappers.contains_key("tax"));
        }
        other => panic!("same descriptive_name should expand, got {:?}", other),
    }
}
