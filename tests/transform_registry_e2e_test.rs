//! End-to-end test for the Global Transform Registry.
//!
//! Proves the full pipeline: register WASM transform → verify hash & classification
//! → create a view referencing it → insert data → query → verify output.
//!
//! The test uses the `medical_summary` WASM transform which genuinely downgrades
//! medical records: strips diagnosis/rx_list, buckets age, converts diagnosis to boolean.

#![cfg(feature = "transform-wasm")]

use fold_db::schema::types::data_classification::DataClassification;
use fold_db::schema::types::field_value_type::FieldValueType;
use fold_db::schema::types::key_value::KeyValue;
use fold_db::schema::types::operations::Query;
use fold_db::schema::types::schema::DeclarativeSchemaType as SchemaType;
use fold_db::schema_service::types::RegisterTransformRequest;
use fold_db::view::types::TransformView;
use fold_db::MutationType;
use fold_db_node::fold_node::config::NodeConfig;
use fold_db_node::fold_node::FoldNode;
use fold_db_node::fold_node::OperationProcessor;
use fold_db_node::schema_service::server::SchemaServiceState;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use tempfile::TempDir;

/// Load the pre-compiled medical_summary WASM fixture.
fn load_wasm_fixture() -> Vec<u8> {
    let fixture_path = std::env::current_dir()
        .expect("current dir")
        .join("tests/fixtures/medical_summary.wasm");
    std::fs::read(&fixture_path).unwrap_or_else(|e| {
        panic!(
            "Failed to read WASM fixture at {}: {}",
            fixture_path.display(),
            e
        )
    })
}

/// Build a medical_records schema for the schema service (global registry).
fn build_medical_records_schema_for_registry(
    classifications: HashMap<String, DataClassification>,
) -> fold_db::schema::types::Schema {
    let mut schema: fold_db::schema::types::Schema = serde_json::from_value(json!({
        "name": "medical_records",
        "descriptive_name": "Medical Records",
        "fields": ["name", "age", "diagnosis", "rx_list", "blood_type"],
        "field_classifications": {
            "name": ["low"],
            "age": ["low"],
            "diagnosis": ["high", "medical", "hipaa"],
            "rx_list": ["high", "medical"],
            "blood_type": ["low"]
        }
    }))
    .expect("deserialize schema");

    if schema.descriptive_name.is_none() {
        schema.descriptive_name = Some(schema.name.clone());
    }
    for f in schema.fields.clone().unwrap_or_default() {
        schema
            .field_descriptions
            .entry(f.clone())
            .or_insert_with(|| format!("{} field", f));
    }
    schema.field_data_classifications = classifications;
    schema
}

/// Create a FoldNode with a medical_records schema loaded.
async fn setup_node_with_medical_schema() -> (FoldNode, TempDir) {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_db_path = temp_dir.path().to_str().unwrap();

    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let config = NodeConfig::new(temp_db_path.into())
        .with_schema_service_url("test://mock")
        .with_identity(&keypair.public_key_base64(), &keypair.secret_key_base64());
    let node = FoldNode::new(config).await.expect("create FoldNode");

    // Load the medical_records schema into FoldDB
    let medical_schema_json = fold_db::test_helpers::TestSchemaBuilder::new("medical_records")
        .descriptive_name("Medical Records")
        .fields(&["age", "diagnosis", "rx_list", "blood_type"])
        .range_key("name")
        .classify("diagnosis", 3, "medical")
        .classify("rx_list", 3, "medical")
        .build_json();

    let fold_db = node.get_fold_db().expect("get FoldDB");
    fold_db
        .load_schema_from_json(&medical_schema_json)
        .await
        .expect("load medical_records schema");

    (node, temp_dir)
}

async fn insert_medical_record(
    processor: &OperationProcessor,
    name: &str,
    age: i64,
    diagnosis: &str,
    rx_list: &str,
    blood_type: &str,
) {
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), json!(name));
    fields.insert("age".to_string(), json!(age));
    fields.insert("diagnosis".to_string(), json!(diagnosis));
    fields.insert("rx_list".to_string(), json!(rx_list));
    fields.insert("blood_type".to_string(), json!(blood_type));

    processor
        .execute_mutation(
            "medical_records".to_string(),
            fields,
            KeyValue::new(None, Some(name.to_string())),
            MutationType::Create,
        )
        .await
        .expect("insert medical record");
}

#[tokio::test(flavor = "multi_thread")]
async fn transform_registry_e2e_medical_summary() {
    // ========================================================
    // Part 1: Global Transform Registry (SchemaServiceState)
    // ========================================================

    let wasm_bytes = load_wasm_fixture();
    assert!(wasm_bytes.len() > 4, "WASM fixture should be non-empty");
    assert_eq!(&wasm_bytes[..4], b"\0asm", "Should be valid WASM");

    // Step 1: Create a SchemaServiceState (global registry)
    let registry_dir = TempDir::new().expect("registry temp dir");
    let registry_path = registry_dir
        .path()
        .join("transform_registry")
        .to_string_lossy()
        .to_string();
    let schema_state = SchemaServiceState::new(registry_path).expect("create SchemaServiceState");

    // Step 2: Register the medical_records schema in the global registry
    // (needed so resolve_input_classifications can look up field classifications)
    let mut data_classifications = HashMap::new();
    data_classifications.insert(
        "name".to_string(),
        DataClassification::new(0, "general").unwrap(),
    );
    data_classifications.insert(
        "age".to_string(),
        DataClassification::new(0, "general").unwrap(),
    );
    data_classifications.insert(
        "diagnosis".to_string(),
        DataClassification::new(4, "medical").unwrap(),
    );
    data_classifications.insert(
        "rx_list".to_string(),
        DataClassification::new(4, "medical").unwrap(),
    );
    data_classifications.insert(
        "blood_type".to_string(),
        DataClassification::new(0, "general").unwrap(),
    );

    let schema = build_medical_records_schema_for_registry(data_classifications);
    schema_state
        .add_schema(schema, HashMap::new())
        .await
        .expect("register medical_records schema in global registry");

    // Step 3: Verify WASM hash = SHA256 of bytes
    let mut hasher = Sha256::new();
    hasher.update(&wasm_bytes);
    let expected_hash = format!("{:x}", hasher.finalize());

    let computed_hash = SchemaServiceState::compute_wasm_hash(&wasm_bytes);
    assert_eq!(
        computed_hash, expected_hash,
        "compute_wasm_hash should return SHA256 of WASM bytes"
    );

    // Step 4: Verify hash via verify_transform
    let (matches, verified_hash) =
        SchemaServiceState::verify_transform(&expected_hash, &wasm_bytes);
    assert!(matches, "verify_transform should confirm hash match");
    assert_eq!(verified_hash, expected_hash);

    // Step 5: Register the transform via the global registry
    let output_fields: HashMap<String, FieldValueType> = HashMap::from([
        ("patient_name".to_string(), FieldValueType::String),
        ("age_group".to_string(), FieldValueType::String),
        ("has_known_conditions".to_string(), FieldValueType::Boolean),
    ]);

    // The schema service stores schemas by identity_hash; the descriptive_name_index
    // maps "Medical Records" → identity_hash, so use the descriptive name in input queries.
    let request = RegisterTransformRequest {
        name: "medical_summary".to_string(),
        version: "1.0.0".to_string(),
        description: Some(
            "Downgrades medical records: strips diagnosis/rx_list, buckets age, boolean conditions"
                .to_string(),
        ),
        input_queries: vec![Query::new(
            "Medical Records".to_string(),
            vec![
                "name".to_string(),
                "age".to_string(),
                "diagnosis".to_string(),
                "rx_list".to_string(),
                "blood_type".to_string(),
            ],
        )],
        output_fields: output_fields.clone(),
        source_url: None,
        wasm_bytes: wasm_bytes.clone(),
    };

    let (record, outcome) = schema_state
        .register_transform(request)
        .await
        .expect("register transform");

    assert!(
        matches!(
            outcome,
            fold_db::schema_service::types::TransformAddOutcome::Added
        ),
        "transform should be newly added"
    );

    // Step 6: Verify the record hash matches
    assert_eq!(
        record.hash, expected_hash,
        "registered transform hash should match SHA256"
    );

    // Step 7: Verify Phase 1 classification = HIGH (max of input field classifications)
    // diagnosis and rx_list are tagged "high"/"medical" → maps to DataClassification::high()
    assert_eq!(
        record.input_ceiling,
        DataClassification::high(),
        "input_ceiling should be HIGH (from diagnosis/rx_list 'high' tags)"
    );

    // assigned_classification >= input_ceiling
    assert!(
        record.assigned_classification >= DataClassification::high(),
        "assigned_classification should be at least HIGH"
    );

    // Step 8: Verify the transform can be retrieved by hash
    let retrieved = schema_state
        .get_transform_by_hash(&expected_hash)
        .expect("get_transform_by_hash")
        .expect("transform should exist");
    assert_eq!(retrieved.name, "medical_summary");
    assert_eq!(retrieved.version, "1.0.0");

    // Verify WASM bytes are persisted and retrievable
    let stored_wasm = schema_state
        .get_transform_wasm(&expected_hash)
        .await
        .expect("get_transform_wasm")
        .expect("WASM bytes should be stored");
    assert_eq!(stored_wasm, wasm_bytes, "stored WASM should match original");

    // ========================================================
    // Part 2: FoldDB View (local database + query)
    // ========================================================

    // Step 9: Set up FoldDB with medical_records schema + test data
    let (node, _tmp) = setup_node_with_medical_schema().await;
    let processor = OperationProcessor::new(std::sync::Arc::new(node));

    insert_medical_record(
        &processor,
        "Alice Smith",
        34,
        "Type 2 Diabetes",
        "Metformin 500mg",
        "A+",
    )
    .await;
    insert_medical_record(&processor, "Bob Jones", 72, "", "", "O-").await;
    insert_medical_record(
        &processor,
        "Carol Lee",
        8,
        "Asthma",
        "Albuterol inhaler",
        "B+",
    )
    .await;

    // Step 10: Create a TransformView referencing the WASM transform
    let view = TransformView::new(
        "MedicalSummaryView",
        SchemaType::Single,
        None,
        vec![Query::new(
            "medical_records".to_string(),
            vec![
                "name".to_string(),
                "age".to_string(),
                "diagnosis".to_string(),
            ],
        )],
        Some(wasm_bytes),
        output_fields,
    );
    processor
        .create_view(view)
        .await
        .expect("register MedicalSummaryView");

    // Step 11: Query the view
    let query = Query::new(
        "MedicalSummaryView".to_string(),
        vec![
            "patient_name".to_string(),
            "age_group".to_string(),
            "has_known_conditions".to_string(),
        ],
    );
    let results = processor
        .execute_query_json(query)
        .await
        .expect("query MedicalSummaryView");

    // Step 12: Verify output contains ONLY the downgraded fields
    assert!(
        !results.is_empty(),
        "view query should return results, got empty"
    );

    for result in &results {
        let fields = result
            .get("fields")
            .and_then(|f| f.as_object())
            .expect("result should have fields object");

        // Verify output fields are present
        assert!(
            fields.contains_key("patient_name"),
            "should have patient_name field"
        );
        assert!(
            fields.contains_key("age_group"),
            "should have age_group field"
        );
        assert!(
            fields.contains_key("has_known_conditions"),
            "should have has_known_conditions field"
        );

        // Verify sensitive fields are NOT present
        assert!(
            !fields.contains_key("diagnosis"),
            "diagnosis should NOT be in output (stripped by transform)"
        );
        assert!(
            !fields.contains_key("rx_list"),
            "rx_list should NOT be in output (stripped by transform)"
        );
        assert!(
            !fields.contains_key("blood_type"),
            "blood_type should NOT be in output (not requested)"
        );

        // Verify age_group is a bucketed string, not a raw number
        let age_group = fields
            .get("age_group")
            .and_then(|v| v.as_str())
            .expect("age_group should be a string");
        let valid_buckets = ["0-17", "18-29", "30-39", "40-49", "50-64", "65+"];
        assert!(
            valid_buckets.contains(&age_group),
            "age_group '{}' should be a valid bucket",
            age_group
        );

        // Verify has_known_conditions is a boolean
        assert!(
            fields
                .get("has_known_conditions")
                .map(|v| v.is_boolean())
                .unwrap_or(false),
            "has_known_conditions should be a boolean"
        );
    }

    // Step 13: Verify classification on the transform record
    assert_eq!(
        record.assigned_classification,
        std::cmp::max(
            record.input_ceiling.clone(),
            record.output_classification.clone()
        ),
        "assigned_classification = max(input_ceiling, output_classification)"
    );
}

/// Verify idempotent re-registration returns AlreadyExists.
#[tokio::test(flavor = "multi_thread")]
async fn transform_registry_idempotent_reregistration() {
    let wasm_bytes = load_wasm_fixture();

    let registry_dir = TempDir::new().expect("registry temp dir");
    let registry_path = registry_dir
        .path()
        .join("idempotent_test")
        .to_string_lossy()
        .to_string();
    let state = SchemaServiceState::new(registry_path).expect("create state");

    // Register medical_records schema first (all fields need classifications)
    let mut data_classifications = HashMap::new();
    data_classifications.insert(
        "name".to_string(),
        DataClassification::new(0, "general").unwrap(),
    );
    data_classifications.insert(
        "age".to_string(),
        DataClassification::new(0, "general").unwrap(),
    );
    data_classifications.insert(
        "diagnosis".to_string(),
        DataClassification::new(4, "medical").unwrap(),
    );
    data_classifications.insert(
        "rx_list".to_string(),
        DataClassification::new(4, "medical").unwrap(),
    );
    data_classifications.insert(
        "blood_type".to_string(),
        DataClassification::new(0, "general").unwrap(),
    );
    let schema = build_medical_records_schema_for_registry(data_classifications);
    state
        .add_schema(schema, HashMap::new())
        .await
        .expect("register schema");

    let make_request = || RegisterTransformRequest {
        name: "medical_summary".to_string(),
        version: "1.0.0".to_string(),
        description: None,
        input_queries: vec![Query::new(
            "Medical Records".to_string(),
            vec![
                "name".to_string(),
                "age".to_string(),
                "diagnosis".to_string(),
            ],
        )],
        output_fields: HashMap::from([
            ("patient_name".to_string(), FieldValueType::String),
            ("age_group".to_string(), FieldValueType::String),
            ("has_known_conditions".to_string(), FieldValueType::Boolean),
        ]),
        source_url: None,
        wasm_bytes: wasm_bytes.clone(),
    };

    // First registration
    let (record1, outcome1) = state
        .register_transform(make_request())
        .await
        .expect("first registration");
    assert!(matches!(
        outcome1,
        fold_db::schema_service::types::TransformAddOutcome::Added
    ));

    // Second registration — same WASM bytes → idempotent
    let (record2, outcome2) = state
        .register_transform(make_request())
        .await
        .expect("second registration");
    assert!(matches!(
        outcome2,
        fold_db::schema_service::types::TransformAddOutcome::AlreadyExists
    ));
    assert_eq!(record1.hash, record2.hash, "hash should be identical");
}
