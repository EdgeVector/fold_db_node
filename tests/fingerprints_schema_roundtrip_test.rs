//! Schema round-trip test for the Fingerprints Phase 1 substrate.
//!
//! This is the first Phase 1 coding task per
//! `docs/designs/fingerprints_phase1_audit.md`: before writing the
//! fingerprints implementation, confirm the schema registration and
//! content-derived-key patterns actually work end to end against the
//! real fold_db schema system.
//!
//! The test exercises the critical invariants:
//!
//! 1. A Hash schema can be registered at runtime via the schema
//!    manager's `load_schema_from_json` without any code-gen.
//! 2. A content-derived hash key (e.g. `fp_<sha256(...)>`) computed in
//!    the caller and passed as a field value becomes the primary key,
//!    with fold_db's upsert semantics handling dedup.
//! 3. Fetching the record back via `Query` returns the original content.
//! 4. Re-inserting the same canonical content dedupes to a single
//!    record (the silent-failure mitigation for concurrent ingest).
//!
//! If ANY of the above fails, the fingerprints design's core storage
//! assumption is wrong and the audit reopens.

use fold_db::schema::types::key_value::KeyValue;
use fold_db::schema::types::operations::Query;
use fold_db::schema::SchemaState;
use fold_db::test_helpers::TestSchemaBuilder;
use fold_db::MutationType;
use fold_db_node::fold_node::config::NodeConfig;
use fold_db_node::fold_node::{FoldNode, OperationProcessor};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

/// Compute a content-derived Fingerprint key in the shape the Phase 1
/// resolver will produce at ingest time.
fn fingerprint_id(kind: &str, value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"Fingerprint");
    hasher.update(kind.as_bytes());
    hasher.update(value.as_bytes());
    format!("fp_{:x}", hasher.finalize())
}

fn fingerprint_schema_json() -> String {
    TestSchemaBuilder::new("FingerprintsTest")
        .fields(&["kind", "value"])
        .hash_key("id")
        .build_json()
}

async fn setup_node() -> (Arc<FoldNode>, TempDir) {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_str().unwrap();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let config = NodeConfig::new(path.into())
        .with_schema_service_url("test://mock")
        .with_identity(&keypair.public_key_base64(), &keypair.secret_key_base64());
    let node = FoldNode::new(config).await.expect("create FoldNode");
    (Arc::new(node), tmp)
}

async fn register_schema(node: &FoldNode) {
    let schema_str = fingerprint_schema_json();
    let fold_db = node.get_fold_db().expect("get fold_db");
    fold_db
        .schema_manager()
        .load_schema_from_json(&schema_str)
        .await
        .expect("load schema");
    fold_db
        .schema_manager()
        .set_schema_state("FingerprintsTest", SchemaState::Approved)
        .await
        .expect("approve schema");
}

async fn write_fingerprint(processor: &OperationProcessor, kind: &str, value: &str) -> String {
    let id = fingerprint_id(kind, value);
    let mut fields = HashMap::new();
    fields.insert("id".to_string(), json!(id));
    fields.insert("kind".to_string(), json!(kind));
    fields.insert("value".to_string(), json!(value));

    processor
        .execute_mutation(
            "FingerprintsTest".to_string(),
            fields,
            KeyValue::new(Some(id.clone()), None),
            MutationType::Create,
        )
        .await
        .expect("execute mutation");

    id
}

fn simple_query() -> Query {
    Query {
        schema_name: "FingerprintsTest".to_string(),
        fields: vec!["id".to_string(), "kind".to_string(), "value".to_string()],
        filter: None,
        as_of: None,
        rehydrate_depth: None,
        sort_order: None,
        value_filters: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn content_derived_hash_key_roundtrips() {
    let (node, _tmp) = setup_node().await;
    register_schema(&node).await;
    let processor = OperationProcessor::new(node.clone());

    let id = write_fingerprint(&processor, "email", "tom@acme.com").await;

    // Deterministic: same inputs must always produce the same key.
    assert_eq!(id, fingerprint_id("email", "tom@acme.com"));
    assert!(
        id.starts_with("fp_"),
        "id must start with fp_ prefix: {}",
        id
    );

    let results = processor
        .execute_query_json(simple_query())
        .await
        .expect("query failed");

    assert_eq!(
        results.len(),
        1,
        "expected exactly one record after single write"
    );

    let record_fields = results[0]
        .get("fields")
        .expect("record missing 'fields' envelope");
    assert_eq!(record_fields["id"], json!(id));
    assert_eq!(record_fields["kind"], json!("email"));
    assert_eq!(record_fields["value"], json!("tom@acme.com"));
}

/// The silent-failure mitigation for concurrent ingest: writing the
/// same canonical content twice must collapse to a single record.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_upsert_of_same_content_dedupes() {
    let (node, _tmp) = setup_node().await;
    register_schema(&node).await;
    let processor = OperationProcessor::new(node.clone());

    // Simulate two independent ingest paths that both observed the same
    // canonical email at roughly the same moment.
    let id_a = write_fingerprint(&processor, "email", "tom@acme.com").await;
    let id_b = write_fingerprint(&processor, "email", "tom@acme.com").await;

    assert_eq!(
        id_a, id_b,
        "content-derived ids must collide for identical canonical inputs"
    );

    let results = processor
        .execute_query_json(simple_query())
        .await
        .expect("query failed");

    let with_our_id: Vec<_> = results
        .iter()
        .filter(|r| {
            r.get("fields")
                .and_then(|f| f.get("id"))
                .and_then(|v| v.as_str())
                == Some(id_a.as_str())
        })
        .collect();

    assert_eq!(
        with_our_id.len(),
        1,
        "expected exactly one record for content-derived id after two writes; \
         dedup invariant violated (Phase 1 design assumption broken)"
    );
}
