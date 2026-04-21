//! Integration test: memory schema registration + native-index round-trip.
//!
//! Proves the Phase 0 thesis from `docs/design/memory_agent.md`: memories
//! are ordinary fold_db molecules. Specifically exercises:
//!
//! 1. Spin up an in-process schema service (actix web + SchemaServiceState).
//! 2. Create a FoldNode pointing at it.
//! 3. Call `memory::register_memory_schema(&node)` — propose, canonicalize,
//!    load locally, approve.
//! 4. Write several memories via the standard mutation processor.
//! 5. Wait for background indexing to complete.
//! 6. Query the native index with a semantic term.
//! 7. Assert the written memories surface in the search results.
//!
//! Schema-service spin-up pattern is lifted from
//! `tests/fingerprints_registration_test.rs`.

use fold_db::db_operations::IndexResult;
use fold_db::schema::types::key_value::KeyValue;
use fold_db::schema::types::operations::MutationType;
use fold_db_node::fold_node::config::NodeConfig;
use fold_db_node::fold_node::{FoldNode, OperationProcessor};
use fold_db_node::memory::{self, fields};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

mod common;
use common::schema_service::spawn_schema_service_with_builtins as spawn_schema_service;

async fn create_node(schema_service_url: &str) -> (Arc<FoldNode>, TempDir) {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_string_lossy().to_string();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let config = NodeConfig::new(path.into())
        .with_schema_service_url(schema_service_url)
        .with_identity(&keypair.public_key_base64(), &keypair.secret_key_base64());
    let node = FoldNode::new(config).await.expect("create FoldNode");
    (Arc::new(node), tmp)
}

// ── Helpers ────────────────────────────────────────────────────────────

fn memory_fields(id: &str, body: &str, kind: &str) -> HashMap<String, Value> {
    let mut f = HashMap::new();
    f.insert(fields::ID.to_string(), json!(id));
    f.insert(fields::BODY.to_string(), json!(body));
    f.insert(fields::KIND.to_string(), json!(kind));
    f.insert(fields::STATUS.to_string(), json!("live"));
    f.insert(fields::TAGS.to_string(), json!([] as [String; 0]));
    f.insert(fields::SOURCE.to_string(), json!("integration_test"));
    f.insert(
        fields::CREATED_AT.to_string(),
        json!("2026-04-20T12:00:00Z"),
    );
    f.insert(fields::DERIVED_FROM.to_string(), json!([] as [String; 0]));
    f
}

async fn write_memory(
    processor: &OperationProcessor,
    canonical_name: &str,
    id: &str,
    body: &str,
    kind: &str,
) {
    let fields_and_values = memory_fields(id, body, kind);
    let key_value = KeyValue::new(Some(id.to_string()), None);
    processor
        .execute_mutation(
            canonical_name.to_string(),
            fields_and_values,
            key_value,
            MutationType::Create,
        )
        .await
        .unwrap_or_else(|e| panic!("failed to write memory `{}`: {}", id, e));
}

// ── Tests ──────────────────────────────────────────────────────────────

/// Core Phase 0 verification: register the memory schema and write +
/// semantic-search a memory end-to-end. Exercises the auto-embedding
/// path in NativeIndexManager without any transform or agent code.
#[actix_web::test]
async fn memory_register_then_ingest_and_search_roundtrip() {
    let service = spawn_schema_service().await;
    let (node, _tmp) = create_node(&service.url).await;

    // 1. Register the memory schema through the real flow.
    let canonical = memory::register_memory_schema(&node)
        .await
        .expect("register_memory_schema must succeed");

    // Schema service canonicalizes to identity_hash, so the name must
    // differ from the descriptive "Memory" label.
    assert_ne!(
        canonical,
        memory::MEMORY_DESCRIPTIVE_NAME,
        "schema service must rename to identity_hash — got back the descriptive name, indicating a bypass"
    );

    // Schema is loaded locally.
    let fold_db = node.get_fold_db().expect("fold_db handle");
    let manager = fold_db.schema_manager();
    let meta = manager.get_schema_metadata(&canonical);
    assert!(
        meta.ok().flatten().is_some(),
        "canonical memory schema `{}` not loaded locally after register",
        canonical
    );

    // 2. Write three memories with distinct content.
    let processor = OperationProcessor::new(node.clone());

    let memories = [
        (
            "mem_deploy_policy",
            "Always rebase on the base branch before pushing. CI fails otherwise.",
            "feedback",
        ),
        (
            "mem_schema_patent",
            "The schema service deduplicates equivalent schemas across nodes via embedding similarity.",
            "reference",
        ),
        (
            "mem_hiking_note",
            "Mount Rainier Paradise trail is crowded on weekends; go weekdays.",
            "project",
        ),
    ];

    for (id, body, kind) in &memories {
        write_memory(&processor, &canonical, id, body, kind).await;
    }

    // Wait for background indexing to finish — NativeIndexManager
    // embeds asynchronously after mutation persistence.
    node.wait_for_background_tasks(std::time::Duration::from_secs(10))
        .await;

    // 3. Semantic search for content related to the first memory.
    let results = processor
        .native_index_search("rebase before pushing to avoid CI failure")
        .await
        .expect("native_index_search must succeed");

    let memory_hits = filter_memory_hits(&results, &canonical);
    assert!(
        memory_hits.contains(&"mem_deploy_policy".to_string()),
        "native-index search for deploy-policy query did not return the deploy-policy memory. \
         Memory-scoped hits: {:?}",
        memory_hits
    );

    // 4. Semantic search for a different topic surfaces the right memory.
    let hiking_results = processor
        .native_index_search("hiking trail weekend crowds")
        .await
        .expect("native_index_search for hiking must succeed");
    let hiking_hits = filter_memory_hits(&hiking_results, &canonical);
    assert!(
        hiking_hits.contains(&"mem_hiking_note".to_string()),
        "native-index search for hiking query did not return the hiking memory. \
         Memory-scoped hits: {:?}",
        hiking_hits
    );
}

/// Re-registering the memory schema must be idempotent. Running it
/// twice must return the same canonical name (schema service returns
/// AlreadyExists) and must not panic.
#[actix_web::test]
async fn memory_register_is_idempotent() {
    let service = spawn_schema_service().await;
    let (node, _tmp) = create_node(&service.url).await;

    let first = memory::register_memory_schema(&node)
        .await
        .expect("first registration");
    let second = memory::register_memory_schema(&node)
        .await
        .expect("second registration");

    assert_eq!(
        first, second,
        "re-registering must return the same canonical name (identity_hash stable)"
    );
}

// ── Result inspection ──────────────────────────────────────────────────

/// Filter native-index results to those whose schema matches the memory
/// schema's canonical name, and return the `key_value.hash` (the memory
/// id) for each. The search is workspace-wide and picks up fragments
/// from other schemas (Fingerprint, Identity, ...) — we only care about
/// hits against the Memory schema here.
fn filter_memory_hits(results: &[IndexResult], memory_canonical: &str) -> Vec<String> {
    results
        .iter()
        .filter(|r| r.schema_name == memory_canonical)
        .filter_map(|r| r.key_value.hash.clone())
        .collect()
}
