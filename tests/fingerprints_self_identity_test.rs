//! Integration test: Self-Identity + Me Persona bootstrap end-to-end.
//!
//! Validates the full stack for the owner's own data:
//!
//! 1. Spin up an in-process schema service with Phase 1 built-ins
//! 2. Create a FoldNode pointing at it
//! 3. `register_phase_1_schemas` populates the canonical_names registry
//! 4. `bootstrap_self_identity(&node, "Tom Tang")` writes the four
//!    records (Identity, NodePubKey Fingerprint, IdentityReceipt,
//!    Me Persona)
//! 5. Query each canonical schema and verify the records persisted
//!    with the expected shape
//! 6. Resolve the Me Persona via `PersonaResolver` and verify the
//!    cluster contains exactly the NodePubKey fingerprint (no edges
//!    yet; the cluster is just the seed)
//! 7. Round-trip the signed Identity Card: re-derive the public key
//!    from the stored `pub_key` field, verify the `card_signature`
//!    against a reconstructed canonical payload
//!
//! This is the seed integration case that every other Phase 1
//! feature builds on — the resolver, writer, schema registration,
//! canonical_names layer, and Persona traversal are all exercised
//! in one pass.

use actix_web::{web, App, HttpResponse, HttpServer};
use fold_db::schema::types::field::HashRangeFilter;
use fold_db::schema::types::operations::Query;
use fold_db_node::fingerprints::canonical_names;
use fold_db_node::fingerprints::registration::register_phase_1_schemas;
use fold_db_node::fingerprints::resolver::{PersonaResolver, PersonaSpec};
use fold_db_node::fingerprints::schemas::{FINGERPRINT, IDENTITY, IDENTITY_RECEIPT, PERSONA};
use fold_db_node::fingerprints::self_identity::bootstrap_self_identity;
use fold_db_node::fold_node::config::NodeConfig;
use fold_db_node::fold_node::{FoldNode, OperationProcessor};
use fold_db_node::schema_service::server::{
    AddSchemaResponse, SchemaAddOutcome, SchemaServiceState,
};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::net::TcpListener;
use std::sync::Arc;
use tempfile::TempDir;

// ── In-process schema service setup (lifted from other integration tests) ──

async fn handle_add_schema(
    payload: web::Json<serde_json::Value>,
    state: web::Data<SchemaServiceState>,
) -> HttpResponse {
    let req = payload.into_inner();
    let schema: fold_db::schema::types::Schema = match serde_json::from_value(req["schema"].clone())
    {
        Ok(s) => s,
        Err(e) => {
            return HttpResponse::BadRequest()
                .json(json!({ "error": format!("deserialize schema: {}", e) }))
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
        _ => HttpResponse::NotFound().json(json!({ "error": "not found" })),
    }
}

async fn handle_list_schemas(state: web::Data<SchemaServiceState>) -> HttpResponse {
    match state.get_schema_names() {
        Ok(names) => HttpResponse::Ok().json(json!({ "schemas": names })),
        Err(e) => HttpResponse::InternalServerError().json(json!({ "error": e.to_string() })),
    }
}

async fn handle_available(state: web::Data<SchemaServiceState>) -> HttpResponse {
    match state.get_all_schemas_cached() {
        Ok(s) => HttpResponse::Ok().json(json!({ "schemas": s })),
        Err(e) => HttpResponse::InternalServerError().json(json!({ "error": e.to_string() })),
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
        .join("self_identity_test_registry")
        .to_string_lossy()
        .to_string();
    let state = SchemaServiceState::new(db_path).unwrap();
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

fn query_by_hash_key(canonical: &str, fields: &[&str], hash_key: &str) -> Query {
    Query {
        schema_name: canonical.to_string(),
        fields: fields.iter().map(|s| s.to_string()).collect(),
        filter: Some(HashRangeFilter::HashKey(hash_key.to_string())),
        as_of: None,
        rehydrate_depth: None,
        sort_order: None,
        value_filters: None,
    }
}

// ── The test ────────────────────────────────────────────────────

#[actix_web::test]
async fn bootstrap_self_identity_writes_all_four_records_end_to_end() {
    canonical_names::reset_for_tests();

    let service = spawn_schema_service().await;
    let (node, _tmp) = create_node(&service.url).await;

    register_phase_1_schemas(&node)
        .await
        .expect("register_phase_1_schemas");

    let display_name = "Tom Tang".to_string();
    let outcome = bootstrap_self_identity(node.clone(), display_name.clone())
        .await
        .expect("bootstrap_self_identity");

    // The self-Identity id encodes the node's pubkey directly, so
    // we can reconstruct it for the assertion below.
    let pub_key = node.get_node_public_key().to_string();
    assert_eq!(outcome.self_identity_id, format!("id_{}", pub_key));
    assert!(outcome.node_pub_key_fingerprint_id.starts_with("fp_"));
    assert!(outcome.me_persona_id.starts_with("ps_"));
    assert!(outcome.identity_receipt_id.starts_with("ir_"));

    // Query each canonical schema and verify the record exists with
    // the expected fields. Resolving canonical names through the
    // registry ensures we're looking up runtime schema names, not
    // descriptive labels.
    let processor = OperationProcessor::new(node.clone());

    // ── Identity ──
    let identity_canonical = canonical_names::lookup(IDENTITY).unwrap();
    let results = processor
        .execute_query_json(query_by_hash_key(
            &identity_canonical,
            &[
                "id",
                "pub_key",
                "display_name",
                "card_signature",
                "node_id",
                "issued_at",
            ],
            &outcome.self_identity_id,
        ))
        .await
        .expect("query identity");
    assert_eq!(results.len(), 1);
    let fields = results[0].get("fields").unwrap();
    assert_eq!(fields["pub_key"], json!(pub_key));
    assert_eq!(fields["display_name"], json!(display_name));
    assert_eq!(fields["node_id"], json!(pub_key));
    let stored_signature = fields["card_signature"]
        .as_str()
        .expect("card_signature must be a string")
        .to_string();
    let stored_issued_at = fields["issued_at"]
        .as_str()
        .expect("issued_at must be a string")
        .to_string();
    assert!(!stored_signature.is_empty());

    // ── Verify the stored signature re-verifies against the payload
    //    reconstructed from the stored fields. This exercises the
    //    full signing-roundtrip: build canonical bytes from what
    //    landed in the record, verify with the pubkey.
    {
        use base64::Engine;
        use ed25519_dalek::{Verifier, VerifyingKey};

        let pub_key_bytes = base64::engine::general_purpose::STANDARD
            .decode(&pub_key)
            .expect("pub_key base64 decodes");
        let pub_key_array: [u8; 32] = pub_key_bytes
            .as_slice()
            .try_into()
            .expect("ed25519 pub_key is 32 bytes");
        let verifying = VerifyingKey::from_bytes(&pub_key_array).unwrap();

        let sig_bytes = base64::engine::general_purpose::STANDARD
            .decode(&stored_signature)
            .expect("signature base64 decodes");
        let sig_array: [u8; 64] = sig_bytes
            .as_slice()
            .try_into()
            .expect("ed25519 signature is 64 bytes");
        let sig = ed25519_dalek::Signature::from_bytes(&sig_array);

        // Must exactly mirror the canonical payload in self_identity.rs.
        let canonical_payload = json!({
            "birthday": Value::Null,
            "display_name": display_name,
            "face_embedding": Value::Null,
            "issued_at": stored_issued_at,
            "pub_key": pub_key,
        });
        let canonical_bytes = serde_json::to_vec(&canonical_payload).unwrap();
        verifying
            .verify(&canonical_bytes, &sig)
            .expect("stored card_signature must verify against reconstructed payload");
    }

    // ── NodePubKey Fingerprint ──
    let fingerprint_canonical = canonical_names::lookup(FINGERPRINT).unwrap();
    let results = processor
        .execute_query_json(query_by_hash_key(
            &fingerprint_canonical,
            &["id", "kind", "value"],
            &outcome.node_pub_key_fingerprint_id,
        ))
        .await
        .expect("query fingerprint");
    assert_eq!(results.len(), 1);
    let fields = results[0].get("fields").unwrap();
    assert_eq!(fields["kind"], json!("node_pub_key"));
    assert_eq!(fields["value"], json!(pub_key));

    // ── IdentityReceipt ──
    let receipt_canonical = canonical_names::lookup(IDENTITY_RECEIPT).unwrap();
    let results = processor
        .execute_query_json(query_by_hash_key(
            &receipt_canonical,
            &["id", "identity_id", "received_via", "trust_level"],
            &outcome.identity_receipt_id,
        ))
        .await
        .expect("query identity receipt");
    assert_eq!(results.len(), 1);
    let fields = results[0].get("fields").unwrap();
    assert_eq!(fields["identity_id"], json!(outcome.self_identity_id));
    assert_eq!(fields["received_via"], json!("Self"));
    assert_eq!(fields["trust_level"], json!("Self"));

    // ── Me Persona ──
    let persona_canonical = canonical_names::lookup(PERSONA).unwrap();
    let results = processor
        .execute_query_json(query_by_hash_key(
            &persona_canonical,
            &[
                "id",
                "name",
                "seed_fingerprint_ids",
                "identity_id",
                "built_in",
                "relationship",
                "threshold",
            ],
            &outcome.me_persona_id,
        ))
        .await
        .expect("query me persona");
    assert_eq!(results.len(), 1);
    let fields = results[0].get("fields").unwrap();
    assert_eq!(fields["name"], json!(display_name));
    assert_eq!(fields["built_in"], json!(true));
    assert_eq!(fields["relationship"], json!("self"));
    // identity_id is a SchemaRef shape: {"schema": "Identity", "key": "<id>"}
    assert_eq!(
        fields["identity_id"],
        json!({ "schema": "Identity", "key": outcome.self_identity_id })
    );
    let seeds: Vec<&Value> = fields["seed_fingerprint_ids"]
        .as_array()
        .unwrap()
        .iter()
        .collect();
    assert_eq!(seeds.len(), 1);
    assert_eq!(
        seeds[0].as_str().unwrap(),
        outcome.node_pub_key_fingerprint_id
    );

    // ── Finally: resolve the Me Persona via PersonaResolver. The
    //    cluster should contain exactly the NodePubKey fingerprint —
    //    there are no edges pointing into it yet because the owner
    //    has ingested no data.
    let resolver = PersonaResolver::new(node.clone());
    let spec = PersonaSpec {
        persona_id: outcome.me_persona_id.clone(),
        seed_fingerprint_ids: vec![outcome.node_pub_key_fingerprint_id.clone()],
        threshold: 0.9,
        excluded_edge_ids: HashSet::new(),
        excluded_mention_ids: HashSet::new(),
        included_mention_ids: HashSet::new(),
        identity_id: Some(outcome.self_identity_id.clone()),
    };
    let result = resolver.resolve(&spec).await.expect("resolve me persona");

    let fps = result.fingerprint_ids();
    assert_eq!(fps.len(), 1);
    assert!(fps.contains(&outcome.node_pub_key_fingerprint_id));
    assert!(
        result.edge_ids().is_empty(),
        "me persona should have no edges yet (no ingestion)"
    );
    assert!(
        result.mention_ids().is_empty(),
        "me persona should have no mentions yet (no ingestion)"
    );
    assert!(
        result.is_clean(),
        "clean resolve expected, got {:?}",
        result.diagnostics()
    );
}

// Note: the "fails loudly without registered canonical_names" case is
// covered by the unit tests in src/fingerprints/canonical_names.rs
// (global_lookup_before_install_returns_error and friends). We do
// NOT test it here because integration tests share process state
// through the canonical_names::REGISTRY OnceCell — the preceding
// end-to-end test leaves the registry populated, and the test
// runner may execute tests in parallel, which makes negative
// assertions on registry state racy. The positive-path integration
// test above is sufficient to validate the bootstrap flow.
