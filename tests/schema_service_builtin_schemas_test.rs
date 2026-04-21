//! Integration test: verify that `SchemaServiceServer::new_with_builtins`
//! installs every Phase 1 built-in schema into a fresh Sled store,
//! and that `seed()` is idempotent across repeated invocations.
//!
//! This test is independent of the fingerprints subsystem — it
//! exercises the schema service's own seeding contract. If this test
//! fails, fold_db_node's fingerprints subsystem cannot start.

use schema_service_core::builtin_schemas::{self, PHASE_1_DESCRIPTIVE_NAMES};
use schema_service_core::state::SchemaServiceState;
use schema_service_server_http::SchemaServiceServer;
use std::collections::HashSet;
use tempfile::TempDir;

#[tokio::test]
async fn new_with_builtins_installs_all_twelve_on_fresh_store() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir
        .path()
        .join("fresh_store")
        .to_string_lossy()
        .to_string();

    // bind_address is irrelevant for this test — we never call run().
    let _server = SchemaServiceServer::new_with_builtins(db_path.clone(), "127.0.0.1:0")
        .await
        .expect("new_with_builtins must succeed on fresh store");

    // Re-open the same Sled store via a separate state handle and
    // verify the twelve built-ins are present. (The server keeps the
    // state internal; checking via a second handle is the easiest way
    // to inspect the underlying storage.)
    drop(_server); // release the original state's sled handle
    let state = SchemaServiceState::new(
        db_path,
        ::std::sync::Arc::new(::schema_service_core::embedder::MockEmbeddingModel),
    )
    .unwrap();
    let all_names: HashSet<String> = state
        .get_schema_names()
        .expect("list schemas")
        .into_iter()
        .collect();

    // The schema service renames every schema to its identity_hash,
    // so we can't just check PHASE_1_DESCRIPTIVE_NAMES against
    // schema.name. Instead we check that the count is >= 12 and
    // every descriptive name has a corresponding schema whose
    // descriptive_name matches.
    assert!(
        all_names.len() >= PHASE_1_DESCRIPTIVE_NAMES.len(),
        "expected at least {} schemas, got {}: {:?}",
        PHASE_1_DESCRIPTIVE_NAMES.len(),
        all_names.len(),
        all_names
    );

    let all_schemas = state.get_all_schemas_cached().expect("list cached schemas");
    let descriptive_seen: HashSet<String> = all_schemas
        .iter()
        .filter_map(|s| s.descriptive_name.clone())
        .collect();

    for expected in PHASE_1_DESCRIPTIVE_NAMES {
        assert!(
            descriptive_seen.contains(*expected),
            "built-in schema '{}' missing after new_with_builtins (descriptive names present: {:?})",
            expected,
            descriptive_seen
        );
    }
}

#[tokio::test]
async fn seed_is_idempotent_across_multiple_calls() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir
        .path()
        .join("idempotent_store")
        .to_string_lossy()
        .to_string();

    let state = SchemaServiceState::new(
        db_path,
        ::std::sync::Arc::new(::schema_service_core::embedder::MockEmbeddingModel),
    )
    .unwrap();

    // Seed once.
    builtin_schemas::seed(&state).await.expect("first seed");
    let count_1 = state.get_schema_names().unwrap().len();

    // Seed again — should be a no-op.
    builtin_schemas::seed(&state).await.expect("second seed");
    let count_2 = state.get_schema_names().unwrap().len();

    // And once more for good measure.
    builtin_schemas::seed(&state).await.expect("third seed");
    let count_3 = state.get_schema_names().unwrap().len();

    assert_eq!(
        count_1, count_2,
        "second seed must not add new schemas (first: {}, second: {})",
        count_1, count_2
    );
    assert_eq!(
        count_2, count_3,
        "third seed must not add new schemas (second: {}, third: {})",
        count_2, count_3
    );
    assert!(
        count_1 >= PHASE_1_DESCRIPTIVE_NAMES.len(),
        "first seed must install at least {} schemas, got {}",
        PHASE_1_DESCRIPTIVE_NAMES.len(),
        count_1
    );
}
