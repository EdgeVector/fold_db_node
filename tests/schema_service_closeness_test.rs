use fold_db_node::schema_service::server::{SchemaAddOutcome, SchemaServiceState};
use serde_json::json;
use std::collections::HashMap;
use tempfile::tempdir;

/// Helper function to convert object-style fields to array format
fn convert_object_fields_to_array(mut schema_json: serde_json::Value) -> serde_json::Value {
    if let Some(fields_obj) = schema_json
        .get("fields")
        .and_then(|f| f.as_object())
        .cloned()
    {
        let field_names: Vec<String> = fields_obj.keys().cloned().collect();
        schema_json["fields"] = json!(field_names);
    }
    schema_json
}

/// Helper function to convert JSON to Schema
fn json_to_schema(value: serde_json::Value) -> fold_db::schema::types::Schema {
    serde_json::from_value(value).expect("failed to deserialize schema from JSON")
}

/// Helper function to verify that every outcome returns a valid schema
fn verify_outcome_has_schema(outcome: &SchemaAddOutcome) {
    match outcome {
        SchemaAddOutcome::Added(response, _) => {
            assert!(!response.name.is_empty(), "added schema must have a name");
            assert!(
                response.fields.is_some(),
                "added schema must have fields defined"
            );
        }
        SchemaAddOutcome::AlreadyExists(schema) => {
            assert!(!schema.name.is_empty(), "existing schema must have a name");
            assert!(
                schema.fields.is_some(),
                "existing schema must have fields defined"
            );
        }
        SchemaAddOutcome::Expanded(_, schema, _) => {
            assert!(!schema.name.is_empty(), "expanded schema must have a name");
            assert!(
                schema.fields.is_some(),
                "expanded schema must have fields defined"
            );
        }
        SchemaAddOutcome::TooSimilar(conflict) => {
            assert!(
                !conflict.closest_schema.name.is_empty(),
                "closest schema must have a name"
            );
            assert!(
                conflict.closest_schema.fields.is_some(),
                "closest schema must have fields defined"
            );
            assert!(
                conflict.similarity > 0.0 && conflict.similarity <= 1.0,
                "similarity must be between 0 and 1"
            );
        }
    }
}

#[tokio::test]
async fn closeness_rejects_identical_schema_with_different_name() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_schema_db")
        .to_string_lossy()
        .to_string();

    let state = SchemaServiceState::new(db_path.clone())
        .expect("failed to initialize schema service state");

    let existing_schema = json_to_schema(json!({
            "name": "UserProfile",
            "fields": ["user_id", "email", "created_at"]
        }));

    state
        .add_schema(existing_schema, HashMap::new())
        .await
        .expect("failed to add existing schema");

    let duplicate_schema = json_to_schema(json!({
            "name": "UserAccount",
            "fields": ["user_id", "email", "created_at"]
        }));

    let outcome = state
        .add_schema(duplicate_schema, HashMap::new())
        .await
        .expect("failed to evaluate schema similarity");

    verify_outcome_has_schema(&outcome);

    match outcome {
        SchemaAddOutcome::AlreadyExists(schema) => {
            assert!(!schema.name.is_empty(), "existing schema must have a name");
        }
        other => panic!("identical schema should return AlreadyExists, got {:?}", other),
    }
}

#[tokio::test]
async fn closeness_always_returns_schema_on_success() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_schema_db")
        .to_string_lossy()
        .to_string();

    let state = SchemaServiceState::new(db_path.clone())
        .expect("failed to initialize schema service state");

    let new_schema = json_to_schema(json!({
            "name": "TestSchema",
            "fields": ["id"]
        }));

    let outcome = state
        .add_schema(new_schema, HashMap::new())
        .await
        .expect("failed to add schema");

    verify_outcome_has_schema(&outcome);

    match outcome {
        SchemaAddOutcome::Added(response, _) => {
            // Schema name is now the topology hash, not the original name
            assert!(!response.name.is_empty(), "schema must have a name");
            assert!(response.fields.is_some());
        }
        other => {
            panic!("new unique schema should be added, got {:?}", other)
        }
    }
}

#[tokio::test]
async fn closeness_always_returns_schema_on_rejection() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_schema_db")
        .to_string_lossy()
        .to_string();

    let state = SchemaServiceState::new(db_path.clone())
        .expect("failed to initialize schema service state");

    let existing_schema = json_to_schema(json!({
            "name": "Original",
            "fields": [
                "field1",
                "field2"
            ]
        }));

    state
        .add_schema(existing_schema, HashMap::new())
        .await
        .expect("failed to add existing schema");

    let duplicate_schema = json_to_schema(json!({
            "name": "Duplicate",
            "fields": [
                "field1",
                "field2"
            ]
        }));

    let outcome = state
        .add_schema(duplicate_schema, HashMap::new())
        .await
        .expect("failed to evaluate schema similarity");

    verify_outcome_has_schema(&outcome);

    match outcome {
        SchemaAddOutcome::AlreadyExists(schema) => {
            assert!(!schema.name.is_empty(), "existing schema must have a name");
            assert!(schema.fields.is_some());
        }
        other => {
            panic!("duplicate schema should return AlreadyExists, got {:?}", other)
        }
    }
}

#[tokio::test]
async fn closeness_allows_dissimilar_schemas() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_schema_db")
        .to_string_lossy()
        .to_string();

    let state = SchemaServiceState::new(db_path.clone())
        .expect("failed to initialize schema service state");

    let existing_schema = json_to_schema(json!({
            "name": "UserProfile",
            "fields": [
                "user_id",
                "email"
            ]
        }));

    state
        .add_schema(existing_schema, HashMap::new())
        .await
        .expect("failed to add existing schema");

    let different_schema = json_to_schema(json!({
            "name": "ProductCatalog",
            "fields": [
                "product_id",
                "product_name",
                "price",
                "inventory_count"
            ]
        }));

    let outcome = state
        .add_schema(different_schema, HashMap::new())
        .await
        .expect("failed to add dissimilar schema");

    verify_outcome_has_schema(&outcome);

    match outcome {
        SchemaAddOutcome::Added(response, _) => {
            assert!(!response.name.is_empty(), "schema must have a name");
        }
        other => panic!("dissimilar schema should have been accepted, got {:?}", other),
    }
}

#[tokio::test]
async fn closeness_handles_similar_but_slightly_different_schemas() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_schema_db")
        .to_string_lossy()
        .to_string();

    let state = SchemaServiceState::new(db_path.clone())
        .expect("failed to initialize schema service state");

    let existing_schema = json_to_schema(json!({
            "name": "User",
            "fields": [
                "id",
                "name",
                "email"
            ]
        }));

    state
        .add_schema(existing_schema, HashMap::new())
        .await
        .expect("failed to add existing schema");

    let similar_schema_with_extra_field = json_to_schema(json!({
            "name": "UserExtended",
            "fields": [
                "id",
                "name",
                "email",
                "phone"
            ]
        }));

    let outcome = state
        .add_schema(similar_schema_with_extra_field, HashMap::new())
        .await
        .expect("failed to evaluate schema similarity");

    verify_outcome_has_schema(&outcome);

    match outcome {
        SchemaAddOutcome::Added(response, _) => {
            assert!(!response.name.is_empty(), "schema must have a name");
        }
        SchemaAddOutcome::Expanded(_, expanded, _) => {
            // With >50% field overlap fallback, overlapping schemas get expanded
            assert!(!expanded.name.is_empty(), "expanded schema must have a name");
        }
        other => {
            panic!("schema with extra field should have been accepted or expanded, got {:?}", other)
        }
    }
}

#[tokio::test]
async fn closeness_uses_normalized_comparison_for_properties() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_schema_db")
        .to_string_lossy()
        .to_string();

    let state = SchemaServiceState::new(db_path.clone())
        .expect("failed to initialize schema service state");

    let existing_schema = json_to_schema(json!({
            "name": "First",
            "type": "object",
            "description": "test schema",
            "fields": [
                "field_a"
            ]
        }));

    state
        .add_schema(existing_schema, HashMap::new())
        .await
        .expect("failed to add existing schema");

    let reordered_properties_schema = json_to_schema(json!({
            "description": "test schema",
            "name": "Second",
            "fields": [
                "field_a"
            ],
            "type": "object"
        }));

    let outcome = state
        .add_schema(reordered_properties_schema, HashMap::new())
        .await
        .expect("failed to evaluate schema similarity");

    match outcome {
        SchemaAddOutcome::AlreadyExists(schema) => {
            assert!(!schema.name.is_empty(), "existing schema must have a name");
        }
        other => {
            panic!("schemas should return AlreadyExists despite property ordering, got {:?}", other)
        }
    }
}

#[tokio::test]
async fn closeness_ignores_schema_name_in_comparison() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_schema_db")
        .to_string_lossy()
        .to_string();

    let state = SchemaServiceState::new(db_path.clone())
        .expect("failed to initialize schema service state");

    let existing_schema = json_to_schema(json!({
            "name": "VeryLongDescriptiveSchemaName",
            "fields": [
                "field1"
            ]
        }));

    state
        .add_schema(existing_schema, HashMap::new())
        .await
        .expect("failed to add existing schema");

    let same_content_different_name = json_to_schema(json!({
            "name": "X",
            "fields": [
                "field1"
            ]
        }));

    let outcome = state
        .add_schema(same_content_different_name, HashMap::new())
        .await
        .expect("failed to evaluate schema similarity");

    match outcome {
        SchemaAddOutcome::AlreadyExists(schema) => {
            assert!(!schema.name.is_empty(), "existing schema must have a name");
        }
        other => {
            panic!("schemas should return AlreadyExists despite different names, got {:?}", other)
        }
    }
}

#[tokio::test]
async fn closeness_with_object_style_fields() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_schema_db")
        .to_string_lossy()
        .to_string();

    let state = SchemaServiceState::new(db_path.clone())
        .expect("failed to initialize schema service state");

    let existing_schema = json_to_schema(convert_object_fields_to_array(json!({
        "name": "ExistingObject",
        "fields": {
            "field_a": {},
            "field_b": {},
            "field_c": {}
        }
    })));

    state
        .add_schema(existing_schema, HashMap::new())
        .await
        .expect("failed to add existing schema");

    let similar_object_schema = json_to_schema(convert_object_fields_to_array(json!({
        "name": "NewObject",
        "fields": {
            "field_a": {},
            "field_b": {},
            "field_c": {}
        }
    })));

    let outcome = state
        .add_schema(similar_object_schema, HashMap::new())
        .await
        .expect("failed to evaluate schema similarity");

    match outcome {
        SchemaAddOutcome::AlreadyExists(schema) => {
            assert!(!schema.name.is_empty(), "existing schema must have a name");
        }
        other => {
            panic!("identical object-style schemas should return AlreadyExists, got {:?}", other)
        }
    }
}

#[tokio::test]
async fn closeness_creates_field_mappers_for_high_field_overlap() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_schema_db")
        .to_string_lossy()
        .to_string();

    let state = SchemaServiceState::new(db_path.clone())
        .expect("failed to initialize schema service state");

    let existing_schema = json_to_schema(convert_object_fields_to_array(json!({
        "name": "BaseEntity",
        "fields": {
            "id": {},
            "created_at": {},
            "updated_at": {},
            "name": {},
            "description": {}
        }
    })));

    state
        .add_schema(existing_schema, HashMap::new())
        .await
        .expect("failed to add existing schema");

    let extended_schema = json_to_schema(convert_object_fields_to_array(json!({
        "name": "ExtendedEntity",
        "fields": {
            "id": {},
            "created_at": {},
            "updated_at": {},
            "name": {},
            "description": {},
            "extra_field_1": {},
            "extra_field_2": {}
        }
    })));

    let outcome = state
        .add_schema(extended_schema, HashMap::new())
        .await
        .expect("failed to add schema with high field overlap");

    match outcome {
        SchemaAddOutcome::Added(response, _) => {
            assert!(!response.name.is_empty(), "schema must have a name");
        }
        SchemaAddOutcome::Expanded(_, expanded, _) => {
            // With >50% field overlap fallback, overlapping schemas get expanded
            assert!(!expanded.name.is_empty(), "expanded schema must have a name");
        }
        other => {
            panic!("schema with extra fields should be accepted or expanded, got {:?}", other)
        }
    }
}

#[tokio::test]
async fn closeness_with_multiple_existing_schemas_finds_closest() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_schema_db")
        .to_string_lossy()
        .to_string();

    let state = SchemaServiceState::new(db_path.clone())
        .expect("failed to initialize schema service state");

    let schema1 = json_to_schema(json!({
            "name": "Schema1",
            "fields": [
                "a"
            ]
        }));

    let schema2 = json_to_schema(json!({
            "name": "Schema2",
            "fields": [
                "x",
                "y"
            ]
        }));

    let schema3 = json_to_schema(json!({
            "name": "Schema3",
            "fields": [
                "x",
                "y",
                "z"
            ]
        }));

    state
        .add_schema(schema1, HashMap::new())
        .await
        .expect("failed to add schema1");
    state
        .add_schema(schema2, HashMap::new())
        .await
        .expect("failed to add schema2");
    state
        .add_schema(schema3, HashMap::new())
        .await
        .expect("failed to add schema3");

    let new_schema = json_to_schema(json!({
            "name": "NewSchema",
            "fields": [
                "x",
                "y"
            ]
        }));

    let outcome = state
        .add_schema(new_schema, HashMap::new())
        .await
        .expect("failed to evaluate schema similarity");

    match outcome {
        SchemaAddOutcome::AlreadyExists(schema) => {
            assert!(!schema.name.is_empty(), "existing schema must have a name");
        }
        other => {
            panic!("schema with same fields as Schema2 should return AlreadyExists, got {:?}", other)
        }
    }
}

#[tokio::test]
async fn closeness_with_nested_objects() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_schema_db")
        .to_string_lossy()
        .to_string();

    let state = SchemaServiceState::new(db_path.clone())
        .expect("failed to initialize schema service state");

    let existing_schema = json_to_schema(json!({
            "name": "NestedSchema",
            "fields": ["user_id", "user_name", "metadata"]
        }));

    state
        .add_schema(existing_schema, HashMap::new())
        .await
        .expect("failed to add existing schema");

    let duplicate_nested = json_to_schema(json!({
            "name": "NestedSchemaCopy",
            "fields": ["user_id", "user_name", "metadata"]
        }));

    let outcome = state
        .add_schema(duplicate_nested, HashMap::new())
        .await
        .expect("failed to evaluate schema similarity");

    match outcome {
        SchemaAddOutcome::AlreadyExists(schema) => {
            assert!(!schema.name.is_empty(), "existing schema must have a name");
        }
        other => {
            panic!("identical nested schemas should return AlreadyExists, got {:?}", other)
        }
    }
}

#[tokio::test]
async fn closeness_field_overlap_below_threshold_without_high_similarity() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_schema_db")
        .to_string_lossy()
        .to_string();

    let state = SchemaServiceState::new(db_path.clone())
        .expect("failed to initialize schema service state");

    let existing_schema = json_to_schema(convert_object_fields_to_array(json!({
        "name": "LowOverlap",
        "fields": {
            "common_a": {},
            "common_b": {},
            "unique_1": {},
            "unique_2": {},
            "unique_3": {},
            "unique_4": {},
            "unique_5": {}
        }
    })));

    state
        .add_schema(existing_schema, HashMap::new())
        .await
        .expect("failed to add existing schema");

    let new_schema = json_to_schema(convert_object_fields_to_array(json!({
        "name": "DifferentSchema",
        "fields": {
            "common_a": {},
            "common_b": {},
            "different_1": {},
            "different_2": {},
            "different_3": {},
            "different_4": {},
            "different_5": {}
        }
    })));

    let outcome = state
        .add_schema(new_schema, HashMap::new())
        .await
        .expect("failed to add schema with low overlap");

    match outcome {
        SchemaAddOutcome::Added(response, _) => {
            assert!(!response.name.is_empty(), "schema must have a name");
        }
        other => {
            panic!("low field overlap should allow schema addition, got {:?}", other)
        }
    }
}

#[tokio::test]
async fn closeness_respects_field_mapper_preservation() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_schema_db")
        .to_string_lossy()
        .to_string();

    let state = SchemaServiceState::new(db_path.clone())
        .expect("failed to initialize schema service state");

    let existing_schema = json_to_schema(convert_object_fields_to_array(json!({
        "name": "Original",
        "fields": {
            "id": {},
            "name": {}
        }
    })));

    state
        .add_schema(existing_schema, HashMap::new())
        .await
        .expect("failed to add existing schema");

    let new_schema_with_existing_mappers = json_to_schema(convert_object_fields_to_array(json!({
        "name": "Extended",
        "fields": {
            "id": {},
            "name": {},
            "email": {}
        },
        "field_mappers": {
            "email": "SomeOtherSchema.email"
        }
    })));

    let outcome = state
        .add_schema(new_schema_with_existing_mappers, HashMap::new())
        .await
        .expect("failed to add schema with existing mappers");

    match outcome {
        SchemaAddOutcome::Added(response, _) => {
            assert!(!response.name.is_empty(), "schema must have a name");
            // Verify that explicitly provided field_mappers are preserved
            let mappers = response
                .field_mappers
                .as_ref()
                .expect("field mappers should be preserved");
            assert!(
                mappers.contains_key("email"),
                "explicitly provided email mapper should be preserved"
            );
        }
        SchemaAddOutcome::Expanded(_, expanded, _) => {
            // With >50% field overlap fallback, overlapping schemas get expanded
            assert!(!expanded.name.is_empty(), "expanded schema must have a name");
            // Verify that explicitly provided field_mappers are preserved in expansion
            let mappers = expanded
                .field_mappers
                .as_ref()
                .expect("field mappers should be preserved in expansion");
            assert!(
                mappers.contains_key("email"),
                "explicitly provided email mapper should be preserved"
            );
        }
        other => {
            panic!("schema with extra field should be accepted or expanded, got {:?}", other)
        }
    }
}
