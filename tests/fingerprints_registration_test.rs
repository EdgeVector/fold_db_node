//! Integration test: Phase 1 fingerprint-schema registration against
//! an in-process schema service.
//!
//! This is the test that replaces the deleted, bypass-prone
//! `fingerprints_schema_roundtrip_test.rs`. Per the architectural
//! invariant documented in `exemem-workspace/docs/designs/fingerprints.md`,
//! **all schemas must come from the schema service**. There must be
//! NO local schemas that were not first verified by the service. This
//! test enforces that invariant end-to-end.
//!
//! ## What it exercises
//!
//! 1. Spin up a `SchemaServiceState` in-process, backed by a tempdir.
//! 2. Wrap it in an `actix_web` HTTP server bound to a random port.
//! 3. Create a `FoldNode` pointing at that URL.
//! 4. Call `register_phase_1_schemas(&node)` — the real thing, no mocks.
//! 5. Assert that the schema service accepted every one of the twelve
//!    schemas cleanly.
//! 6. Assert that the canonical names in the registry differ from the
//!    descriptive names we proposed (the service renamed them to
//!    identity_hash, as documented).
//! 7. Assert that the descriptive_name → canonical_name lookup works
//!    via `canonical_names::lookup()`.
//! 8. Assert that every canonical schema is queryable on the local
//!    node (proving the loaded-and-approved step worked).
//!
//! The pattern for spinning up the schema service in-process is lifted
//! from `tests/image_ingestion_keys_test.rs`.

use fold_db_node::fingerprints::canonical_names;
use fold_db_node::fingerprints::registration::register_phase_1_schemas;
use fold_db_node::fold_node::config::NodeConfig;
use fold_db_node::fold_node::FoldNode;
use tempfile::TempDir;

mod common;
use common::schema_service::spawn_schema_service_with_builtins as spawn_schema_service;

async fn create_node(schema_service_url: &str) -> (FoldNode, TempDir) {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_string_lossy().to_string();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let config = NodeConfig::new(path.into())
        .with_schema_service_url(schema_service_url)
        .with_seed_identity(fold_db_node::identity::identity_from_keypair(&keypair));
    let node = FoldNode::new(config).await.expect("create FoldNode");
    (node, tmp)
}

/// Core verification: run register_phase_1_schemas against a real
/// schema service and assert that every schema made it through the
/// full flow (propose → canonicalize → load → approve).
#[actix_web::test]
async fn register_phase_1_schemas_end_to_end() {
    // Each test runs in its own process thanks to cargo test's
    // binary-per-file model, so the global canonical_names registry
    // is fresh here.
    canonical_names::reset_for_tests();

    let service = spawn_schema_service().await;
    let (node, _tmp) = create_node(&service.url).await;

    let outcome = register_phase_1_schemas(&node)
        .await
        .expect("register_phase_1_schemas must succeed");

    // The canonical Phase 1 schema list lives in schema_service_core
    // and grows over time via the bump cascade — don't pin a hard
    // count or duplicate the list here. Every name that
    // `PHASE_1_DESCRIPTIVE_NAMES` exposes upstream must register, no
    // more, no fewer.
    use schema_service_core::builtin_schemas::PHASE_1_DESCRIPTIVE_NAMES;
    assert_eq!(
        outcome.total(),
        PHASE_1_DESCRIPTIVE_NAMES.len(),
        "expected all {} Phase 1 schemas to register, got {}",
        PHASE_1_DESCRIPTIVE_NAMES.len(),
        outcome.total()
    );

    let expected_descriptive: Vec<&str> = PHASE_1_DESCRIPTIVE_NAMES.to_vec();
    for expected in &expected_descriptive {
        assert!(
            outcome
                .registered
                .iter()
                .any(|r| r.descriptive_name == *expected),
            "descriptive name '{}' missing from registration outcome",
            expected
        );
    }

    // THE CORE INVARIANT: the schema service renames schemas to
    // their identity_hash. canonical_name must differ from
    // descriptive_name for every schema. If this assertion fails,
    // either the schema service stopped canonicalizing or the
    // registration flow is bypassing it.
    for entry in &outcome.registered {
        assert_ne!(
            entry.canonical_name, entry.descriptive_name,
            "schema '{}' must have been renamed by the schema service, but \
             canonical_name == descriptive_name. This indicates a bypass.",
            entry.descriptive_name
        );
    }

    // Every canonical_name must be a distinct hash (no collisions).
    let unique_canonical: std::collections::HashSet<_> = outcome
        .registered
        .iter()
        .map(|r| r.canonical_name.clone())
        .collect();
    assert_eq!(
        unique_canonical.len(),
        outcome.registered.len(),
        "canonical names must all be distinct (identity hash collision?)"
    );

    // canonical_names::lookup() resolves every descriptive_name.
    for expected in &expected_descriptive {
        let canonical = canonical_names::lookup(expected)
            .unwrap_or_else(|e| panic!("lookup({}) failed: {}", expected, e));
        assert!(
            !canonical.is_empty(),
            "lookup({}) returned empty string",
            expected
        );
        // And the resolved canonical matches the outcome.
        let matching = outcome
            .registered
            .iter()
            .find(|r| r.descriptive_name == *expected)
            .unwrap();
        assert_eq!(
            canonical, matching.canonical_name,
            "canonical_names lookup inconsistent with registration outcome for '{}'",
            expected
        );
    }

    // Every canonical schema is queryable on the local node — proving
    // load_schema_from_json + approve completed for each.
    let fold_db = node.get_fold_db().expect("fold_db handle");
    let manager = fold_db.schema_manager();
    for entry in &outcome.registered {
        let meta = manager.get_schema_metadata(&entry.canonical_name);
        assert!(
            meta.ok().flatten().is_some(),
            "canonical schema '{}' (descriptive '{}') not loaded locally",
            entry.canonical_name,
            entry.descriptive_name,
        );
    }
}

/// Re-running registration on a node that already has these schemas
/// must not panic and must not double-register. Uses the same
/// in-process schema service — so the second call should see
/// AlreadyExists / deterministic canonical names and produce the
/// same mapping.
#[actix_web::test]
async fn register_phase_1_schemas_is_idempotent() {
    canonical_names::reset_for_tests();

    let service = spawn_schema_service().await;
    let (node, _tmp) = create_node(&service.url).await;

    use schema_service_core::builtin_schemas::PHASE_1_DESCRIPTIVE_NAMES;
    let expected_total = PHASE_1_DESCRIPTIVE_NAMES.len();

    let first = register_phase_1_schemas(&node).await.expect("first run");
    assert_eq!(first.total(), expected_total);

    // Second call. The canonical_names registry is already populated
    // with the same mapping, so install() should succeed without
    // returning a conflict.
    let second = register_phase_1_schemas(&node).await.expect("second run");
    assert_eq!(second.total(), expected_total);

    // Canonical names must be identical across the two runs.
    for (a, b) in first.registered.iter().zip(second.registered.iter()) {
        assert_eq!(
            a.descriptive_name, b.descriptive_name,
            "descriptive names should appear in the same order"
        );
        assert_eq!(
            a.canonical_name, b.canonical_name,
            "canonical names must be deterministic across runs for '{}'",
            a.descriptive_name
        );
    }
}
