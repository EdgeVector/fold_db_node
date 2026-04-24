//! Integration test: Persona resolver traversal against a real
//! fingerprint graph written via the writer layer.
//!
//! Builds on the writer test pattern but focuses on the traversal
//! algorithm rather than the write path:
//!
//! 1. Register the twelve Phase 1 schemas through the schema service
//! 2. Hand-craft a synthetic graph via the writer (fingerprints,
//!    edges of varying weights and kinds, mentions)
//! 3. Run PersonaResolver::resolve against a PersonaSpec
//! 4. Assert the expected fingerprint/edge/mention sets come back
//! 5. Assert ResolveResult diagnostics fire correctly for
//!    below-threshold, forbidden, excluded, and missing-seed cases
//!
//! The in-process schema-service setup is lifted from the existing
//! tests/fingerprints_registration_test.rs and
//! tests/fingerprints_writer_test.rs to keep the pattern consistent.

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

// ── Synthetic graph builder ───────────────────────────────────────
//
// Build records that form a small graph with controlled structure:
//
//    fp_A ── StrongMatch (w=0.97) ── fp_B ── StrongMatch (w=0.90) ── fp_C
//     │
//     └─ MediumMatch (w=0.87) ─── fp_D
//
//    fp_E (isolated, no edges)
//
// With mentions:
//    mn_1 references fp_A, fp_B   (photo 1)
//    mn_2 references fp_B, fp_C   (photo 2)
//    mn_3 references fp_D         (photo 3)
//    mn_4 references fp_E         (photo 4)

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
    source_schema: &str,
    source_key: &str,
    fingerprint_ids: &[&str],
) -> Vec<PlannedRecord> {
    let mut fields = HashMap::new();
    fields.insert("id".to_string(), json!(mention_id));
    fields.insert("source_schema".to_string(), json!(source_schema));
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

    // MentionBySource
    let composite = format!("{}:{}", source_schema, source_key);
    let mut source_junction = HashMap::new();
    source_junction.insert("source_composite".to_string(), json!(composite));
    source_junction.insert("mention_id".to_string(), json!(mention_id));
    let source_j = PlannedRecord::hash_range(
        MENTION_BY_SOURCE,
        composite,
        mention_id.to_string(),
        source_junction,
    );

    // MentionByFingerprint per fingerprint
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

struct GraphFixture {
    fp_a: String,
    fp_b: String,
    fp_c: String,
    fp_d: String,
    fp_e: String,
    eg_ab: String,
    eg_bc: String,
    eg_ad: String,
}

async fn build_graph(node: Arc<FoldNode>) -> GraphFixture {
    let fp_a = fp_from_seed(0.1);
    let fp_b = fp_from_seed(0.2);
    let fp_c = fp_from_seed(0.3);
    let fp_d = fp_from_seed(0.4);
    let fp_e = fp_from_seed(0.5);

    let mut records: Vec<PlannedRecord> = vec![
        fingerprint_record(&fp_a),
        fingerprint_record(&fp_b),
        fingerprint_record(&fp_c),
        fingerprint_record(&fp_d),
        fingerprint_record(&fp_e),
    ];

    let (eg_ab, mut recs) = edge_record(&fp_a, &fp_b, edge_kind::STRONG_MATCH, 0.97);
    records.append(&mut recs);

    let (eg_bc, mut recs) = edge_record(&fp_b, &fp_c, edge_kind::STRONG_MATCH, 0.90);
    records.append(&mut recs);

    let (eg_ad, mut recs) = edge_record(&fp_a, &fp_d, edge_kind::STRONG_MATCH, 0.87);
    records.append(&mut recs);

    records.append(&mut mention_record(
        "mn_1",
        "Photos",
        "IMG_1",
        &[&fp_a, &fp_b],
    ));
    records.append(&mut mention_record(
        "mn_2",
        "Photos",
        "IMG_2",
        &[&fp_b, &fp_c],
    ));
    records.append(&mut mention_record("mn_3", "Photos", "IMG_3", &[&fp_d]));
    records.append(&mut mention_record("mn_4", "Photos", "IMG_4", &[&fp_e]));

    write_records(node.clone(), &records)
        .await
        .expect("writing synthetic graph");

    GraphFixture {
        fp_a,
        fp_b,
        fp_c,
        fp_d,
        fp_e,
        eg_ab,
        eg_bc,
        eg_ad,
    }
}

// ── Tests ─────────────────────────────────────────────────────────

/// Threshold 0.85 — everything above is reachable from fp_A.
/// Expected cluster: {fp_A, fp_B, fp_C, fp_D} (not fp_E which has
/// no edges).
#[actix_web::test]
async fn resolves_full_connected_component_above_threshold() {
    canonical_names::reset_for_tests();

    let service = spawn_schema_service().await;
    let (node, _tmp) = create_node(&service.url).await;
    register_phase_1_schemas(&node).await.unwrap();

    let g = build_graph(node.clone()).await;

    let spec = PersonaSpec {
        persona_id: "ps_test_full".to_string(),
        seed_fingerprint_ids: vec![g.fp_a.clone()],
        threshold: 0.85,
        excluded_edge_ids: HashSet::new(),
        excluded_mention_ids: HashSet::new(),
        included_mention_ids: HashSet::new(),
        identity_id: None,
    };

    let resolver = PersonaResolver::new(node.clone());
    let result = resolver.resolve(&spec).await.expect("resolve");

    let fps = result.fingerprint_ids();
    assert!(fps.contains(&g.fp_a));
    assert!(fps.contains(&g.fp_b));
    assert!(fps.contains(&g.fp_c));
    assert!(fps.contains(&g.fp_d));
    assert!(
        !fps.contains(&g.fp_e),
        "isolated fp_E must not be reachable"
    );
    assert_eq!(fps.len(), 4);

    let edges = result.edge_ids();
    assert!(edges.contains(&g.eg_ab));
    assert!(edges.contains(&g.eg_bc));
    assert!(edges.contains(&g.eg_ad));
    assert_eq!(edges.len(), 3);

    // Mentions from every visited fingerprint:
    // fp_a → mn_1; fp_b → mn_1, mn_2; fp_c → mn_2; fp_d → mn_3
    // → {mn_1, mn_2, mn_3}
    let mns = result.mention_ids();
    assert!(mns.contains("mn_1"));
    assert!(mns.contains("mn_2"));
    assert!(mns.contains("mn_3"));
    assert!(!mns.contains("mn_4"));
    assert_eq!(mns.len(), 3);

    assert!(
        result.is_clean(),
        "expected clean resolve, got {:?}",
        result.diagnostics()
    );
}

/// Threshold 0.95 — only the strongest edge qualifies, so only
/// fp_A and fp_B are reachable. The 0.90 and 0.87 edges get
/// filtered out and counted in diagnostics.below_threshold_edge_count.
#[actix_web::test]
async fn threshold_filters_below_weight_edges_with_diagnostics() {
    canonical_names::reset_for_tests();

    let service = spawn_schema_service().await;
    let (node, _tmp) = create_node(&service.url).await;
    register_phase_1_schemas(&node).await.unwrap();

    let g = build_graph(node.clone()).await;

    let spec = PersonaSpec {
        persona_id: "ps_test_high_thresh".to_string(),
        seed_fingerprint_ids: vec![g.fp_a.clone()],
        threshold: 0.95,
        excluded_edge_ids: HashSet::new(),
        excluded_mention_ids: HashSet::new(),
        included_mention_ids: HashSet::new(),
        identity_id: None,
    };

    let resolver = PersonaResolver::new(node.clone());
    let result = resolver.resolve(&spec).await.expect("resolve");

    let fps = result.fingerprint_ids();
    assert!(fps.contains(&g.fp_a));
    assert!(fps.contains(&g.fp_b));
    assert_eq!(
        fps.len(),
        2,
        "only fp_A and fp_B should be reachable at threshold 0.95"
    );

    // The 0.87 edge (A—D) was filtered. The 0.90 edge (B—C) was filtered
    // from the B side of the traversal. Both bumped
    // below_threshold_edge_count.
    let diag = result
        .diagnostics()
        .expect("expected below-threshold diagnostics");
    assert!(
        diag.below_threshold_edge_count >= 2,
        "expected >=2 below-threshold hits, got {}",
        diag.below_threshold_edge_count
    );
}

/// An `excluded_edge_ids` entry surgically blocks traversal through
/// a specific edge. Expect fp_D to fall off when the A-D edge is
/// excluded, even though it otherwise qualifies.
#[actix_web::test]
async fn excluded_edge_id_blocks_traversal_and_is_counted() {
    canonical_names::reset_for_tests();

    let service = spawn_schema_service().await;
    let (node, _tmp) = create_node(&service.url).await;
    register_phase_1_schemas(&node).await.unwrap();

    let g = build_graph(node.clone()).await;

    let mut excluded = HashSet::new();
    excluded.insert(g.eg_ad.clone());

    let spec = PersonaSpec {
        persona_id: "ps_test_exclude".to_string(),
        seed_fingerprint_ids: vec![g.fp_a.clone()],
        threshold: 0.85,
        excluded_edge_ids: excluded,
        excluded_mention_ids: HashSet::new(),
        included_mention_ids: HashSet::new(),
        identity_id: None,
    };

    let resolver = PersonaResolver::new(node.clone());
    let result = resolver.resolve(&spec).await.expect("resolve");

    let fps = result.fingerprint_ids();
    assert!(fps.contains(&g.fp_a));
    assert!(fps.contains(&g.fp_b));
    assert!(fps.contains(&g.fp_c));
    assert!(
        !fps.contains(&g.fp_d),
        "fp_D must be unreachable with A-D edge excluded"
    );

    let diag = result
        .diagnostics()
        .expect("expected excluded-edge diagnostics");
    assert!(diag.excluded_edge_count >= 1);
}

/// Missing seed fingerprint → the seed gets listed in
/// diagnostics.missing_seed_fingerprint_ids, but the resolve still
/// succeeds and returns an empty cluster.
#[actix_web::test]
async fn missing_seed_fingerprint_is_diagnostic_not_error() {
    canonical_names::reset_for_tests();

    let service = spawn_schema_service().await;
    let (node, _tmp) = create_node(&service.url).await;
    register_phase_1_schemas(&node).await.unwrap();

    let _g = build_graph(node.clone()).await;

    let spec = PersonaSpec {
        persona_id: "ps_test_missing".to_string(),
        seed_fingerprint_ids: vec!["fp_nonexistent".to_string()],
        threshold: 0.85,
        excluded_edge_ids: HashSet::new(),
        excluded_mention_ids: HashSet::new(),
        included_mention_ids: HashSet::new(),
        identity_id: None,
    };

    let resolver = PersonaResolver::new(node.clone());
    let result = resolver.resolve(&spec).await.expect("resolve must succeed");

    assert!(result.fingerprint_ids().is_empty());
    let diag = result
        .diagnostics()
        .expect("expected diagnostics for missing seed");
    assert_eq!(
        diag.missing_seed_fingerprint_ids,
        vec!["fp_nonexistent".to_string()]
    );
}

/// included_mention_ids adds mentions that wouldn't otherwise be
/// reached by the traversal — for example, a mention tied to fp_E
/// (isolated) when the Persona's seed is fp_A.
#[actix_web::test]
async fn included_mention_ids_are_added_to_result() {
    canonical_names::reset_for_tests();

    let service = spawn_schema_service().await;
    let (node, _tmp) = create_node(&service.url).await;
    register_phase_1_schemas(&node).await.unwrap();

    let g = build_graph(node.clone()).await;

    let mut included = HashSet::new();
    included.insert("mn_4".to_string()); // isolated mention on fp_E

    let spec = PersonaSpec {
        persona_id: "ps_test_include".to_string(),
        seed_fingerprint_ids: vec![g.fp_a.clone()],
        threshold: 0.85,
        excluded_edge_ids: HashSet::new(),
        excluded_mention_ids: HashSet::new(),
        included_mention_ids: included,
        identity_id: None,
    };

    let resolver = PersonaResolver::new(node.clone());
    let result = resolver.resolve(&spec).await.expect("resolve");

    let mns = result.mention_ids();
    assert!(
        mns.contains("mn_4"),
        "explicitly included mention must appear"
    );
    // Plus the normal traversal mentions.
    assert!(mns.contains("mn_1"));
    assert!(mns.contains("mn_2"));
    assert!(mns.contains("mn_3"));
}

/// excluded_mention_ids removes a specific mention from the result
/// and increments the diagnostic counter.
#[actix_web::test]
async fn excluded_mention_ids_are_filtered_with_diagnostics() {
    canonical_names::reset_for_tests();

    let service = spawn_schema_service().await;
    let (node, _tmp) = create_node(&service.url).await;
    register_phase_1_schemas(&node).await.unwrap();

    let g = build_graph(node.clone()).await;

    let mut excluded = HashSet::new();
    excluded.insert("mn_2".to_string());

    let spec = PersonaSpec {
        persona_id: "ps_test_mention_exclude".to_string(),
        seed_fingerprint_ids: vec![g.fp_a.clone()],
        threshold: 0.85,
        excluded_edge_ids: HashSet::new(),
        excluded_mention_ids: excluded,
        included_mention_ids: HashSet::new(),
        identity_id: None,
    };

    let resolver = PersonaResolver::new(node.clone());
    let result = resolver.resolve(&spec).await.expect("resolve");

    let mns = result.mention_ids();
    assert!(!mns.contains("mn_2"), "mn_2 was excluded");
    assert!(mns.contains("mn_1"));
    assert!(mns.contains("mn_3"));

    let diag = result
        .diagnostics()
        .expect("expected diagnostics for excluded mention");
    // mn_2 is referenced by fp_B and fp_C so it gets visited twice
    // and filtered twice.
    assert!(diag.excluded_mention_count >= 1);
}
