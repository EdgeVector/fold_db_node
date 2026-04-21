//! Integration test: all ResolveDiagnostics counters firing together.
//!
//! The resolver test suite already covers each counter in isolation
//! (below_threshold, excluded_edge, forbidden_edge, excluded_mention,
//! missing_seed). This test exercises a single resolve where every
//! non-zero counter fires in the same `ResolveDiagnostics` output,
//! making sure the resolver correctly aggregates them and doesn't
//! short-circuit on the first flag.
//!
//! Graph topology:
//!
//! ```text
//!          0.97           0.90           0.50 (below threshold 0.85)
//!   fp_A ─────────── fp_B ─────────── fp_C ─────────── fp_D
//!     │
//!     │ 0.92 (StrongMatch)    ← this edge is in excluded_edge_ids
//!     │
//!   fp_E
//!
//!   fp_A ── UserForbidden ── fp_F   (kind=UserForbidden, always skipped)
//!
//!   mentions:
//!     mn_1   on {fp_A, fp_B}
//!     mn_2   on {fp_B}         ← this id is in excluded_mention_ids
//!     mn_3   on {fp_C}
//!
//!   seeds: [fp_A, "fp_nonexistent"]  ← second seed triggers missing-seed
//! ```
//!
//! Expected diagnostics after `resolve`:
//! - `missing_seed_fingerprint_ids`  = ["fp_nonexistent"]
//! - `below_threshold_edge_count`    ≥ 1 (the fp_C—fp_D edge at w=0.50)
//! - `forbidden_edge_count`          ≥ 1 (the fp_A—fp_F UserForbidden edge)
//! - `excluded_edge_count`           ≥ 1 (the fp_A—fp_E edge placed in excluded_edge_ids)
//! - `excluded_mention_count`        ≥ 1 (mn_2 when visiting fp_B)
//!
//! None of the existing tests exercises this combination; a regression
//! in the aggregation logic (e.g. accidentally returning early after
//! the first flag) would pass every current test but fail this one.

use actix_web::{web, App, HttpResponse, HttpServer};
use fold_db::schema_service::state::SchemaServiceState;
use fold_db::schema_service::types::{AddSchemaResponse, SchemaAddOutcome};
use fold_db_node::fingerprints::canonical_names;
use fold_db_node::fingerprints::keys::{edge_id, edge_kind, fingerprint_id_for_face_embedding};
use fold_db_node::fingerprints::planned_record::PlannedRecord;
use fold_db_node::fingerprints::registration::register_phase_1_schemas;
use fold_db_node::fingerprints::resolver::{PersonaResolver, PersonaSpec};
use fold_db_node::fingerprints::schemas::{
    EDGE, EDGE_BY_FINGERPRINT, FINGERPRINT, MENTION, MENTION_BY_FINGERPRINT, MENTION_BY_SOURCE,
};
use fold_db_node::fingerprints::writer::write_records;
use fold_db_node::fold_node::config::NodeConfig;
use fold_db_node::fold_node::FoldNode;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::net::TcpListener;
use std::sync::Arc;
use tempfile::TempDir;

// ── In-process schema service setup ───────────────────────────────
//
// Same boilerplate as the other tests/fingerprints_*.rs files.
// Each integration test compiles to its own binary, so helper
// modules can't be shared without a common module subdir.

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
        .join("diagnostics_comprehensive_test_registry")
        .to_string_lossy()
        .to_string();
    let state = SchemaServiceState::new(db_path).unwrap();
    fold_db::schema_service::builtin_schemas::seed(&state)
        .await
        .expect("seed built-in schemas");
    let state_data = web::Data::new(state);
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let state_clone = state_data.clone();

    let server = HttpServer::new(move || {
        App::new().app_data(state_clone.clone()).service(
            web::scope("/v1")
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

// ── Synthetic-data helpers ────────────────────────────────────────

fn fp_from_seed(seed: f32) -> String {
    fingerprint_id_for_face_embedding(&[seed; 8])
}

fn fingerprint_record(fp_id: &str) -> PlannedRecord {
    let mut fields = HashMap::new();
    fields.insert("id".to_string(), json!(fp_id));
    fields.insert("kind".to_string(), json!("face_embedding"));
    fields.insert("value".to_string(), json!([0.1, 0.2, 0.3]));
    fields.insert("first_seen".to_string(), json!("2026-04-14T00:00:00Z"));
    fields.insert("last_seen".to_string(), json!("2026-04-14T00:00:00Z"));
    PlannedRecord::hash(FINGERPRINT, fp_id.to_string(), fields)
}

fn edge_record(a: &str, b: &str, kind: &str, weight: f32) -> (String, Vec<PlannedRecord>) {
    let eg_id = edge_id(a, b, kind);
    let (first, second) = if a <= b { (a, b) } else { (b, a) };

    let mut edge_fields = HashMap::new();
    edge_fields.insert("id".to_string(), json!(eg_id));
    edge_fields.insert("a".to_string(), json!(first));
    edge_fields.insert("b".to_string(), json!(second));
    edge_fields.insert("kind".to_string(), json!(kind));
    edge_fields.insert("weight".to_string(), json!(weight));
    edge_fields.insert(
        "evidence_mention_ids".to_string(),
        json!(Vec::<String>::new()),
    );
    edge_fields.insert("created_at".to_string(), json!("2026-04-14T00:00:00Z"));

    let mut a_junction = HashMap::new();
    a_junction.insert("fingerprint_id".to_string(), json!(a));
    a_junction.insert("edge_id".to_string(), json!(eg_id));
    let mut b_junction = HashMap::new();
    b_junction.insert("fingerprint_id".to_string(), json!(b));
    b_junction.insert("edge_id".to_string(), json!(eg_id));

    let edge = PlannedRecord::hash(EDGE, eg_id.clone(), edge_fields);
    let a_j = PlannedRecord::hash_range(
        EDGE_BY_FINGERPRINT,
        a.to_string(),
        eg_id.clone(),
        a_junction,
    );
    let b_j = PlannedRecord::hash_range(
        EDGE_BY_FINGERPRINT,
        b.to_string(),
        eg_id.clone(),
        b_junction,
    );

    (eg_id, vec![edge, a_j, b_j])
}

fn mention_record(
    mention_id: &str,
    source_key: &str,
    fingerprint_ids: &[&str],
) -> Vec<PlannedRecord> {
    let mut fields = HashMap::new();
    fields.insert("id".to_string(), json!(mention_id));
    fields.insert("source_schema".to_string(), json!("Photos"));
    fields.insert("source_key".to_string(), json!(source_key));
    fields.insert("source_field".to_string(), json!("face"));
    fields.insert(
        "fingerprint_ids".to_string(),
        json!(fingerprint_ids
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()),
    );
    fields.insert("extractor".to_string(), json!("face_detect"));
    fields.insert("confidence".to_string(), json!(1.0_f32));
    fields.insert("created_at".to_string(), json!("2026-04-14T00:00:00Z"));

    let mention = PlannedRecord::hash(MENTION, mention_id.to_string(), fields);

    let composite = format!("Photos:{}", source_key);
    let mut source_junction = HashMap::new();
    source_junction.insert("source_composite".to_string(), json!(composite));
    source_junction.insert("mention_id".to_string(), json!(mention_id));
    let source_j = PlannedRecord::hash_range(
        MENTION_BY_SOURCE,
        composite,
        mention_id.to_string(),
        source_junction,
    );

    let mut records = vec![mention, source_j];
    for fp in fingerprint_ids {
        let mut j = HashMap::new();
        j.insert("fingerprint_id".to_string(), json!(fp));
        j.insert("mention_id".to_string(), json!(mention_id));
        records.push(PlannedRecord::hash_range(
            MENTION_BY_FINGERPRINT,
            fp.to_string(),
            mention_id.to_string(),
            j,
        ));
    }
    records
}

// ── Test ──────────────────────────────────────────────────────────

#[actix_web::test]
async fn all_resolve_diagnostics_counters_fire_simultaneously() {
    canonical_names::reset_for_tests();

    let service = spawn_schema_service().await;
    let (node, _tmp) = create_node(&service.url).await;
    register_phase_1_schemas(&node).await.unwrap();

    // Build the topology described in the module docs.
    let fp_a = fp_from_seed(0.1);
    let fp_b = fp_from_seed(0.2);
    let fp_c = fp_from_seed(0.3);
    let fp_d = fp_from_seed(0.4);
    let fp_e = fp_from_seed(0.5);
    let fp_f = fp_from_seed(0.6);

    let mut records: Vec<PlannedRecord> = vec![
        fingerprint_record(&fp_a),
        fingerprint_record(&fp_b),
        fingerprint_record(&fp_c),
        fingerprint_record(&fp_d),
        fingerprint_record(&fp_e),
        fingerprint_record(&fp_f),
    ];

    // fp_A ─ 0.97 ─ fp_B — normal strong match
    let (_eg_ab, mut recs) = edge_record(&fp_a, &fp_b, edge_kind::STRONG_MATCH, 0.97);
    records.append(&mut recs);

    // fp_B ─ 0.90 ─ fp_C — normal strong match
    let (_eg_bc, mut recs) = edge_record(&fp_b, &fp_c, edge_kind::STRONG_MATCH, 0.90);
    records.append(&mut recs);

    // fp_C ─ 0.50 ─ fp_D — BELOW THRESHOLD at 0.85
    let (_eg_cd, mut recs) = edge_record(&fp_c, &fp_d, edge_kind::STRONG_MATCH, 0.50);
    records.append(&mut recs);

    // fp_A ─ 0.92 ─ fp_E — will be in excluded_edge_ids
    let (eg_ae, mut recs) = edge_record(&fp_a, &fp_e, edge_kind::STRONG_MATCH, 0.92);
    records.append(&mut recs);

    // fp_A ─ UserForbidden ─ fp_F — always skipped by the resolver
    let (_eg_af, mut recs) = edge_record(&fp_a, &fp_f, edge_kind::USER_FORBIDDEN, 0.99);
    records.append(&mut recs);

    // mn_1 on fp_A + fp_B (included normally)
    records.append(&mut mention_record("mn_1", "IMG_1", &[&fp_a, &fp_b]));
    // mn_2 on fp_B (will be in excluded_mention_ids)
    records.append(&mut mention_record("mn_2", "IMG_2", &[&fp_b]));
    // mn_3 on fp_C (included normally)
    records.append(&mut mention_record("mn_3", "IMG_3", &[&fp_c]));

    write_records(node.clone(), &records)
        .await
        .expect("writing synthetic graph");

    let mut excluded_edges = HashSet::new();
    excluded_edges.insert(eg_ae.clone());

    let mut excluded_mentions = HashSet::new();
    excluded_mentions.insert("mn_2".to_string());

    let spec = PersonaSpec {
        persona_id: "ps_comprehensive".to_string(),
        seed_fingerprint_ids: vec![fp_a.clone(), "fp_nonexistent".to_string()],
        threshold: 0.85,
        excluded_edge_ids: excluded_edges,
        excluded_mention_ids: excluded_mentions,
        included_mention_ids: HashSet::new(),
        identity_id: None,
    };

    let resolver = PersonaResolver::new(node.clone());
    let result = resolver.resolve(&spec).await.expect("resolve");

    let diag = result
        .diagnostics()
        .expect("every counter should fire → diagnostics must be present");

    // ── Missing seed ──
    assert!(
        diag.missing_seed_fingerprint_ids
            .iter()
            .any(|s| s == "fp_nonexistent"),
        "missing_seed_fingerprint_ids should contain fp_nonexistent; got {:?}",
        diag.missing_seed_fingerprint_ids,
    );

    // ── Below threshold ──
    assert!(
        diag.below_threshold_edge_count >= 1,
        "expected at least one below-threshold edge (fp_C→fp_D @ 0.50), got {}",
        diag.below_threshold_edge_count,
    );

    // ── Forbidden ──
    assert!(
        diag.forbidden_edge_count >= 1,
        "expected at least one forbidden edge (fp_A→fp_F UserForbidden), got {}",
        diag.forbidden_edge_count,
    );

    // ── Excluded edge ──
    assert!(
        diag.excluded_edge_count >= 1,
        "expected at least one excluded edge (fp_A→fp_E in excluded_edge_ids), got {}",
        diag.excluded_edge_count,
    );

    // ── Excluded mention ──
    assert!(
        diag.excluded_mention_count >= 1,
        "expected at least one excluded mention (mn_2 in excluded_mention_ids), got {}",
        diag.excluded_mention_count,
    );

    // And sanity-check the resolved cluster: fp_A, fp_B, fp_C reachable
    // via strong matches above threshold. fp_D below threshold. fp_E
    // excluded. fp_F forbidden.
    let fps = result.fingerprint_ids();
    assert!(fps.contains(&fp_a));
    assert!(fps.contains(&fp_b));
    assert!(fps.contains(&fp_c));
    assert!(
        !fps.contains(&fp_d),
        "fp_D should be cut off by below-threshold edge"
    );
    assert!(
        !fps.contains(&fp_e),
        "fp_E should be cut off by excluded edge"
    );
    assert!(
        !fps.contains(&fp_f),
        "fp_F should be cut off by forbidden edge"
    );
}
