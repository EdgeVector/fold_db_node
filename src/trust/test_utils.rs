#![cfg(test)]
//! Shared test helpers for the `trust` module.

use fold_db::FoldDB;
use std::sync::Arc;
use tempfile::TempDir;

pub(super) async fn setup_db() -> (Arc<FoldDB>, TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let config = crate::fold_node::NodeConfig::new(tmp.path().to_path_buf())
        .with_schema_service_url("test://mock")
        .with_seed_identity(crate::identity::identity_from_keypair(&keypair));
    let node = crate::fold_node::FoldNode::new(config).await.unwrap();
    let db = node.get_fold_db().unwrap();
    (db, tmp)
}
