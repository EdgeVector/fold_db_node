//! Alpha BLOCKER b507b regression test.
//!
//! After `POST /api/schema/<name>/set-org-hash`, subsequent queries must
//! continue returning data — both post-tag atoms (the common case) and
//! pre-tag atoms (via the 3e063 dual-read fallback). Run-6 saw EVERY query
//! fail with `Atom not found for key '…'` because `FieldCommon.org_hash`
//! was missing at the query call site even though `schema.org_hash` was set.

use fold_db::schema::types::key_value::KeyValue;
use fold_db::schema::types::operations::Query;
use fold_db::schema::SchemaState;
use fold_db::test_helpers::TestSchemaBuilder;
use fold_db::MutationType;
use fold_db_node::fold_node::config::NodeConfig;
use fold_db_node::fold_node::FoldNode;
use fold_db_node::fold_node::OperationProcessor;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

async fn create_node(db_path: &str) -> FoldNode {
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let config = NodeConfig::new(db_path.into())
        .with_schema_service_url("test://mock")
        .with_seed_identity(fold_db_node::identity::identity_from_keypair(&keypair));
    FoldNode::new(config).await.expect("create FoldNode")
}

fn register_test_org(node: &FoldNode, org_hash: &str) {
    let fold_db = node.get_fold_db().expect("get fold_db");
    let pool = fold_db.sled_pool().expect("sled backend").clone();
    fold_db::org::operations::insert_test_membership(&pool, org_hash)
        .expect("insert test org membership");
}

async fn write_file(processor: &OperationProcessor, source_file: &str, content: &str) {
    let mut fields = HashMap::new();
    fields.insert("source_file".to_string(), json!(source_file));
    fields.insert("content".to_string(), json!(content));

    processor
        .execute_mutation(
            "NotesShared".to_string(),
            fields,
            KeyValue::new(None, Some(source_file.to_string())),
            MutationType::Create,
        )
        .await
        .expect("execute mutation");
}

async fn query_source_files(processor: &OperationProcessor) -> Vec<String> {
    let query = Query::new("NotesShared".to_string(), vec!["source_file".to_string()]);
    let result = processor
        .execute_query_map(query, &fold_db::access::AccessContext::owner("test"))
        .await
        .expect("execute query");

    let field_results = result.get("source_file").expect("source_file in results");
    let mut files: Vec<String> = field_results
        .values()
        .map(|fv| fv.value.as_str().unwrap().to_string())
        .collect();
    files.sort();
    files
}

/// All post-tag: tag the schema FIRST, then ingest. Queries must return everything.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_org_hash_then_write_then_query_returns_all_post_tag_data() {
    let temp_dir = TempDir::new().expect("temp dir");
    let node = create_node(temp_dir.path().to_str().unwrap()).await;

    let schema_json = TestSchemaBuilder::new("NotesShared")
        .fields(&["content"])
        .range_key("source_file")
        .build_json();
    let fold_db = node.get_fold_db().expect("get fold_db");
    fold_db
        .schema_manager()
        .load_schema_from_json(&schema_json)
        .await
        .expect("load schema");
    fold_db
        .schema_manager()
        .set_schema_state("NotesShared", SchemaState::Approved)
        .await
        .expect("approve schema");

    let org_hash = "b507b5b507b5b507b5b507b5b507b5b507b5b507b5b507b5b507b5b507b5b507".to_string();
    register_test_org(&node, &org_hash);

    let node_arc = Arc::new(node);
    let processor = OperationProcessor::new(node_arc.clone());

    // Tag FIRST, then write, then query.
    processor
        .set_schema_org_hash("NotesShared", Some(org_hash.clone()))
        .await
        .expect("set_schema_org_hash");

    for i in 0..5 {
        write_file(
            &processor,
            &format!("note-{}.md", i),
            &format!("body {}", i),
        )
        .await;
    }

    let files = query_source_files(&processor).await;
    assert_eq!(
        files.len(),
        5,
        "post-tag writes must all be queryable after set-org-hash; got {:?}",
        files
    );
}

/// Pre-tag atoms plus the 3e063 dual-read fallback: write first, then tag, then query.
/// Queries must return pre-tag data via the unprefixed fallback.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_then_set_org_hash_then_query_returns_pre_tag_data() {
    let temp_dir = TempDir::new().expect("temp dir");
    let node = create_node(temp_dir.path().to_str().unwrap()).await;

    let schema_json = TestSchemaBuilder::new("NotesShared")
        .fields(&["content"])
        .range_key("source_file")
        .build_json();
    let fold_db = node.get_fold_db().expect("get fold_db");
    fold_db
        .schema_manager()
        .load_schema_from_json(&schema_json)
        .await
        .expect("load schema");
    fold_db
        .schema_manager()
        .set_schema_state("NotesShared", SchemaState::Approved)
        .await
        .expect("approve schema");

    let node_arc = Arc::new(node);
    let processor = OperationProcessor::new(node_arc.clone());

    // Write BEFORE tagging.
    for i in 0..3 {
        write_file(&processor, &format!("pre-{}.md", i), &format!("body {}", i)).await;
    }

    // Now tag.
    let org_hash = "b507b5b507b5b507b5b507b5b507b5b507b5b507b5b507b5b507b5b507b5b507".to_string();
    processor
        .set_schema_org_hash("NotesShared", Some(org_hash.clone()))
        .await
        .expect("set_schema_org_hash");

    // Queries must still see the pre-tag data (3e063 dual-read fallback).
    let files = query_source_files(&processor).await;
    assert_eq!(
        files.len(),
        3,
        "pre-tag writes must remain queryable after set-org-hash via dual-read fallback; got {:?}",
        files
    );
}

/// Full dogfood path: write pre-tag, tag, write post-tag, query — must see ALL 8.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mixed_pre_and_post_tag_data_all_queryable() {
    let temp_dir = TempDir::new().expect("temp dir");
    let node = create_node(temp_dir.path().to_str().unwrap()).await;

    let schema_json = TestSchemaBuilder::new("NotesShared")
        .fields(&["content"])
        .range_key("source_file")
        .build_json();
    let fold_db = node.get_fold_db().expect("get fold_db");
    fold_db
        .schema_manager()
        .load_schema_from_json(&schema_json)
        .await
        .expect("load schema");
    fold_db
        .schema_manager()
        .set_schema_state("NotesShared", SchemaState::Approved)
        .await
        .expect("approve schema");

    let org_hash = "b507b5b507b5b507b5b507b5b507b5b507b5b507b5b507b5b507b5b507b5b507".to_string();
    register_test_org(&node, &org_hash);

    let node_arc = Arc::new(node);
    let processor = OperationProcessor::new(node_arc.clone());

    for i in 0..3 {
        write_file(&processor, &format!("pre-{}.md", i), &format!("body {}", i)).await;
    }

    processor
        .set_schema_org_hash("NotesShared", Some(org_hash.clone()))
        .await
        .expect("set_schema_org_hash");

    for i in 0..5 {
        write_file(
            &processor,
            &format!("post-{}.md", i),
            &format!("body {}", i),
        )
        .await;
    }

    let files = query_source_files(&processor).await;
    assert_eq!(
        files.len(),
        8,
        "pre-tag + post-tag writes must both be queryable; got {:?}",
        files
    );
}
