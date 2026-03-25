use fold_db::schema::types::data_classification::DataClassification;
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
    let mut schema: fold_db::schema::types::Schema =
        serde_json::from_value(value).expect("failed to deserialize schema from JSON");
    // Ensure descriptive_name is set (required by schema service)
    if schema.descriptive_name.is_none() {
        schema.descriptive_name = Some(schema.name.clone());
    }
    // Ensure field_descriptions and data classifications are set (required by schema service)
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
        SchemaAddOutcome::AlreadyExists(schema, _) => {
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
    }
}

#[tokio::test]
async fn closeness_treats_different_names_as_distinct_schemas() {
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

    // Same fields but different name — identity hash now includes name,
    // so these are distinct schemas.
    let different_name_schema = json_to_schema(json!({
        "name": "UserAccount",
        "fields": ["user_id", "email", "created_at"]
    }));

    let outcome = state
        .add_schema(different_name_schema, HashMap::new())
        .await
        .expect("failed to evaluate schema similarity");

    verify_outcome_has_schema(&outcome);

    match outcome {
        SchemaAddOutcome::Added(response, _) => {
            assert!(!response.name.is_empty(), "added schema must have a name");
        }
        other => panic!(
            "schema with different name should be Added as distinct, got {:?}",
            other
        ),
    }
}

#[tokio::test]
async fn closeness_rejects_identical_schema_with_same_name() {
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

    // Same name AND same fields → same identity hash → AlreadyExists
    let duplicate_schema = json_to_schema(json!({
        "name": "UserProfile",
        "fields": ["user_id", "email", "created_at"]
    }));

    let outcome = state
        .add_schema(duplicate_schema, HashMap::new())
        .await
        .expect("failed to evaluate schema similarity");

    verify_outcome_has_schema(&outcome);

    match outcome {
        SchemaAddOutcome::AlreadyExists(schema, _) => {
            assert!(!schema.name.is_empty(), "existing schema must have a name");
        }
        other => panic!(
            "identical schema (same name + fields) should return AlreadyExists, got {:?}",
            other
        ),
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

    // Same name AND same fields → AlreadyExists (identity hash match)
    let duplicate_schema = json_to_schema(json!({
        "name": "Original",
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
        SchemaAddOutcome::AlreadyExists(schema, _) => {
            assert!(!schema.name.is_empty(), "existing schema must have a name");
            assert!(schema.fields.is_some());
        }
        other => {
            panic!(
                "duplicate schema (same name + fields) should return AlreadyExists, got {:?}",
                other
            )
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
        other => panic!(
            "dissimilar schema should have been accepted, got {:?}",
            other
        ),
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
            assert!(
                !expanded.name.is_empty(),
                "expanded schema must have a name"
            );
        }
        other => {
            panic!(
                "schema with extra field should have been accepted or expanded, got {:?}",
                other
            )
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
        "name": "TestSchema",
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

    // Same name, same fields, but JSON properties in different order —
    // should still match as AlreadyExists since identity hash is the same.
    let reordered_properties_schema = json_to_schema(json!({
        "description": "test schema",
        "name": "TestSchema",
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
        SchemaAddOutcome::AlreadyExists(schema, _) => {
            assert!(!schema.name.is_empty(), "existing schema must have a name");
        }
        other => {
            panic!("schemas with same name+fields should return AlreadyExists despite property ordering, got {:?}", other)
        }
    }
}

#[tokio::test]
async fn closeness_includes_schema_name_in_identity() {
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

    // Different name with same fields — identity hash includes name,
    // so these are distinct schemas and the new one should be Added.
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
        SchemaAddOutcome::Added(response, _) => {
            assert!(!response.name.is_empty(), "added schema must have a name");
        }
        other => {
            panic!(
                "schemas with different names should be Added as distinct, got {:?}",
                other
            )
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

    // Same descriptive_name + same fields → should dedup as AlreadyExists
    let similar_object_schema = json_to_schema(convert_object_fields_to_array(json!({
        "name": "ExistingObject",
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
        SchemaAddOutcome::AlreadyExists(schema, _) => {
            assert!(!schema.name.is_empty(), "existing schema must have a name");
        }
        other => {
            panic!(
                "identical object-style schemas should return AlreadyExists, got {:?}",
                other
            )
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
            assert!(
                !expanded.name.is_empty(),
                "expanded schema must have a name"
            );
        }
        other => {
            panic!(
                "schema with extra fields should be accepted or expanded, got {:?}",
                other
            )
        }
    }
}

#[tokio::test]
async fn closeness_with_multiple_existing_schemas_same_name_matches() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_schema_db")
        .to_string_lossy()
        .to_string();

    let state = SchemaServiceState::new(db_path.clone())
        .expect("failed to initialize schema service state");

    // Use semantically distinct names to avoid false embedding matches
    let schema1 = json_to_schema(json!({
        "name": "weather_stations",
        "descriptive_name": "Weather Station Records",
        "fields": [
            "temperature"
        ]
    }));

    let schema2 = json_to_schema(json!({
        "name": "pet_records",
        "descriptive_name": "Pet Health Records",
        "fields": [
            "breed",
            "weight"
        ]
    }));

    let schema3 = json_to_schema(json!({
        "name": "grocery_items",
        "descriptive_name": "Grocery Inventory Items",
        "fields": [
            "product",
            "price",
            "quantity"
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

    // Same name AND same fields as pet_records → AlreadyExists
    let new_schema = json_to_schema(json!({
        "name": "pet_records",
        "descriptive_name": "Pet Health Records",
        "fields": [
            "breed",
            "weight"
        ]
    }));

    let outcome = state
        .add_schema(new_schema, HashMap::new())
        .await
        .expect("failed to evaluate schema similarity");

    match outcome {
        SchemaAddOutcome::AlreadyExists(schema, _) => {
            assert!(!schema.name.is_empty(), "existing schema must have a name");
        }
        other => {
            panic!(
                "schema with same name+fields as pet_records should return AlreadyExists, got {:?}",
                other
            )
        }
    }
}

#[tokio::test]
async fn closeness_with_multiple_existing_schemas_different_name_is_distinct() {
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
        "fields": ["a"]
    }));

    let schema2 = json_to_schema(json!({
        "name": "Schema2",
        "fields": ["x", "y"]
    }));

    state
        .add_schema(schema1, HashMap::new())
        .await
        .expect("failed to add schema1");
    state
        .add_schema(schema2, HashMap::new())
        .await
        .expect("failed to add schema2");

    // Same fields as Schema2 but different name → distinct schema, Added
    let new_schema = json_to_schema(json!({
        "name": "NewSchema",
        "fields": ["x", "y"]
    }));

    let outcome = state
        .add_schema(new_schema, HashMap::new())
        .await
        .expect("failed to evaluate schema similarity");

    match outcome {
        SchemaAddOutcome::Added(response, _) => {
            assert!(!response.name.is_empty(), "added schema must have a name");
        }
        other => {
            panic!(
                "schema with different name should be Added as distinct, got {:?}",
                other
            )
        }
    }
}

#[tokio::test]
async fn closeness_with_nested_objects_same_name() {
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

    // Same name AND same fields → AlreadyExists
    let duplicate_nested = json_to_schema(json!({
        "name": "NestedSchema",
        "fields": ["user_id", "user_name", "metadata"]
    }));

    let outcome = state
        .add_schema(duplicate_nested, HashMap::new())
        .await
        .expect("failed to evaluate schema similarity");

    match outcome {
        SchemaAddOutcome::AlreadyExists(schema, _) => {
            assert!(!schema.name.is_empty(), "existing schema must have a name");
        }
        other => {
            panic!(
                "identical schemas (same name + fields) should return AlreadyExists, got {:?}",
                other
            )
        }
    }
}

#[tokio::test]
async fn closeness_with_nested_objects_different_name_is_distinct() {
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

    // Same fields but different name → distinct schema
    let different_name = json_to_schema(json!({
        "name": "NestedSchemaCopy",
        "fields": ["user_id", "user_name", "metadata"]
    }));

    let outcome = state
        .add_schema(different_name, HashMap::new())
        .await
        .expect("failed to evaluate schema similarity");

    match outcome {
        SchemaAddOutcome::Added(response, _) => {
            assert!(!response.name.is_empty(), "added schema must have a name");
        }
        other => {
            panic!(
                "schema with different name should be Added as distinct, got {:?}",
                other
            )
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
            panic!(
                "low field overlap should allow schema addition, got {:?}",
                other
            )
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
            assert!(
                !expanded.name.is_empty(),
                "expanded schema must have a name"
            );
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
            panic!(
                "schema with extra field should be accepted or expanded, got {:?}",
                other
            )
        }
    }
}

/// Schemas from the same broad domain (art/images) but with different schema
/// names must NOT be merged. This tests the real scenario where "famous_paintings"
/// and "holiday_illustrations" share the same image fields but are clearly
/// different collections.
#[tokio::test]
async fn closeness_same_domain_different_names_stay_separate() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_schema_db")
        .to_string_lossy()
        .to_string();

    let state =
        SchemaServiceState::new(db_path).expect("failed to initialize schema service state");

    // First schema: holiday illustrations (image-like fields)
    let mut illustrations = json_to_schema(json!({
        "name": "holiday_illustrations",
        "descriptive_name": "Holiday Illustration Collection",
        "fields": ["source_file_name", "created_at", "image_type", "description", "name"]
    }));
    illustrations.field_descriptions.insert(
        "source_file_name".to_string(),
        "path to the image file".to_string(),
    );
    illustrations.field_descriptions.insert(
        "created_at".to_string(),
        "when the image was created".to_string(),
    );
    illustrations
        .field_descriptions
        .insert("image_type".to_string(), "type of image".to_string());
    illustrations.field_descriptions.insert(
        "description".to_string(),
        "description of the image content".to_string(),
    );
    illustrations
        .field_descriptions
        .insert("name".to_string(), "name of the artwork".to_string());

    let outcome = state
        .add_schema(illustrations, HashMap::new())
        .await
        .expect("failed to add illustrations schema");
    assert!(
        matches!(outcome, SchemaAddOutcome::Added(..)),
        "first schema should be Added, got {:?}",
        outcome
    );

    // Second schema: famous paintings (same fields, different name and descriptive name)
    let mut paintings = json_to_schema(json!({
        "name": "famous_paintings",
        "descriptive_name": "Famous Paintings Collection",
        "fields": ["source_file_name", "created_at", "image_type", "description", "name"]
    }));
    paintings.field_descriptions.insert(
        "source_file_name".to_string(),
        "path to the image file".to_string(),
    );
    paintings.field_descriptions.insert(
        "created_at".to_string(),
        "when the painting was created".to_string(),
    );
    paintings
        .field_descriptions
        .insert("image_type".to_string(), "type of image".to_string());
    paintings.field_descriptions.insert(
        "description".to_string(),
        "description of the painting".to_string(),
    );
    paintings
        .field_descriptions
        .insert("name".to_string(), "name of the painting".to_string());

    let outcome = state
        .add_schema(paintings, HashMap::new())
        .await
        .expect("failed to add paintings schema");

    // This MUST be Added (separate schema), NOT Expanded or AlreadyExists
    assert!(
        matches!(outcome, SchemaAddOutcome::Added(..)),
        "famous_paintings should be a SEPARATE schema from holiday_illustrations, got {:?}",
        outcome
    );

    // Verify both schemas exist independently (names are hashes now)
    let names = state.get_schema_names().expect("failed to list schemas");
    assert_eq!(
        names.len(),
        2,
        "should have exactly 2 schemas, got: {:?}",
        names
    );
}

/// Near-synonymous schema names with the same fields should still be kept
/// separate — it's safer to have two schemas than to wrongly merge different
/// collections. Only exact-name matches should merge.
#[tokio::test]
async fn closeness_near_synonymous_names_stay_separate() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_schema_db")
        .to_string_lossy()
        .to_string();

    let state =
        SchemaServiceState::new(db_path).expect("failed to initialize schema service state");

    let posts = json_to_schema(json!({
        "name": "blog_posts",
        "descriptive_name": "Blog Posts",
        "fields": ["title", "author", "content", "published_at"]
    }));

    let outcome = state
        .add_schema(posts, HashMap::new())
        .await
        .expect("failed to add blog_posts");
    assert!(matches!(outcome, SchemaAddOutcome::Added(..)));

    // "blog_articles" has a different name — stays separate even with same fields
    let articles = json_to_schema(json!({
        "name": "blog_articles",
        "descriptive_name": "Blog Articles",
        "fields": ["title", "author", "content", "published_at"]
    }));

    let outcome = state
        .add_schema(articles, HashMap::new())
        .await
        .expect("failed to add blog_articles");

    // Different schema names → separate schemas
    assert!(
        matches!(outcome, SchemaAddOutcome::Added(..)),
        "blog_articles should be separate from blog_posts (different names), got {:?}",
        outcome
    );

    let names = state.get_schema_names().expect("failed to list");
    assert_eq!(
        names.len(),
        2,
        "should have 2 separate schemas, got: {:?}",
        names
    );
}

/// Same schema name submitted twice with same fields → AlreadyExists.
#[tokio::test]
async fn closeness_exact_same_name_and_fields_merges() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_schema_db")
        .to_string_lossy()
        .to_string();

    let state =
        SchemaServiceState::new(db_path).expect("failed to initialize schema service state");

    let schema1 = json_to_schema(json!({
        "name": "blog_posts",
        "descriptive_name": "Blog Posts",
        "fields": ["title", "author", "content"]
    }));

    state
        .add_schema(schema1, HashMap::new())
        .await
        .expect("failed to add first schema");

    // Same name, same fields → should be AlreadyExists
    let schema2 = json_to_schema(json!({
        "name": "blog_posts",
        "descriptive_name": "Blog Posts",
        "fields": ["title", "author", "content"]
    }));

    let outcome = state
        .add_schema(schema2, HashMap::new())
        .await
        .expect("failed to add second schema");

    assert!(
        matches!(outcome, SchemaAddOutcome::AlreadyExists(..)),
        "same name + same fields should return AlreadyExists, got {:?}",
        outcome
    );
}
