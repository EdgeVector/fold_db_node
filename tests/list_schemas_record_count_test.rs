//! Integration test for record-count primitive used by the LLM agent's
//! `list_schemas` tool.
//!
//! The agent's inventory question ("how much data do I have?") relied on
//! issuing an unfiltered query per schema — when iteration budget ran low
//! it would skip the query and report "Unknown count". This test pins the
//! cheap counting path that backs the new `record_count` field.
//!
//! Run: `cargo test --test list_schemas_record_count_test -- --nocapture`

mod common;

use fold_db::schema::types::{KeyValue, Mutation, MutationType};
use fold_db_node::fold_node::node::FoldNode;
use fold_db_node::fold_node::OperationProcessor;
use serde_json::json;
use std::collections::HashMap;

fn note_schema_json(name: &str) -> serde_json::Value {
    json!({
        "name": name,
        "schema_type": "Hash",
        "key": { "hash_field": "id" },
        "fields": ["id", "title"],
        "field_classifications": {
            "id": ["word"],
            "title": ["word"]
        },
        "field_descriptions": {
            "id": "Note id",
            "title": "Note title"
        },
        "field_data_classifications": {
            "id": { "sensitivity_level": 0, "data_domain": "general" },
            "title": { "sensitivity_level": 0, "data_domain": "general" }
        },
        "permissions": {
            "read": { "policy_type": "NoPolicy" },
            "write": { "policy_type": "NoPolicy" }
        }
    })
}

fn note_mutation(schema_name: &str, id: &str, title: &str, pub_key: &str) -> Mutation {
    let mut fields = HashMap::new();
    fields.insert("id".to_string(), json!(id));
    fields.insert("title".to_string(), json!(title));
    Mutation::new(
        schema_name.to_string(),
        fields,
        KeyValue::new(Some(id.to_string()), None),
        pub_key.to_string(),
        MutationType::Create,
    )
}

#[actix_web::test]
async fn test_count_schema_records_reflects_inserted_rows() {
    let schema_name = "test_count_notes";

    // Setup node
    let mut config = common::create_test_node_config();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let pub_key = keypair.public_key_base64();
    config = config.with_seed_identity(fold_db_node::identity::identity_from_keypair(&keypair));
    config = config.with_schema_service_url("test://mock");
    let node = FoldNode::new(config).await.unwrap();

    // Load + approve schema
    {
        let db = node.get_fold_db().unwrap();
        let json_str = serde_json::to_string(&note_schema_json(schema_name)).unwrap();
        db.schema_manager()
            .load_schema_from_json(&json_str)
            .await
            .expect("Failed to load schema");
        db.schema_manager()
            .approve(schema_name)
            .await
            .expect("Failed to approve schema");
    }

    let processor = OperationProcessor::new(std::sync::Arc::new(node.clone()));

    // Empty schema → count is 0.
    let count = processor.count_schema_records(schema_name).await.unwrap();
    assert_eq!(count, 0, "Empty schema should report 0 records");

    // Insert 3 records.
    let mutations = vec![
        note_mutation(schema_name, "n1", "First", &pub_key),
        note_mutation(schema_name, "n2", "Second", &pub_key),
        note_mutation(schema_name, "n3", "Third", &pub_key),
    ];
    node.mutate_batch(mutations)
        .await
        .expect("mutate_batch failed");

    // Count must equal the number of inserted records.
    let count = processor.count_schema_records(schema_name).await.unwrap();
    assert_eq!(
        count, 3,
        "After 3 inserts, count_schema_records must return 3"
    );
}

#[actix_web::test]
async fn test_count_schema_records_unknown_schema_returns_zero() {
    // Setup node — no schemas loaded.
    let mut config = common::create_test_node_config();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    config = config.with_seed_identity(fold_db_node::identity::identity_from_keypair(&keypair));
    config = config.with_schema_service_url("test://mock");
    let node = FoldNode::new(config).await.unwrap();

    let processor = OperationProcessor::new(std::sync::Arc::new(node.clone()));

    // Unknown schema must not error — returning 0 lets the LLM `list_schemas`
    // tool degrade gracefully on a single uninitialized entry instead of
    // failing the whole inventory call.
    let count = processor
        .count_schema_records("does_not_exist")
        .await
        .unwrap();
    assert_eq!(count, 0);
}
