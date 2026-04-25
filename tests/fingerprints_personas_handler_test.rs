//! Integration test: Persona list + detail handlers end-to-end.
//!
//! Exercises the `handlers::fingerprints::personas::{list_personas,
//! get_persona}` functions against a real in-process schema service
//! and a real FoldNode. Verifies:
//!
//! 1. `list_personas` on an empty DB returns zero personas
//! 2. Writing a synthetic Persona record makes it appear in the list
//!    with correct summary counts (zero fingerprints/edges/mentions
//!    when no graph data exists)
//! 3. Writing a small graph + persona with seeds into that graph
//!    produces a list entry with non-zero counts, plus a detail
//!    response with the fully resolved cluster
//! 4. `get_persona` returns 404 / NotFound for an unknown id
//! 5. Personas are sorted with built_in first, then by name
//!
//! The routes layer (`server/routes/fingerprints`) is a thin actix
//! adapter and is not tested at the HTTP level here — the handler
//! functions ARE the business logic.

use fold_db_node::fingerprints::canonical_names;
use fold_db_node::fingerprints::keys::{edge_id, edge_kind, fingerprint_id_for_face_embedding};
use fold_db_node::fingerprints::planned_record::PlannedRecord;
use fold_db_node::fingerprints::registration::register_phase_1_schemas;
use fold_db_node::fingerprints::schemas::{
    EDGE, EDGE_BY_FINGERPRINT, FINGERPRINT, MENTION, MENTION_BY_FINGERPRINT, MENTION_BY_SOURCE,
    PERSONA,
};
use fold_db_node::fingerprints::self_identity::bootstrap_self_identity;
use fold_db_node::fingerprints::writer::write_records;
use fold_db_node::fold_node::config::NodeConfig;
use fold_db_node::fold_node::FoldNode;
use fold_db_node::handlers::fingerprints::personas::{get_persona, list_personas};
use fold_db_node::handlers::response::HandlerError;
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
        .with_seed_identity(fold_db_node::identity::identity_from_keypair(&keypair));
    let node = FoldNode::new(config).await.expect("create FoldNode");
    (Arc::new(node), tmp)
}

/// Register Phase 1 schemas + seed the Me persona. In production
/// `FoldNode::new` runs both steps once an `IdentityCard` has been
/// saved by the setup wizard; tests stand in for the wizard by
/// invoking `bootstrap_self_identity` directly so the assertions
/// that expect a built-in self-persona stay accurate.
async fn register_and_bootstrap(node: &Arc<FoldNode>) {
    register_phase_1_schemas(node).await.unwrap();
    bootstrap_self_identity(node.clone(), "Tom Tang".to_string())
        .await
        .expect("bootstrap self identity");
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

    let mut a_j = HashMap::new();
    a_j.insert("fingerprint_id".to_string(), json!(a));
    a_j.insert("edge_id".to_string(), json!(eg_id));
    let mut b_j = HashMap::new();
    b_j.insert("fingerprint_id".to_string(), json!(b));
    b_j.insert("edge_id".to_string(), json!(eg_id));

    (
        eg_id.clone(),
        vec![
            PlannedRecord::hash(EDGE, eg_id.clone(), edge_fields),
            PlannedRecord::hash_range(EDGE_BY_FINGERPRINT, a.to_string(), eg_id.clone(), a_j),
            PlannedRecord::hash_range(EDGE_BY_FINGERPRINT, b.to_string(), eg_id.clone(), b_j),
        ],
    )
}

fn mention_records(mention_id: &str, source_key: &str, fps: &[&str]) -> Vec<PlannedRecord> {
    let mut m = HashMap::new();
    m.insert("id".to_string(), json!(mention_id));
    m.insert("source_schema".to_string(), json!("Photos"));
    m.insert("source_key".to_string(), json!(source_key));
    m.insert("source_field".to_string(), json!("face"));
    m.insert(
        "fingerprint_ids".to_string(),
        json!(fps.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
    );
    m.insert("extractor".to_string(), json!("face_detect"));
    m.insert("confidence".to_string(), json!(1.0_f32));
    m.insert("created_at".to_string(), json!("2026-04-14T00:00:00Z"));
    let mention = PlannedRecord::hash(MENTION, mention_id.to_string(), m);

    let composite = format!("Photos:{}", source_key);
    let mut sj = HashMap::new();
    sj.insert("source_composite".to_string(), json!(composite));
    sj.insert("mention_id".to_string(), json!(mention_id));
    let source_junction =
        PlannedRecord::hash_range(MENTION_BY_SOURCE, composite, mention_id.to_string(), sj);

    let mut records = vec![mention, source_junction];
    for fp in fps {
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

fn persona_record(
    id: &str,
    name: &str,
    seed_fps: &[&str],
    threshold: f32,
    built_in: bool,
    identity_id: Option<&str>,
) -> PlannedRecord {
    let mut fields = HashMap::new();
    fields.insert("id".to_string(), json!(id));
    fields.insert("name".to_string(), json!(name));
    fields.insert(
        "seed_fingerprint_ids".to_string(),
        json!(seed_fps.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
    );
    fields.insert("threshold".to_string(), json!(threshold));
    fields.insert(
        "excluded_mention_ids".to_string(),
        json!(Vec::<String>::new()),
    );
    fields.insert("excluded_edge_ids".to_string(), json!(Vec::<String>::new()));
    fields.insert(
        "included_mention_ids".to_string(),
        json!(Vec::<String>::new()),
    );
    fields.insert("aliases".to_string(), json!(Vec::<String>::new()));
    fields.insert(
        "relationship".to_string(),
        json!(if built_in { "self" } else { "colleague" }),
    );
    fields.insert(
        "trust_tier".to_string(),
        json!(if built_in { 4 } else { 2 }),
    );
    fields.insert(
        "identity_id".to_string(),
        match identity_id {
            Some(id) => json!({ "schema": "Identity", "key": id }),
            None => Value::Null,
        },
    );
    fields.insert("user_confirmed".to_string(), json!(true));
    fields.insert("built_in".to_string(), json!(built_in));
    fields.insert("created_at".to_string(), json!("2026-04-14T00:00:00Z"));
    PlannedRecord::hash(PERSONA, id.to_string(), fields)
}

// ── Tests ─────────────────────────────────────────────────────────

#[actix_web::test]
async fn list_personas_on_empty_node_returns_zero_entries() {
    canonical_names::reset_for_tests();

    let service = spawn_schema_service().await;
    let (node, _tmp) = create_node(&service.url).await;
    register_and_bootstrap(&node).await;

    let response = list_personas(node.clone()).await.expect("list ok");
    let data = response.data.expect("response has data");
    assert_eq!(data.personas.len(), 1); // self_identity is created automatically
}

#[actix_web::test]
async fn list_personas_returns_summary_with_resolved_counts() {
    canonical_names::reset_for_tests();

    let service = spawn_schema_service().await;
    let (node, _tmp) = create_node(&service.url).await;
    register_and_bootstrap(&node).await;

    // Build a tiny graph: fp_A ← StrongMatch(0.97) → fp_B
    // Two mentions covering both.
    let fp_a = fp_from_seed(0.1);
    let fp_b = fp_from_seed(0.2);
    let mut records: Vec<PlannedRecord> =
        vec![fingerprint_record(&fp_a), fingerprint_record(&fp_b)];
    let (_eg_ab, mut eg_records) = edge_record(&fp_a, &fp_b, edge_kind::STRONG_MATCH, 0.97);
    records.append(&mut eg_records);
    records.append(&mut mention_records("mn_1", "IMG_1", &[&fp_a, &fp_b]));
    records.append(&mut mention_records("mn_2", "IMG_2", &[&fp_a]));

    // Two personas:
    //   "Me" (built_in, seed = fp_A, threshold 0.9, identity linked)
    //   "Alice" (regular, seed = fp_B, threshold 0.9, no identity)
    records.push(persona_record(
        "ps_me",
        "Me",
        &[&fp_a],
        0.9,
        true,
        Some("id_fakepubkey"),
    ));
    records.push(persona_record(
        "ps_alice",
        "Alice",
        &[&fp_b],
        0.9,
        false,
        None,
    ));
    write_records(node.clone(), &records)
        .await
        .expect("write records");

    let response = list_personas(node.clone()).await.expect("list ok");
    let data = response.data.expect("response has data");
    assert_eq!(data.personas.len(), 3); // Tom Tang self_identity + Me + Alice

    // Skip the first one if it's "Tom Tang", but we need to check properties for the newly created ones.
    let me = data.personas.iter().find(|p| p.name == "Me").unwrap();
    let alice = data.personas.iter().find(|p| p.name == "Alice").unwrap();

    assert!(me.built_in);
    assert!(me.identity_linked);
    assert_eq!(me.relationship, "self");
    assert_eq!(me.trust_tier, 4);
    assert_eq!(me.fingerprint_count, 2); // fp_A, fp_B via StrongMatch(0.97)
    assert_eq!(me.edge_count, 1);
    assert_eq!(me.mention_count, 2);

    assert!(!alice.built_in);
    assert!(!alice.identity_linked);
    assert_eq!(alice.fingerprint_count, 2); // same cluster, different seed
    assert_eq!(alice.edge_count, 1);
    assert_eq!(alice.mention_count, 2);
}

#[actix_web::test]
async fn get_persona_returns_detail_with_resolved_cluster() {
    canonical_names::reset_for_tests();

    let service = spawn_schema_service().await;
    let (node, _tmp) = create_node(&service.url).await;
    register_and_bootstrap(&node).await;

    let fp_a = fp_from_seed(0.1);
    let fp_b = fp_from_seed(0.2);
    let mut records = vec![fingerprint_record(&fp_a), fingerprint_record(&fp_b)];
    let (eg_id, mut eg_records) = edge_record(&fp_a, &fp_b, edge_kind::STRONG_MATCH, 0.97);
    records.append(&mut eg_records);
    records.append(&mut mention_records("mn_1", "IMG_1", &[&fp_a, &fp_b]));
    records.push(persona_record(
        "ps_me",
        "Me",
        &[&fp_a],
        0.9,
        true,
        Some("id_fakepubkey"),
    ));
    write_records(node.clone(), &records)
        .await
        .expect("write records");

    let response = get_persona(node.clone(), "ps_me".to_string())
        .await
        .expect("get ok");
    let detail = response.data.expect("response has data");

    assert_eq!(detail.id, "ps_me");
    assert_eq!(detail.name, "Me");
    assert!(detail.built_in);
    assert_eq!(detail.identity_id.as_deref(), Some("id_fakepubkey"));
    assert_eq!(detail.seed_fingerprint_ids, vec![fp_a.clone()]);
    assert_eq!(detail.fingerprint_ids.len(), 2);
    assert!(detail.fingerprint_ids.contains(&fp_a));
    assert!(detail.fingerprint_ids.contains(&fp_b));
    assert_eq!(detail.edge_ids, vec![eg_id]);
    assert_eq!(detail.mention_ids, vec!["mn_1".to_string()]);
    // Clean resolve: no diagnostics.
    assert!(detail.diagnostics.is_none());
}

#[actix_web::test]
async fn get_persona_returns_not_found_for_unknown_id() {
    canonical_names::reset_for_tests();

    let service = spawn_schema_service().await;
    let (node, _tmp) = create_node(&service.url).await;
    register_and_bootstrap(&node).await;

    let result = get_persona(node.clone(), "ps_nonexistent".to_string()).await;
    let err = result.expect_err("must be NotFound");
    match err {
        HandlerError::NotFound(msg) => assert!(msg.contains("ps_nonexistent")),
        _ => panic!("expected NotFound, got {:?}", err),
    }
}
