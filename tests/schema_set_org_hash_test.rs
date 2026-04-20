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

/// Regression guard for 903af (alpha dogfood run-7 COVERAGE gap).
///
/// fold_db#568 covered `trust_domain` persistence at the SchemaCore level
/// (direct DbOps round-trip). This test exercises the same invariant through
/// the full on-node path — `OperationProcessor::set_schema_org_hash` on one
/// `FoldNode`, drop + reopen a second `FoldNode` on the same Sled dir,
/// assert `trust_domain` survived — after b507b/2767c landed org-prefix
/// replay dispatch.
#[tokio::test(flavor = "multi_thread")]
async fn test_set_schema_org_hash_persists_across_node_restart() {
    let tmp = TempDir::new().expect("Failed to create temp dir");
    let db_path: std::path::PathBuf = tmp.path().into();

    // Identity must be stable across restarts so the second FoldNode reuses
    // the same Sled state rather than booting as a fresh node.
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let pubkey = keypair.public_key_base64();
    let seckey = keypair.secret_key_base64();

    let org_hash = "deadbeefcafefeed".to_string();
    let expected_trust_domain = format!("org:{}", org_hash);

    // --- First boot: tag the schema. ---
    let pool = {
        let config = NodeConfig::new(db_path.clone())
            .with_schema_service_url("test://mock")
            .with_identity(&pubkey, &seckey);
        let node = FoldNode::new(config)
            .await
            .expect("Failed to create first FoldNode");

        load_schema(&node, "BlogPost.json").await;

        let pool = node
            .get_fold_db()
            .expect("get_fold_db")
            .sled_pool()
            .cloned();

        let node_arc = Arc::new(node);
        let processor = OperationProcessor::new(node_arc.clone());

        processor
            .approve_schema("BlogPost")
            .await
            .expect("approve_schema");
        processor
            .set_schema_org_hash("BlogPost", Some(org_hash.clone()))
            .await
            .expect("set_schema_org_hash");

        let tagged = processor
            .get_schema("BlogPost")
            .await
            .expect("get_schema")
            .expect("schema present");
        assert_eq!(
            tagged.schema.trust_domain.as_deref(),
            Some(expected_trust_domain.as_str()),
            "trust_domain should be tagged before restart"
        );

        drop(processor);
        drop(node_arc);
        pool
    };

    // Release the first pool's Sled handle so the second FoldNode can acquire
    // the file lock without waiting 30s for the idle reaper. The reaper task
    // spawned by the first pool is a leaked tokio task that holds only the
    // pool (not the Db) — benign for the remainder of this test.
    if let Some(p) = pool {
        p.release();
        drop(p);
    }

    // --- Second boot: same path, expect trust_domain still set. ---
    let config = NodeConfig::new(db_path)
        .with_schema_service_url("test://mock")
        .with_identity(&pubkey, &seckey);
    let node = FoldNode::new(config)
        .await
        .expect("Failed to create second FoldNode on same path");
    let processor = OperationProcessor::new(Arc::new(node));

    let reloaded = processor
        .get_schema("BlogPost")
        .await
        .expect("get_schema after restart")
        .expect("schema present after restart");

    assert_eq!(
        reloaded.schema.org_hash.as_deref(),
        Some(org_hash.as_str()),
        "org_hash must persist across FoldNode restart"
    );
    assert_eq!(
        reloaded.schema.trust_domain.as_deref(),
        Some(expected_trust_domain.as_str()),
        "trust_domain must persist across FoldNode restart (903af regression)"
    );

    // Field-level org_hash must also survive — the mirror done in
    // set_schema_org_hash is what makes subsequent writes land on the
    // org-prefixed sync log.
    let db = processor.get_db_public().expect("fold_db");
    let schema = db
        .schema_manager()
        .get_schema_metadata("BlogPost")
        .expect("get_schema_metadata")
        .expect("schema present");
    for (field_name, field) in &schema.runtime_fields {
        assert_eq!(
            field.common().org_hash(),
            Some(org_hash.as_str()),
            "field '{}' should carry org_hash after restart",
            field_name
        );
    }
}
