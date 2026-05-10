//! `/api/query` and `/api/mutation` accept either the canonical schema
//! hash or the schema's `descriptive_name`.
//!
//! Background: the schema service rewrites every schema's `name` to an
//! identity hash and preserves the human-readable label in
//! `descriptive_name`. UI, humans, and the AI agent naturally use the
//! descriptive label, so handlers must resolve it before handing the
//! query off to fold_db (whose query executor only knows canonical
//! names).

use fold_db::schema::types::operations::{Mutation, Query};
use fold_db::schema::types::SchemaError;
use fold_db::schema::SchemaState;
use fold_db::test_helpers::TestSchemaBuilder;
use fold_db::MutationType;
use fold_db_node::fold_node::config::NodeConfig;
use fold_db_node::fold_node::FoldNode;
use fold_db_node::handlers::{mutation as mutation_handlers, query as query_handlers};
use serde_json::json;
use std::collections::HashMap;
use tempfile::TempDir;

/// Synthetic canonical name (mimics what the schema service emits — a
/// content-hash that no human would type).
const CANONICAL_NAME: &str = "76a65df700112233445566778899aabbccddeeff00112233445566778899aabb";
const DESCRIPTIVE_NAME: &str = "Apple Reminders";

async fn create_node(db_path: &str) -> FoldNode {
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let config = NodeConfig::new(db_path.into())
        .with_schema_service_url("test://mock")
        .with_seed_identity(fold_db_node::identity::identity_from_keypair(&keypair));
    FoldNode::new(config).await.expect("create FoldNode")
}

async fn load_descriptive_schema(node: &FoldNode, canonical: &str, descriptive: &str) {
    let schema_json = TestSchemaBuilder::new(canonical)
        .descriptive_name(descriptive)
        .fields(&["name", "completed"])
        .range_key("name")
        .build_json();
    let fold_db = node.get_fold_db().expect("get fold_db");
    fold_db
        .schema_manager()
        .load_schema_from_json(&schema_json)
        .await
        .expect("load schema");
    fold_db
        .schema_manager()
        .set_schema_state(canonical, SchemaState::Approved)
        .await
        .expect("approve schema");
}

async fn write_record(node: &FoldNode, schema: &str, name: &str, completed: bool) {
    let mut fields_and_values: HashMap<String, serde_json::Value> = HashMap::new();
    fields_and_values.insert("name".to_string(), json!(name));
    fields_and_values.insert("completed".to_string(), json!(completed));
    let key_value = fold_db::schema::types::key_value::KeyValue::new(None, Some(name.to_string()));
    let mutation = Mutation::new(
        schema.to_string(),
        fields_and_values,
        key_value,
        node.get_node_public_key().to_string(),
        MutationType::Create,
    );
    mutation_handlers::execute_mutation(mutation, "test-user", node)
        .await
        .expect("write record");
}

async fn run_query(node: &FoldNode, schema_name: &str) -> serde_json::Value {
    let query = Query::new(
        schema_name.to_string(),
        vec!["name".to_string(), "completed".to_string()],
    );
    let response = query_handlers::execute_query(query, None, None, "test-user", node)
        .await
        .expect("execute query");
    response.data.expect("query response data").results
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn query_accepts_canonical_hash() {
    let temp_dir = TempDir::new().expect("temp dir");
    let node = create_node(temp_dir.path().to_str().unwrap()).await;
    load_descriptive_schema(&node, CANONICAL_NAME, DESCRIPTIVE_NAME).await;

    write_record(&node, CANONICAL_NAME, "Buy milk", false).await;
    write_record(&node, CANONICAL_NAME, "Walk dog", true).await;

    let results = run_query(&node, CANONICAL_NAME).await;
    let arr = results.as_array().expect("results array");
    assert_eq!(arr.len(), 2, "canonical hash should return both records");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn query_accepts_descriptive_name() {
    let temp_dir = TempDir::new().expect("temp dir");
    let node = create_node(temp_dir.path().to_str().unwrap()).await;
    load_descriptive_schema(&node, CANONICAL_NAME, DESCRIPTIVE_NAME).await;

    write_record(&node, CANONICAL_NAME, "Buy milk", false).await;
    write_record(&node, CANONICAL_NAME, "Walk dog", true).await;

    let mut canonical_keys = sorted_keys(&run_query(&node, CANONICAL_NAME).await);
    let mut descriptive_keys = sorted_keys(&run_query(&node, DESCRIPTIVE_NAME).await);
    canonical_keys.sort();
    descriptive_keys.sort();
    assert_eq!(
        canonical_keys, descriptive_keys,
        "descriptive_name lookup must yield the same record set as the canonical hash"
    );
    assert_eq!(canonical_keys.len(), 2);
}

fn sorted_keys(results: &serde_json::Value) -> Vec<String> {
    results
        .as_array()
        .expect("results array")
        .iter()
        .map(|r| r["key"]["range"].as_str().expect("range key").to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mutation_accepts_descriptive_name() {
    let temp_dir = TempDir::new().expect("temp dir");
    let node = create_node(temp_dir.path().to_str().unwrap()).await;
    load_descriptive_schema(&node, CANONICAL_NAME, DESCRIPTIVE_NAME).await;

    let mut fields_and_values: HashMap<String, serde_json::Value> = HashMap::new();
    fields_and_values.insert("name".to_string(), json!("Read book"));
    fields_and_values.insert("completed".to_string(), json!(false));
    let key_value =
        fold_db::schema::types::key_value::KeyValue::new(None, Some("Read book".to_string()));

    let response = mutation_handlers::execute_mutation_from_components(
        DESCRIPTIVE_NAME.to_string(),
        fields_and_values,
        key_value,
        MutationType::Create,
        "test-user",
        &node,
    )
    .await
    .expect("mutation via descriptive_name");
    assert!(response.data.expect("mutation response data").success);

    // Confirm the write landed under the canonical schema.
    let results = run_query(&node, CANONICAL_NAME).await;
    let arr = results.as_array().expect("results array");
    assert_eq!(
        arr.len(),
        1,
        "descriptive-name mutation must reach the same canonical schema"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ambiguous_descriptive_name_returns_400() {
    let temp_dir = TempDir::new().expect("temp dir");
    let node = create_node(temp_dir.path().to_str().unwrap()).await;
    let canonical_b = "abcdef00000000000000000000000000000000000000000000000000000000ff";
    load_descriptive_schema(&node, CANONICAL_NAME, DESCRIPTIVE_NAME).await;
    load_descriptive_schema(&node, canonical_b, DESCRIPTIVE_NAME).await;

    let query = Query::new(
        DESCRIPTIVE_NAME.to_string(),
        vec!["name".to_string(), "completed".to_string()],
    );
    let err = query_handlers::execute_query(query, None, None, "test-user", &node)
        .await
        .expect_err("ambiguous descriptive_name must error");
    let msg = format!("{}", err);
    assert!(
        msg.contains("ambiguous"),
        "expected ambiguous-error message, got: {}",
        msg
    );
    // 400 status (BadRequest), not 500.
    assert_eq!(err.status_code(), 400);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_schema_name_falls_through_to_downstream_not_found() {
    let temp_dir = TempDir::new().expect("temp dir");
    let node = create_node(temp_dir.path().to_str().unwrap()).await;
    load_descriptive_schema(&node, CANONICAL_NAME, DESCRIPTIVE_NAME).await;

    // A name that matches neither canonical nor descriptive should reach
    // fold_db's query executor and surface its existing "not found" error.
    let query = Query::new("Totally Made Up".to_string(), vec!["name".to_string()]);
    let err = query_handlers::execute_query(query, None, None, "test-user", &node)
        .await
        .expect_err("unknown schema must error");
    let msg = format!("{}", err);
    assert!(
        msg.contains("not found"),
        "expected fold_db's not-found message, got: {}",
        msg
    );
    let _ = SchemaError::NotFound("placeholder".into()); // keep import meaningful for clarity
}
