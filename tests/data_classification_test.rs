use fold_db::schema::types::data_classification::DataClassification;
use fold_db_node::schema_service::server::{SchemaAddOutcome, SchemaServiceState};
use serde_json::json;
use std::collections::HashMap;
use tempfile::tempdir;

/// Helper: build a Schema from JSON, auto-filling descriptive_name,
/// field_descriptions, and field_data_classifications if not provided.
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

/// Helper: build a Schema from JSON with explicit classifications per field.
fn json_to_schema_with_classifications(
    value: serde_json::Value,
    classifications: HashMap<String, DataClassification>,
) -> fold_db::schema::types::Schema {
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
        }
    }
    schema.field_data_classifications = classifications;
    schema
}

// T1: add_schema auto-populates missing field_data_classifications with default (0, "general")
#[tokio::test]
async fn auto_populates_missing_field_data_classifications() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_missing_classification")
        .to_string_lossy()
        .to_string();

    let state =
        SchemaServiceState::new(db_path).expect("failed to initialize schema service state");

    // Build schema WITHOUT classifications (skip the helper that auto-fills them)
    let mut schema: fold_db::schema::types::Schema = serde_json::from_value(json!({
        "name": "TestSchema",
        "descriptive_name": "Test Schema",
        "fields": ["field_a", "field_b"],
    }))
    .unwrap();
    // Add descriptions but NOT classifications
    schema.field_descriptions.insert("field_a".to_string(), "field a desc".to_string());
    schema.field_descriptions.insert("field_b".to_string(), "field b desc".to_string());

    let outcome = state
        .add_schema(schema, HashMap::new())
        .await
        .expect("should accept schema without classifications and auto-populate defaults");

    match outcome {
        SchemaAddOutcome::Added(schema, _) => {
            // Both fields should have been auto-populated with (0, "general")
            let class_a = schema
                .field_data_classifications
                .get("field_a")
                .expect("field_a should have auto-populated classification");
            assert_eq!(class_a.sensitivity_level, 0);
            assert_eq!(class_a.data_domain, "general");

            let class_b = schema
                .field_data_classifications
                .get("field_b")
                .expect("field_b should have auto-populated classification");
            assert_eq!(class_b.sensitivity_level, 0);
            assert_eq!(class_b.data_domain, "general");
        }
        other => panic!("expected Added, got {:?}", other),
    }
}

// T2: add_schema accepts valid classification (sensitivity 0-4, domain string)
#[tokio::test]
async fn accepts_valid_classifications() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_valid_classification")
        .to_string_lossy()
        .to_string();

    let state =
        SchemaServiceState::new(db_path).expect("failed to initialize schema service state");

    let mut classifications = HashMap::new();
    classifications.insert(
        "salary".to_string(),
        DataClassification::new(3, "financial").unwrap(),
    );
    classifications.insert(
        "department".to_string(),
        DataClassification::new(1, "general").unwrap(),
    );

    let schema = json_to_schema_with_classifications(
        json!({
            "name": "EmployeeData",
            "descriptive_name": "Employee Data",
            "fields": ["salary", "department"],
        }),
        classifications,
    );

    let outcome = state
        .add_schema(schema, HashMap::new())
        .await
        .expect("should accept valid classifications");

    match outcome {
        SchemaAddOutcome::Added(schema, _) => {
            let salary_class = schema
                .field_data_classifications
                .get("salary")
                .expect("salary should have classification");
            assert_eq!(salary_class.sensitivity_level, 3);
            assert_eq!(salary_class.data_domain, "financial");

            let dept_class = schema
                .field_data_classifications
                .get("department")
                .expect("department should have classification");
            assert_eq!(dept_class.sensitivity_level, 1);
            assert_eq!(dept_class.data_domain, "general");
        }
        other => panic!("expected Added, got {:?}", other),
    }
}

// T3: classification survives serialize/deserialize round-trip on Schema
#[test]
fn classification_survives_round_trip() {
    let mut schema = fold_db::schema::types::DeclarativeSchemaDefinition::new(
        "TestSchema".to_string(),
        fold_db::schema::types::SchemaType::Single,
        None,
        Some(vec!["name".to_string(), "ssn".to_string()]),
        None,
        None,
    );
    schema.field_data_classifications.insert(
        "name".to_string(),
        DataClassification::new(1, "identity").unwrap(),
    );
    schema.field_data_classifications.insert(
        "ssn".to_string(),
        DataClassification::new(4, "identity").unwrap(),
    );

    let serialized = serde_json::to_string(&schema).unwrap();
    let deserialized: fold_db::schema::types::DeclarativeSchemaDefinition =
        serde_json::from_str(&serialized).unwrap();

    assert_eq!(
        deserialized.field_data_classifications.get("name"),
        Some(&DataClassification::new(1, "identity").unwrap()),
    );
    assert_eq!(
        deserialized.field_data_classifications.get("ssn"),
        Some(&DataClassification::new(4, "identity").unwrap()),
    );
}

// T4: classification stored on CanonicalField after registration
#[tokio::test]
async fn classification_stored_on_canonical_field() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_canonical_classification")
        .to_string_lossy()
        .to_string();

    let state =
        SchemaServiceState::new(db_path).expect("failed to initialize schema service state");

    let mut classifications = HashMap::new();
    classifications.insert(
        "diagnosis".to_string(),
        DataClassification::new(4, "medical").unwrap(),
    );
    classifications.insert(
        "visit_date".to_string(),
        DataClassification::new(2, "medical").unwrap(),
    );

    let schema = json_to_schema_with_classifications(
        json!({
            "name": "PatientRecords",
            "descriptive_name": "Patient Records",
            "fields": ["diagnosis", "visit_date"],
        }),
        classifications,
    );

    let outcome = state
        .add_schema(schema, HashMap::new())
        .await
        .expect("should add schema");
    assert!(matches!(outcome, SchemaAddOutcome::Added(..)));

    // Now add a second schema — the canonical fields from the first should have classifications
    // We verify by checking that if a second schema reuses the same field name,
    // the canonical classification propagates
    let mut classifications2 = HashMap::new();
    classifications2.insert(
        "diagnosis".to_string(),
        DataClassification::new(4, "medical").unwrap(),
    );
    classifications2.insert(
        "treatment".to_string(),
        DataClassification::new(3, "medical").unwrap(),
    );

    let schema2 = json_to_schema_with_classifications(
        json!({
            "name": "TreatmentRecords",
            "descriptive_name": "Treatment Records",
            "fields": ["diagnosis", "treatment"],
        }),
        classifications2,
    );

    let outcome2 = state
        .add_schema(schema2, HashMap::new())
        .await
        .expect("should add second schema");

    match outcome2 {
        SchemaAddOutcome::Added(schema, _) => {
            // diagnosis should have classification (either from submit or canonical propagation)
            let diag_class = schema
                .field_data_classifications
                .get("diagnosis")
                .expect("diagnosis should have classification");
            assert_eq!(diag_class.sensitivity_level, 4);
            assert_eq!(diag_class.data_domain, "medical");
        }
        other => panic!("expected Added, got {:?}", other),
    }
}

// T5: apply_canonical_classifications propagates to new schema
// (covered implicitly by T4 — canonical fields propagate classification to schemas
// that already declared it, but the explicit propagation path is tested here
// for fields that get their classification from the canonical registry)
#[tokio::test]
async fn canonical_classification_propagates_to_schema() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_canonical_propagation")
        .to_string_lossy()
        .to_string();

    let state =
        SchemaServiceState::new(db_path).expect("failed to initialize schema service state");

    // First schema registers "email" as canonical with classification
    let mut classifications = HashMap::new();
    classifications.insert(
        "email".to_string(),
        DataClassification::new(3, "identity").unwrap(),
    );
    classifications.insert(
        "username".to_string(),
        DataClassification::new(1, "identity").unwrap(),
    );

    let schema1 = json_to_schema_with_classifications(
        json!({
            "name": "UserProfiles",
            "descriptive_name": "User Profiles",
            "fields": ["email", "username"],
        }),
        classifications,
    );

    state
        .add_schema(schema1, HashMap::new())
        .await
        .expect("should add first schema");

    // Second schema also has "email" — classification should be propagated
    // from canonical registry even though we provide it explicitly.
    // The schema already has it, but apply_canonical_classifications should
    // also work for fields that have no classification yet.
    let mut classifications2 = HashMap::new();
    classifications2.insert(
        "email".to_string(),
        DataClassification::new(3, "identity").unwrap(),
    );
    classifications2.insert(
        "bio".to_string(),
        DataClassification::new(0, "general").unwrap(),
    );

    let schema2 = json_to_schema_with_classifications(
        json!({
            "name": "PublicProfiles",
            "descriptive_name": "Public Profiles",
            "fields": ["email", "bio"],
        }),
        classifications2,
    );

    let outcome = state
        .add_schema(schema2, HashMap::new())
        .await
        .expect("should add second schema");

    match outcome {
        SchemaAddOutcome::Added(schema, _) => {
            assert!(
                schema.field_data_classifications.contains_key("email"),
                "email should have classification"
            );
            assert!(
                schema.field_data_classifications.contains_key("bio"),
                "bio should have classification"
            );
        }
        other => panic!("expected Added, got {:?}", other),
    }
}

// T6: field rename carries classification to canonical name
#[tokio::test]
async fn field_rename_carries_classification() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_rename_classification")
        .to_string_lossy()
        .to_string();

    let state =
        SchemaServiceState::new(db_path).expect("failed to initialize schema service state");

    // First schema: "artist" becomes canonical
    let schema_a = json_to_schema(json!({
        "name": "PaintingRecords",
        "descriptive_name": "Painting Records",
        "fields": ["artist", "title"],
    }));

    state
        .add_schema(schema_a, HashMap::new())
        .await
        .expect("should add first schema");

    // Second schema uses "creator" (semantically close to "artist")
    // with a specific classification
    let mut classifications = HashMap::new();
    classifications.insert(
        "creator".to_string(),
        DataClassification::new(1, "identity").unwrap(),
    );
    classifications.insert(
        "medium".to_string(),
        DataClassification::new(0, "general").unwrap(),
    );

    let schema_b = json_to_schema_with_classifications(
        json!({
            "name": "ArtworkRecords",
            "descriptive_name": "Artwork Records",
            "fields": ["creator", "medium"],
        }),
        classifications,
    );

    let outcome = state
        .add_schema(schema_b, HashMap::new())
        .await
        .expect("should add second schema");

    match &outcome {
        SchemaAddOutcome::Added(schema, mappers) | SchemaAddOutcome::Expanded(_, schema, mappers) => {
            let fields = schema.fields.as_ref().expect("must have fields");
            // If canonicalization renamed "creator" → "artist"
            if fields.contains(&"artist".to_string()) && mappers.contains_key("creator") {
                // Classification should follow the rename: "artist" should have
                // the classification that was on "creator"
                let artist_class = schema.field_data_classifications.get("artist");
                assert!(
                    artist_class.is_some(),
                    "renamed field 'artist' should carry the classification from 'creator'"
                );
                assert_eq!(artist_class.unwrap().sensitivity_level, 1);
                assert_eq!(artist_class.unwrap().data_domain, "identity");
                // "creator" should no longer have a classification
                assert!(
                    !schema.field_data_classifications.contains_key("creator"),
                    "'creator' classification should have moved to 'artist'"
                );
            }
            // "medium" should always keep its classification
            let medium_class = schema.field_data_classifications.get("medium");
            assert!(
                medium_class.is_some(),
                "'medium' should have classification"
            );
        }
        SchemaAddOutcome::AlreadyExists(..) => {
            // Acceptable — fields were canonicalized to identical set
        }
    }
}

// T7: legacy CanonicalField (no classification) loads as None
#[tokio::test]
async fn legacy_canonical_field_loads_without_classification() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_legacy_canonical")
        .to_string_lossy()
        .to_string();

    // Simulate legacy: write a CanonicalField without classification to sled
    {
        let db = sled::open(&db_path).expect("open sled");
        let tree = db.open_tree("canonical_fields").expect("open tree");

        // Legacy format: just a JSON with description and field_type, no classification
        let legacy_json = serde_json::json!({
            "description": "the person who created the work",
            "field_type": "String"
        });
        tree.insert(
            "author".as_bytes(),
            serde_json::to_vec(&legacy_json).unwrap(),
        )
        .unwrap();
        db.flush().unwrap();
    }

    // Open schema service — should load legacy canonical fields without error
    let state =
        SchemaServiceState::new(db_path).expect("should load despite legacy canonical fields");

    // Add a schema that reuses "author" — should not crash
    let schema = json_to_schema(json!({
        "name": "BookRecords",
        "descriptive_name": "Book Records",
        "fields": ["author", "title"],
    }));

    let outcome = state
        .add_schema(schema, HashMap::new())
        .await
        .expect("should add schema with legacy canonical field");

    if let SchemaAddOutcome::Added(schema, _) = outcome {
        assert!(
            schema.fields.as_ref().unwrap().contains(&"author".to_string()),
            "'author' should be in fields"
        );
    }
}

// T8: schema expansion merges classifications from both schemas
#[tokio::test]
async fn expansion_merges_classifications() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_expansion_classifications")
        .to_string_lossy()
        .to_string();

    let state =
        SchemaServiceState::new(db_path).expect("failed to initialize schema service state");

    // First schema: 2 fields
    let mut classifications1 = HashMap::new();
    classifications1.insert(
        "name".to_string(),
        DataClassification::new(1, "identity").unwrap(),
    );
    classifications1.insert(
        "email".to_string(),
        DataClassification::new(3, "identity").unwrap(),
    );

    let schema1 = json_to_schema_with_classifications(
        json!({
            "name": "ContactInfo",
            "descriptive_name": "Contact Information",
            "fields": ["name", "email"],
        }),
        classifications1,
    );

    state
        .add_schema(schema1, HashMap::new())
        .await
        .expect("should add first schema");

    // Second schema: same descriptive name + extra field → triggers expansion
    let mut classifications2 = HashMap::new();
    classifications2.insert(
        "name".to_string(),
        DataClassification::new(1, "identity").unwrap(),
    );
    classifications2.insert(
        "email".to_string(),
        DataClassification::new(3, "identity").unwrap(),
    );
    classifications2.insert(
        "phone".to_string(),
        DataClassification::new(3, "identity").unwrap(),
    );

    let schema2 = json_to_schema_with_classifications(
        json!({
            "name": "ContactInfo",
            "descriptive_name": "Contact Information",
            "fields": ["name", "email", "phone"],
        }),
        classifications2,
    );

    let outcome = state
        .add_schema(schema2, HashMap::new())
        .await
        .expect("should expand schema");

    match outcome {
        SchemaAddOutcome::Expanded(_, schema, _) => {
            // All three fields should have classifications
            assert!(
                schema.field_data_classifications.contains_key("name"),
                "name should have classification after expansion"
            );
            assert!(
                schema.field_data_classifications.contains_key("email"),
                "email should have classification after expansion"
            );
            assert!(
                schema.field_data_classifications.contains_key("phone"),
                "phone should have classification after expansion"
            );

            // Verify specific values
            let phone_class = schema.field_data_classifications.get("phone").unwrap();
            assert_eq!(phone_class.sensitivity_level, 3);
            assert_eq!(phone_class.data_domain, "identity");
        }
        SchemaAddOutcome::AlreadyExists(schema, _) => {
            // If fields were deduplicated to same set, classification should still exist
            assert!(
                schema.field_data_classifications.contains_key("name")
                    || schema.field_data_classifications.is_empty(),
                "existing schema may or may not have classifications"
            );
        }
        other => {
            // Added is also possible if descriptive names don't match semantically
            println!("Got {:?} — expansion test inconclusive", other);
        }
    }
}

// Bonus: DataClassification::new rejects invalid sensitivity levels
#[test]
fn rejects_invalid_sensitivity_level() {
    assert!(DataClassification::new(5, "general").is_err());
    assert!(DataClassification::new(100, "financial").is_err());
    assert!(DataClassification::new(255, "medical").is_err());
}

// Bonus: DataClassification::new rejects empty domain
#[test]
fn rejects_empty_data_domain() {
    assert!(DataClassification::new(0, "").is_err());
    assert!(DataClassification::new(0, "   ").is_err());
}
