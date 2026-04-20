//! Integration test for tagging an existing schema with an org_hash.
//!
//! Regression test for the alpha dogfood run-2 gap: there was no user surface
//! to set `schema.org_hash` post-creation, blocking Flow 3 step c (molecule
//! propagation over org sync).

use fold_db::schema::types::field::Field;
use fold_db_node::fold_node::config::NodeConfig;
use fold_db_node::fold_node::FoldNode;
use fold_db_node::fold_node::OperationProcessor;
use std::sync::Arc;
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

    let fold_db = node.get_fold_db().expect("Failed to get FoldDB");
    fold_db
        .load_schema_from_file(&schema_path)
        .await
        .expect("Failed to load schema");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_set_schema_org_hash_persists_and_propagates() {
    let (node, _tmp) = setup_node().await;
    load_schema(&node, "BlogPost.json").await;
    let node_arc = Arc::new(node);
    let processor = OperationProcessor::new(node_arc.clone());

    processor
        .approve_schema("BlogPost")
        .await
        .expect("approve_schema");

    // Sanity: no org_hash initially.
    let before = processor
        .get_schema("BlogPost")
        .await
        .expect("get_schema")
        .expect("schema present");
    assert!(
        before.schema.org_hash.is_none(),
        "expected no org_hash before tagging, got {:?}",
        before.schema.org_hash
    );

    // Tag it.
    let org_hash = "deadbeefcafefeed".to_string();
    processor
        .set_schema_org_hash("BlogPost", Some(org_hash.clone()))
        .await
        .expect("set_schema_org_hash");

    // Schema-level fields updated.
    let after = processor
        .get_schema("BlogPost")
        .await
        .expect("get_schema")
        .expect("schema present");
    assert_eq!(after.schema.org_hash.as_deref(), Some(org_hash.as_str()));
    assert_eq!(
        after.schema.trust_domain.as_deref(),
        Some(format!("org:{}", org_hash).as_str()),
        "trust_domain should auto-set to org:{{org_hash}}"
    );

    // Field commons must carry the same org_hash so in-memory storage_key
    // construction picks up the org prefix on subsequent writes.
    let db = node_arc.get_fold_db().expect("fold_db");
    let schema = db
        .schema_manager()
        .get_schema_metadata("BlogPost")
        .expect("get_schema_metadata")
        .expect("schema present");
    assert!(
        !schema.runtime_fields.is_empty(),
        "runtime_fields should be populated"
    );
    for (field_name, field) in &schema.runtime_fields {
        assert_eq!(
            field.common().org_hash(),
            Some(org_hash.as_str()),
            "field '{}' should inherit schema org_hash",
            field_name
        );
    }

    // Clearing the tag reverts both fields.
    processor
        .set_schema_org_hash("BlogPost", None)
        .await
        .expect("clear org_hash");
    let cleared = processor
        .get_schema("BlogPost")
        .await
        .expect("get_schema")
        .expect("schema present");
    assert!(cleared.schema.org_hash.is_none());
    assert!(cleared.schema.trust_domain.is_none());

    let schema = db
        .schema_manager()
        .get_schema_metadata("BlogPost")
        .expect("get_schema_metadata")
        .expect("schema present");
    for (field_name, field) in &schema.runtime_fields {
        assert_eq!(
            field.common().org_hash(),
            None,
            "field '{}' should no longer carry an org_hash",
            field_name
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_set_schema_org_hash_missing_schema_errors() {
    let (node, _tmp) = setup_node().await;
    let processor = OperationProcessor::new(Arc::new(node));

    let err = processor
        .set_schema_org_hash("DoesNotExist", Some("abc".into()))
        .await
        .expect_err("should error on missing schema");
    let msg = err.to_string();
    assert!(
        msg.contains("DoesNotExist"),
        "error should mention schema name, got: {}",
        msg
    );
}
