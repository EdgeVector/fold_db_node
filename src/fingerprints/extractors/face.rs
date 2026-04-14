//! Face extractor — the Phase 1 face detection → fingerprint graph
//! bridge.
//!
//! Turns a list of detected faces on a single photo into an
//! `ExtractionPlan` describing every Fingerprint, Mention, Edge, and
//! junction record that should be written to materialize the faces
//! as nodes in the fingerprint graph.
//!
//! Input shape matches `fold_db::db_operations::native_index::face::FaceEmbedding`
//! (embedding: Vec<f32>, bbox: [f32; 4], confidence: f32), but this
//! module defines its own minimal `DetectedFace` struct so it stays
//! usable without the `face-detection` cargo feature — the planning
//! layer has no dependency on ONNX Runtime or the face detection
//! pipeline itself.
//!
//! ## What gets planned
//!
//! For each detected face:
//!
//! * **Fingerprint** record (kind=face_embedding). Content-keyed via
//!   `keys::fingerprint_id_for_face_embedding`. Two ingests of the
//!   same face (same photo, re-processed) dedupe automatically.
//!
//! For each distinct photo (schema, key):
//!
//! * **Mention** record — one per photo, listing every face
//!   fingerprint extracted from it. The `fingerprint_ids` array is
//!   the set of per-face Fingerprints found in this photo.
//! * **MentionBySource** junction row — enables
//!   "what was extracted from Photos/IMG_1234".
//! * **MentionByFingerprint** junction rows — one per face, enables
//!   "what photos contain this face fingerprint".
//! * **ExtractionStatus** — NotRun vs RanWithResults vs RanEmpty.
//!
//! For each pair of faces in the same photo (co-occurrence):
//!
//! * **Edge** record (kind=CoOccurrence, weight=0.2). Two people
//!   appearing in the same photo is weak evidence they know each
//!   other; the resolver only absorbs this via threshold settings
//!   that the user explicitly chose. Content-keyed so duplicate
//!   co-occurrence observations dedupe.
//! * **EdgeByFingerprint** junction rows — two per edge (one per
//!   endpoint), enabling "edges touching fp_X" reverse lookup.
//!
//! HNSW-similarity StrongMatch/MediumMatch edges are **not** produced
//! by this module — those land in the HNSW cache task (#7), which
//! compares new face fingerprints against the existing set and
//! produces similarity edges independently.

use serde_json::{json, Value};
use std::collections::HashMap;

use crate::fingerprints::keys::{
    edge_id, edge_kind, fingerprint_id_for_face_embedding, kind, mention_source_composite,
};
use crate::fingerprints::schemas::{
    EDGE, EDGE_BY_FINGERPRINT, EXTRACTION_STATUS, FINGERPRINT, MENTION, MENTION_BY_FINGERPRINT,
    MENTION_BY_SOURCE,
};

/// Minimal face input for the planner. A subset of the full
/// `FaceEmbedding` struct in fold_db's native_index module, kept
/// local so the planner compiles without the face-detection feature
/// flag.
#[derive(Debug, Clone)]
pub struct DetectedFace {
    pub embedding: Vec<f32>,
    pub bbox: [f32; 4],
    pub confidence: f32,
}

/// A record the extractor plans to write. Schema-agnostic — the
/// writer layer routes each variant to the correct mutation call.
#[derive(Debug, Clone)]
pub struct PlannedRecord {
    pub schema: &'static str,
    pub fields: HashMap<String, Value>,
    /// The value of the schema's declared hash_field (so the writer
    /// can pass the correct KeyValue to execute_mutation).
    pub hash_key: String,
    /// For HashRange schemas only — the value of the declared
    /// range_field.
    pub range_key: Option<String>,
}

impl PlannedRecord {
    fn hash(schema: &'static str, hash_key: String, fields: HashMap<String, Value>) -> Self {
        Self {
            schema,
            fields,
            hash_key,
            range_key: None,
        }
    }

    fn hash_range(
        schema: &'static str,
        hash_key: String,
        range_key: String,
        fields: HashMap<String, Value>,
    ) -> Self {
        Self {
            schema,
            fields,
            hash_key,
            range_key: Some(range_key),
        }
    }
}

/// The full planned output for one photo.
#[derive(Debug, Clone, Default)]
pub struct FaceExtractionPlan {
    pub records: Vec<PlannedRecord>,
    /// Face count the extractor ran against — useful for the writer
    /// layer's ExtractionStatus book-keeping and for tests.
    pub face_count: usize,
    /// Whether this plan represents a "ran, found nothing" state.
    /// Distinct from "extractor wasn't run at all" so the
    /// ExtractionStatus correctly records RanEmpty.
    pub ran_empty: bool,
}

impl FaceExtractionPlan {
    /// Filter a view of records belonging to a specific schema. Used
    /// by tests and the writer layer for schema-grouped routing.
    pub fn records_for_schema<'a>(
        &'a self,
        schema: &'static str,
    ) -> impl Iterator<Item = &'a PlannedRecord> + 'a {
        self.records.iter().filter(move |r| r.schema == schema)
    }

    pub fn count_for_schema(&self, schema: &'static str) -> usize {
        self.records_for_schema(schema).count()
    }
}

/// Build the full plan for a single photo's face extraction.
///
/// Deterministic — given the same inputs, produces identical output.
/// Uses the caller-provided `now_iso8601` so tests can inject a
/// stable timestamp.
pub fn plan_face_extraction(
    source_schema: &str,
    source_key: &str,
    faces: &[DetectedFace],
    mention_id: &str,           // externally generated UUID mn_<...>
    extraction_status_id: &str, // externally generated composite es_<...>
    now_iso8601: &str,
) -> FaceExtractionPlan {
    let mut plan = FaceExtractionPlan {
        records: Vec::new(),
        face_count: faces.len(),
        ran_empty: faces.is_empty(),
    };

    // The ExtractionStatus record is always written regardless of
    // whether faces were found. It's the signal that distinguishes
    // "never ran" from "ran, nothing here" from "ran, N found".
    let status_fields = extraction_status_fields(
        extraction_status_id,
        source_schema,
        source_key,
        faces.len(),
        now_iso8601,
    );
    plan.records.push(PlannedRecord::hash(
        EXTRACTION_STATUS,
        extraction_status_id.to_string(),
        status_fields,
    ));

    if faces.is_empty() {
        return plan;
    }

    // Per-face Fingerprint records + per-face MentionByFingerprint
    // junction rows. Collect fingerprint_ids for the Mention record.
    let mut fingerprint_ids: Vec<String> = Vec::with_capacity(faces.len());
    for face in faces {
        let fp_id = fingerprint_id_for_face_embedding(&face.embedding);
        fingerprint_ids.push(fp_id.clone());

        plan.records.push(PlannedRecord::hash(
            FINGERPRINT,
            fp_id.clone(),
            fingerprint_fields(&fp_id, &face.embedding, now_iso8601),
        ));
    }

    // Dedupe fingerprint_ids so the Mention record and junction rows
    // don't double-count the (rare) case of two identical embeddings
    // in one photo. Content-keyed Fingerprint dedup already collapses
    // those at the schema layer; we mirror that here so the plan
    // matches what will actually exist after the writer runs.
    let mut unique_fp_ids: Vec<String> = Vec::with_capacity(fingerprint_ids.len());
    for fp_id in &fingerprint_ids {
        if !unique_fp_ids.contains(fp_id) {
            unique_fp_ids.push(fp_id.clone());
        }
    }

    // One Mention record per photo, listing all face fingerprints.
    plan.records.push(PlannedRecord::hash(
        MENTION,
        mention_id.to_string(),
        mention_fields(
            mention_id,
            source_schema,
            source_key,
            &unique_fp_ids,
            now_iso8601,
        ),
    ));

    // MentionBySource junction — one row.
    let source_composite = mention_source_composite(source_schema, source_key);
    plan.records.push(PlannedRecord::hash_range(
        MENTION_BY_SOURCE,
        source_composite.clone(),
        mention_id.to_string(),
        mention_by_source_fields(&source_composite, mention_id),
    ));

    // MentionByFingerprint junction — one row per unique face.
    for fp_id in &unique_fp_ids {
        plan.records.push(PlannedRecord::hash_range(
            MENTION_BY_FINGERPRINT,
            fp_id.clone(),
            mention_id.to_string(),
            mention_by_fingerprint_fields(fp_id, mention_id),
        ));
    }

    // Co-occurrence edges: every unordered pair of distinct face
    // fingerprints in this photo. Weight is 0.2 (weak) because
    // "appears in the same photo" is weak evidence on its own.
    for i in 0..unique_fp_ids.len() {
        for j in (i + 1)..unique_fp_ids.len() {
            let a = &unique_fp_ids[i];
            let b = &unique_fp_ids[j];
            let eg_id = edge_id(a, b, edge_kind::CO_OCCURRENCE);

            plan.records.push(PlannedRecord::hash(
                EDGE,
                eg_id.clone(),
                edge_fields(
                    &eg_id,
                    a,
                    b,
                    edge_kind::CO_OCCURRENCE,
                    0.2,
                    mention_id,
                    now_iso8601,
                ),
            ));

            // Two EdgeByFingerprint junction rows per edge — one per
            // endpoint. Canonical order isn't required since both
            // rows go in regardless.
            plan.records.push(PlannedRecord::hash_range(
                EDGE_BY_FINGERPRINT,
                a.clone(),
                eg_id.clone(),
                edge_by_fingerprint_fields(a, &eg_id),
            ));
            plan.records.push(PlannedRecord::hash_range(
                EDGE_BY_FINGERPRINT,
                b.clone(),
                eg_id.clone(),
                edge_by_fingerprint_fields(b, &eg_id),
            ));
        }
    }

    plan
}

// ── Field-builder helpers ─────────────────────────────────────────

fn fingerprint_fields(fp_id: &str, embedding: &[f32], now_iso8601: &str) -> HashMap<String, Value> {
    let mut m = HashMap::new();
    m.insert("id".to_string(), json!(fp_id));
    m.insert("kind".to_string(), json!(kind::FACE_EMBEDDING));
    // Value is serialized as a JSON array of floats. The schema
    // declares this field as `Any`; the resolver consumes it via the
    // Fingerprint's typed reader.
    m.insert("value".to_string(), json!(embedding));
    m.insert("first_seen".to_string(), json!(now_iso8601));
    m.insert("last_seen".to_string(), json!(now_iso8601));
    m
}

fn mention_fields(
    mention_id: &str,
    source_schema: &str,
    source_key: &str,
    fingerprint_ids: &[String],
    now_iso8601: &str,
) -> HashMap<String, Value> {
    let mut m = HashMap::new();
    m.insert("id".to_string(), json!(mention_id));
    m.insert("source_schema".to_string(), json!(source_schema));
    m.insert("source_key".to_string(), json!(source_key));
    m.insert("source_field".to_string(), json!("face"));
    m.insert("fingerprint_ids".to_string(), json!(fingerprint_ids));
    m.insert("extractor".to_string(), json!("face_detect"));
    // We don't propagate per-face confidence into the Mention because
    // the Mention represents a photo-level extraction, not a single
    // face. Per-face confidence lives on the individual Edge
    // weights produced by the HNSW similarity pass.
    m.insert("confidence".to_string(), json!(1.0_f32));
    m.insert("created_at".to_string(), json!(now_iso8601));
    m
}

fn edge_fields(
    eg_id: &str,
    a: &str,
    b: &str,
    kind: &str,
    weight: f32,
    evidence_mention_id: &str,
    now_iso8601: &str,
) -> HashMap<String, Value> {
    // Canonicalize (a, b) so the fields match the order used in
    // edge_id(). This keeps the rows self-consistent: any caller
    // reading the Edge back can rely on a <= b.
    let (first, second) = if a <= b { (a, b) } else { (b, a) };
    let mut m = HashMap::new();
    m.insert("id".to_string(), json!(eg_id));
    m.insert("a".to_string(), json!(first));
    m.insert("b".to_string(), json!(second));
    m.insert("kind".to_string(), json!(kind));
    m.insert("weight".to_string(), json!(weight));
    m.insert(
        "evidence_mention_ids".to_string(),
        json!([evidence_mention_id]),
    );
    m.insert("created_at".to_string(), json!(now_iso8601));
    m
}

fn edge_by_fingerprint_fields(fp_id: &str, eg_id: &str) -> HashMap<String, Value> {
    let mut m = HashMap::new();
    m.insert("fingerprint_id".to_string(), json!(fp_id));
    m.insert("edge_id".to_string(), json!(eg_id));
    m
}

fn mention_by_source_fields(source_composite: &str, mention_id: &str) -> HashMap<String, Value> {
    let mut m = HashMap::new();
    m.insert("source_composite".to_string(), json!(source_composite));
    m.insert("mention_id".to_string(), json!(mention_id));
    m
}

fn mention_by_fingerprint_fields(fp_id: &str, mention_id: &str) -> HashMap<String, Value> {
    let mut m = HashMap::new();
    m.insert("fingerprint_id".to_string(), json!(fp_id));
    m.insert("mention_id".to_string(), json!(mention_id));
    m
}

fn extraction_status_fields(
    id: &str,
    source_schema: &str,
    source_key: &str,
    fingerprint_count: usize,
    now_iso8601: &str,
) -> HashMap<String, Value> {
    let mut m = HashMap::new();
    m.insert("id".to_string(), json!(id));
    m.insert("source_schema".to_string(), json!(source_schema));
    m.insert("source_key".to_string(), json!(source_key));
    m.insert("extractor".to_string(), json!("face_detect"));
    let status = if fingerprint_count > 0 {
        "RanWithResults"
    } else {
        "RanEmpty"
    };
    m.insert("status".to_string(), json!(status));
    m.insert("fingerprint_count".to_string(), json!(fingerprint_count));
    m.insert("ran_at".to_string(), json!(now_iso8601));
    m.insert("model_version".to_string(), json!(Value::Null));
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA: &str = "Photos";
    const KEY: &str = "IMG_1234";
    const MENTION_ID: &str = "mn_testmention";
    const STATUS_ID: &str = "es_Photos:IMG_1234:face_detect";
    const NOW: &str = "2026-04-14T12:00:00Z";

    fn face(seed: f32) -> DetectedFace {
        DetectedFace {
            embedding: vec![seed; 512],
            bbox: [0.1, 0.2, 0.3, 0.4],
            confidence: 0.95,
        }
    }

    // ── Zero-face case ──────────────────────────────────────────

    #[test]
    fn zero_faces_plan_has_only_extraction_status() {
        let plan = plan_face_extraction(SCHEMA, KEY, &[], MENTION_ID, STATUS_ID, NOW);
        assert_eq!(plan.face_count, 0);
        assert!(plan.ran_empty);
        assert_eq!(plan.records.len(), 1);
        assert_eq!(plan.records[0].schema, EXTRACTION_STATUS);
        assert_eq!(
            plan.records[0].fields.get("status").unwrap(),
            &json!("RanEmpty")
        );
    }

    // ── Single-face case ────────────────────────────────────────

    #[test]
    fn single_face_plan_writes_fingerprint_mention_both_junctions_and_status() {
        let faces = vec![face(0.1)];
        let plan = plan_face_extraction(SCHEMA, KEY, &faces, MENTION_ID, STATUS_ID, NOW);

        assert_eq!(plan.face_count, 1);
        assert!(!plan.ran_empty);

        // Expected records: 1 ExtractionStatus + 1 Fingerprint + 1 Mention
        // + 1 MentionBySource + 1 MentionByFingerprint + 0 Edge (single face, no pair)
        assert_eq!(plan.count_for_schema(EXTRACTION_STATUS), 1);
        assert_eq!(plan.count_for_schema(FINGERPRINT), 1);
        assert_eq!(plan.count_for_schema(MENTION), 1);
        assert_eq!(plan.count_for_schema(MENTION_BY_SOURCE), 1);
        assert_eq!(plan.count_for_schema(MENTION_BY_FINGERPRINT), 1);
        assert_eq!(plan.count_for_schema(EDGE), 0);
        assert_eq!(plan.count_for_schema(EDGE_BY_FINGERPRINT), 0);
    }

    // ── Two-face case (one pair → one edge + two junction rows) ─

    #[test]
    fn two_faces_plan_writes_one_cooccurrence_edge_and_two_junction_rows() {
        let faces = vec![face(0.1), face(0.2)];
        let plan = plan_face_extraction(SCHEMA, KEY, &faces, MENTION_ID, STATUS_ID, NOW);

        assert_eq!(plan.count_for_schema(FINGERPRINT), 2);
        assert_eq!(plan.count_for_schema(MENTION), 1);
        assert_eq!(plan.count_for_schema(MENTION_BY_FINGERPRINT), 2);
        assert_eq!(plan.count_for_schema(EDGE), 1);
        assert_eq!(plan.count_for_schema(EDGE_BY_FINGERPRINT), 2);
    }

    // ── Three-face case (3 pairs → 3 edges + 6 junction rows) ──

    #[test]
    fn three_faces_plan_writes_three_edges_and_six_edge_junction_rows() {
        let faces = vec![face(0.1), face(0.2), face(0.3)];
        let plan = plan_face_extraction(SCHEMA, KEY, &faces, MENTION_ID, STATUS_ID, NOW);

        assert_eq!(plan.count_for_schema(FINGERPRINT), 3);
        // C(3, 2) = 3 unordered pairs → 3 edges
        assert_eq!(plan.count_for_schema(EDGE), 3);
        // 2 junction rows per edge
        assert_eq!(plan.count_for_schema(EDGE_BY_FINGERPRINT), 6);
    }

    // ── Cooccurrence edge weight ────────────────────────────────

    #[test]
    fn cooccurrence_edge_weight_is_weak() {
        let faces = vec![face(0.1), face(0.2)];
        let plan = plan_face_extraction(SCHEMA, KEY, &faces, MENTION_ID, STATUS_ID, NOW);
        let edge = plan
            .records_for_schema(EDGE)
            .next()
            .expect("edge record must exist");
        let weight = edge.fields.get("weight").unwrap().as_f64().unwrap();
        assert!(
            (weight - 0.2).abs() < 1e-6,
            "weight should be 0.2, got {}",
            weight
        );
        assert_eq!(
            edge.fields.get("kind").unwrap(),
            &json!(edge_kind::CO_OCCURRENCE)
        );
    }

    // ── Edge endpoint canonicalization ──────────────────────────

    #[test]
    fn edge_fields_have_canonical_endpoint_order() {
        let faces = vec![face(0.1), face(0.2)];
        let plan = plan_face_extraction(SCHEMA, KEY, &faces, MENTION_ID, STATUS_ID, NOW);
        let edge = plan
            .records_for_schema(EDGE)
            .next()
            .expect("edge record must exist");
        let a = edge.fields.get("a").unwrap().as_str().unwrap();
        let b = edge.fields.get("b").unwrap().as_str().unwrap();
        assert!(a <= b, "a ({}) should sort <= b ({})", a, b);
    }

    // ── Dedup of identical embeddings in the same photo ─────────

    #[test]
    fn identical_embeddings_in_same_photo_dedupe_to_one_fingerprint() {
        let faces = vec![face(0.5), face(0.5)]; // same embedding
        let plan = plan_face_extraction(SCHEMA, KEY, &faces, MENTION_ID, STATUS_ID, NOW);

        // The planner writes the Fingerprint record twice (which the
        // schema layer will dedupe via content-addressed primary key)
        // — but the Mention and junction rows should reference the
        // fingerprint only once, since we dedupe fingerprint_ids
        // before writing the Mention.
        assert_eq!(plan.count_for_schema(FINGERPRINT), 2); // two writes, both same key
        assert_eq!(plan.count_for_schema(MENTION_BY_FINGERPRINT), 1);
        // Identical fingerprints → only one "pair" is trivially
        // (same, same), which we skip via i < j iteration → zero edges.
        assert_eq!(plan.count_for_schema(EDGE), 0);
    }

    // ── Mention body shape ──────────────────────────────────────

    #[test]
    fn mention_record_lists_unique_fingerprint_ids() {
        let faces = vec![face(0.1), face(0.2)];
        let plan = plan_face_extraction(SCHEMA, KEY, &faces, MENTION_ID, STATUS_ID, NOW);
        let mention = plan
            .records_for_schema(MENTION)
            .next()
            .expect("mention must exist");
        let fp_ids = mention
            .fields
            .get("fingerprint_ids")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(fp_ids.len(), 2);
        assert_eq!(mention.fields.get("source_schema").unwrap(), &json!(SCHEMA));
        assert_eq!(mention.fields.get("source_key").unwrap(), &json!(KEY));
        assert_eq!(
            mention.fields.get("extractor").unwrap(),
            &json!("face_detect")
        );
    }

    // ── ExtractionStatus body shape ─────────────────────────────

    #[test]
    fn extraction_status_with_faces_is_ran_with_results() {
        let faces = vec![face(0.1)];
        let plan = plan_face_extraction(SCHEMA, KEY, &faces, MENTION_ID, STATUS_ID, NOW);
        let status = plan
            .records_for_schema(EXTRACTION_STATUS)
            .next()
            .expect("status must exist");
        assert_eq!(
            status.fields.get("status").unwrap(),
            &json!("RanWithResults")
        );
        assert_eq!(status.fields.get("fingerprint_count").unwrap(), &json!(1));
    }

    // ── Determinism ─────────────────────────────────────────────

    #[test]
    fn planning_is_deterministic() {
        let faces = vec![face(0.1), face(0.2), face(0.3)];
        let plan_a = plan_face_extraction(SCHEMA, KEY, &faces, MENTION_ID, STATUS_ID, NOW);
        let plan_b = plan_face_extraction(SCHEMA, KEY, &faces, MENTION_ID, STATUS_ID, NOW);
        assert_eq!(plan_a.records.len(), plan_b.records.len());
        for (a, b) in plan_a.records.iter().zip(plan_b.records.iter()) {
            assert_eq!(a.schema, b.schema);
            assert_eq!(a.hash_key, b.hash_key);
            assert_eq!(a.range_key, b.range_key);
        }
    }

    // ── Hash key consistency ────────────────────────────────────

    #[test]
    fn fingerprint_hash_key_equals_id_field() {
        let faces = vec![face(0.1)];
        let plan = plan_face_extraction(SCHEMA, KEY, &faces, MENTION_ID, STATUS_ID, NOW);
        let fp = plan
            .records_for_schema(FINGERPRINT)
            .next()
            .expect("fingerprint must exist");
        let id_field = fp.fields.get("id").unwrap().as_str().unwrap();
        assert_eq!(fp.hash_key, id_field);
        assert!(fp.hash_key.starts_with("fp_"));
    }
}
