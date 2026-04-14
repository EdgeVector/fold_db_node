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

use actix_web::{web, App, HttpResponse, HttpServer};
use fold_db_node::fingerprints::canonical_names;
use fold_db_node::fingerprints::registration::register_phase_1_schemas;
use fold_db_node::fingerprints::schemas;
use fold_db_node::fold_node::config::NodeConfig;
use fold_db_node::fold_node::FoldNode;
use fold_db_node::schema_service::server::{
    AddSchemaResponse, SchemaAddOutcome, SchemaServiceState,
};
use serde_json::json;
use std::collections::HashMap;
use std::net::TcpListener;
use tempfile::TempDir;

async fn handle_add_schema(
    payload: web::Json<serde_json::Value>,
    state: web::Data<SchemaServiceState>,
) -> HttpResponse {
    let req = payload.into_inner();
    let schema: fold_db::schema::types::Schema = match serde_json::from_value(req["schema"].clone())
    {
        Ok(s) => s,
        Err(e) => {
            return HttpResponse::BadRequest().json(json!({
                "error": format!("deserialize schema: {}", e)
            }))
        }
    };
    let mappers: HashMap<String, String> =
        serde_json::from_value(req["mutation_mappers"].clone()).unwrap_or_default();
    match state.add_schema(schema, mappers).await {
        Ok(outcome) => match outcome {
            SchemaAddOutcome::Added(s, m) => HttpResponse::Created().json(AddSchemaResponse {
                schema: s,
                mutation_mappers: m,
                replaced_schema: None,
            }),
            SchemaAddOutcome::AlreadyExists(s, _) => HttpResponse::Ok().json(AddSchemaResponse {
                schema: s,
                mutation_mappers: HashMap::new(),
                replaced_schema: None,
            }),
            SchemaAddOutcome::Expanded(old, s, m) => {
                HttpResponse::Created().json(AddSchemaResponse {
                    schema: s,
                    mutation_mappers: m,
                    replaced_schema: Some(old),
                })
            }
        },
        Err(e) => HttpResponse::BadRequest().json(json!({ "error": e.to_string() })),
    }
}

async fn handle_get_schema(
    path: web::Path<String>,
    state: web::Data<SchemaServiceState>,
) -> HttpResponse {
    let name = path.into_inner();
    match state.get_schema_by_name(&name) {
        Ok(Some(s)) => HttpResponse::Ok().json(s),
        _ => HttpResponse::NotFound().json(json!({"error": "not found"})),
    }
}

async fn handle_list_schemas(state: web::Data<SchemaServiceState>) -> HttpResponse {
    match state.get_schema_names() {
        Ok(names) => HttpResponse::Ok().json(json!({"schemas": names})),
        Err(e) => HttpResponse::InternalServerError().json(json!({"error": e.to_string()})),
    }
}

async fn handle_available(state: web::Data<SchemaServiceState>) -> HttpResponse {
    match state.get_all_schemas_cached() {
        Ok(s) => HttpResponse::Ok().json(json!({"schemas": s})),
        Err(e) => HttpResponse::InternalServerError().json(json!({"error": e.to_string()})),
    }
}

struct SpawnedService {
    url: String,
    _temp_dir: TempDir,
}

async fn spawn_schema_service() -> SpawnedService {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir
        .path()
        .join("phase1_schema_registry")
        .to_string_lossy()
        .to_string();
    let state = SchemaServiceState::new(db_path).unwrap();
    // Seed the built-in Phase 1 schemas — this is what the real
    // SchemaServiceServer::new_with_builtins() does at startup. Tests
    // that skip this step get an empty service and fetch-and-load
    // fails because the built-ins are missing.
    fold_db_node::schema_service::builtin_schemas::seed(&state)
        .await
        .expect("seed built-in schemas");
    let state_data = web::Data::new(state);
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let state_clone = state_data.clone();

    let server = HttpServer::new(move || {
        App::new().app_data(state_clone.clone()).service(
            web::scope("/api")
                .route("/schemas", web::get().to(handle_list_schemas))
                .route("/schemas", web::post().to(handle_add_schema))
                .route("/schemas/available", web::get().to(handle_available))
                .route("/schema/{name}", web::get().to(handle_get_schema)),
        )
    })
    .listen(listener)
    .unwrap()
    .run();

    actix_web::rt::spawn(server);
    actix_web::rt::time::sleep(std::time::Duration::from_millis(200)).await;

    SpawnedService {
        url: format!("http://127.0.0.1:{}", port),
        _temp_dir: temp_dir,
    }
}

async fn create_node(schema_service_url: &str) -> (FoldNode, TempDir) {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_string_lossy().to_string();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let config = NodeConfig::new(path.into())
        .with_schema_service_url(schema_service_url)
        .with_identity(&keypair.public_key_base64(), &keypair.secret_key_base64());
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

    // Twelve schemas, all registered.
    assert_eq!(
        outcome.total(),
        12,
        "expected all twelve Phase 1 schemas to register, got {}",
        outcome.total()
    );

    // Every expected descriptive_name is present.
    let expected_descriptive = [
        schemas::FINGERPRINT,
        schemas::MENTION,
        schemas::EDGE,
        schemas::IDENTITY,
        schemas::IDENTITY_RECEIPT,
        schemas::PERSONA,
        schemas::EDGE_BY_FINGERPRINT,
        schemas::MENTION_BY_FINGERPRINT,
        schemas::MENTION_BY_SOURCE,
        schemas::INGESTION_ERROR,
        schemas::EXTRACTION_STATUS,
        schemas::RECEIVED_SHARE,
    ];
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

    let first = register_phase_1_schemas(&node).await.expect("first run");
    assert_eq!(first.total(), 12);

    // Second call. The canonical_names registry is already populated
    // with the same mapping, so install() should succeed without
    // returning a conflict.
    let second = register_phase_1_schemas(&node).await.expect("second run");
    assert_eq!(second.total(), 12);

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
