//! Integration test: verify array fields survive the full write → query round-trip.
//!
//! Reproduces BUG-004 from docs/dogfood_vacation_findings.md:
//! Array fields (must_see, avoid, interests, dietary_restrictions) were ingested
//! but not queryable by the agent.

use fold_db::schema::types::key_value::KeyValue;
use fold_db::schema::types::operations::Query;
use fold_db::MutationType;
use fold_db_node::fold_node::config::NodeConfig;
use fold_db_node::fold_node::FoldNode;
use fold_db_node::fold_node::OperationProcessor;
use serde_json::json;
use std::collections::HashMap;
use tempfile::TempDir;

async fn setup_node() -> (FoldNode, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let temp_db_path = temp_dir.path().to_str().unwrap();

    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let config = NodeConfig::new(temp_db_path.into())
        .with_schema_service_url("test://mock")
        .with_identity(&keypair.public_key_base64(), &keypair.secret_key_base64());
    let node = FoldNode::new(config)
        .await
        .expect("Failed to create FoldNode");

    (node, temp_dir)
}

async fn load_schema(node: &FoldNode, schema_filename: &str) {
    let schema_path = std::env::current_dir()
        .expect("Failed to get current directory")
        .join("tests/schemas_for_testing")
        .join(schema_filename);

    let mut fold_db = node.get_fold_db().await.expect("Failed to get FoldDB");
    fold_db
        .load_schema_from_file(&schema_path)
        .await
        .expect("Failed to load schema");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_array_fields_survive_roundtrip() {
    let (node, _tmp) = setup_node().await;
    load_schema(&node, "TravelPreferences.json").await;
    let processor = OperationProcessor::new(node);

    // Write a record with array fields
    let mut fields = HashMap::new();
    fields.insert("traveler".to_string(), json!("Alice"));
    fields.insert("destination".to_string(), json!("Tokyo"));
    fields.insert(
        "must_see".to_string(),
        json!(["Meiji Shrine", "Shibuya Crossing", "Tsukiji Market"]),
    );
    fields.insert("avoid".to_string(), json!(["tourist traps", "rush hour"]));
    fields.insert(
        "interests".to_string(),
        json!(["hiking", "street food", "temples"]),
    );
    fields.insert("dietary_restrictions".to_string(), json!(["vegetarian"]));
    fields.insert("budget".to_string(), json!(3000));

    processor
        .execute_mutation(
            "TravelPreferences".to_string(),
            fields,
            KeyValue::new(Some("Alice".to_string()), Some("Tokyo".to_string())),
            MutationType::Create,
        )
        .await
        .expect("Failed to execute mutation");

    // Query all fields back
    let query = Query {
        schema_name: "TravelPreferences".to_string(),
        fields: vec![
            "traveler".to_string(),
            "destination".to_string(),
            "must_see".to_string(),
            "avoid".to_string(),
            "interests".to_string(),
            "dietary_restrictions".to_string(),
            "budget".to_string(),
        ],
        filter: None,
        as_of: None,
        rehydrate_depth: None,
        sort_order: None,
    };

    let results = processor
        .execute_query_json(query)
        .await
        .expect("Query failed");

    assert_eq!(results.len(), 1, "Expected exactly 1 record");
    let record = &results[0];
    let fields = record.get("fields").expect("Record missing 'fields'");

    // Scalar fields
    assert_eq!(fields["traveler"], json!("Alice"));
    assert_eq!(fields["destination"], json!("Tokyo"));
    assert_eq!(fields["budget"], json!(3000));

    // Array fields — the core of BUG-004
    let must_see = fields.get("must_see").expect("must_see field missing");
    assert!(
        must_see.is_array(),
        "must_see should be an array, got: {:?}",
        must_see
    );
    assert_eq!(
        must_see,
        &json!(["Meiji Shrine", "Shibuya Crossing", "Tsukiji Market"])
    );

    let avoid = fields.get("avoid").expect("avoid field missing");
    assert!(
        avoid.is_array(),
        "avoid should be an array, got: {:?}",
        avoid
    );
    assert_eq!(avoid, &json!(["tourist traps", "rush hour"]));

    let interests = fields.get("interests").expect("interests field missing");
    assert!(
        interests.is_array(),
        "interests should be an array, got: {:?}",
        interests
    );
    assert_eq!(interests, &json!(["hiking", "street food", "temples"]));

    let dietary = fields
        .get("dietary_restrictions")
        .expect("dietary_restrictions field missing");
    assert!(
        dietary.is_array(),
        "dietary_restrictions should be an array, got: {:?}",
        dietary
    );
    assert_eq!(dietary, &json!(["vegetarian"]));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_empty_array_fields_survive_roundtrip() {
    let (node, _tmp) = setup_node().await;
    load_schema(&node, "TravelPreferences.json").await;
    let processor = OperationProcessor::new(node);

    let mut fields = HashMap::new();
    fields.insert("traveler".to_string(), json!("Bob"));
    fields.insert("destination".to_string(), json!("Paris"));
    fields.insert("must_see".to_string(), json!([]));
    fields.insert("avoid".to_string(), json!([]));
    fields.insert("interests".to_string(), json!(["art"]));
    fields.insert("dietary_restrictions".to_string(), json!([]));
    fields.insert("budget".to_string(), json!(5000));

    processor
        .execute_mutation(
            "TravelPreferences".to_string(),
            fields,
            KeyValue::new(Some("Bob".to_string()), Some("Paris".to_string())),
            MutationType::Create,
        )
        .await
        .expect("Failed to execute mutation");

    let query = Query {
        schema_name: "TravelPreferences".to_string(),
        fields: vec![
            "must_see".to_string(),
            "avoid".to_string(),
            "interests".to_string(),
        ],
        filter: None,
        as_of: None,
        rehydrate_depth: None,
        sort_order: None,
    };

    let results = processor
        .execute_query_json(query)
        .await
        .expect("Query failed");

    assert_eq!(results.len(), 1);
    let fields = results[0].get("fields").expect("Record missing 'fields'");

    // Empty arrays should survive, not become null
    assert_eq!(fields["must_see"], json!([]));
    assert_eq!(fields["avoid"], json!([]));
    assert_eq!(fields["interests"], json!(["art"]));
}

/// Test that the mutation_mapper backfill logic correctly handles the case
/// where AI omits array fields from mutation_mappers.
#[test]
fn test_backfill_fixes_missing_array_mappers_in_ai_response() {
    use fold_db_node::ingestion::ai::helpers::parse_ai_response;

    // Simulate AI response that declares array fields in schema but omits them from mappers
    let ai_json = r#"{
        "new_schemas": {
            "name": "vacation_prefs",
            "descriptive_name": "Vacation Preferences",
            "schema_type": "HashRange",
            "key": {"hash_field": "traveler", "range_field": "destination"},
            "fields": ["traveler", "destination", "must_see", "avoid", "interests"],
            "field_descriptions": {
                "traveler": "Traveler name",
                "destination": "Travel destination",
                "must_see": "Must-see attractions",
                "avoid": "Places to avoid",
                "interests": "Traveler interests"
            }
        },
        "mutation_mappers": {
            "traveler": "vacation_prefs.traveler",
            "destination": "vacation_prefs.destination"
        }
    }"#;

    let result = parse_ai_response(ai_json).expect("Should parse successfully");

    // All 5 fields should have mappers (2 from AI + 3 backfilled)
    assert_eq!(
        result.mutation_mappers.len(),
        5,
        "Expected 5 mappers, got: {:?}",
        result.mutation_mappers
    );
    assert!(result.mutation_mappers.contains_key("must_see"));
    assert!(result.mutation_mappers.contains_key("avoid"));
    assert!(result.mutation_mappers.contains_key("interests"));
}
