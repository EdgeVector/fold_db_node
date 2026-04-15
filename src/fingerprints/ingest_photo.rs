//! Photo → fingerprint ingestion helper.
//!
//! Ties together [`crate::fingerprints::extractors::face::plan_face_extraction`]
//! and [`crate::fingerprints::writer::write_records`] so callers can
//! turn a list of detected faces on a single photo into persisted
//! Fingerprint + Mention + Edge + junction + ExtractionStatus records
//! in one async call.
//!
//! This is the write-side glue used by two call sites:
//!
//! 1. **Live ingestion** (future) — when a new photo lands and the
//!    native-index face detector runs over it, the detected faces are
//!    passed straight into [`ingest_photo_faces`] to materialize
//!    Phase 1 records.
//! 2. **Migration** (today) — the `migrate_photos` binary iterates
//!    over existing Photo records and calls [`ingest_photo_faces`]
//!    with faces detected against the saved image bytes. See
//!    `src/bin/migrate_photos.rs`.
//!
//! ## Idempotency
//!
//! - `Fingerprint` and `Edge` records are content-addressed by key,
//!   so re-running against the same photo + same detected faces is
//!   a no-op at the schema layer.
//! - `Mention` and `ExtractionStatus` records are keyed deterministically
//!   on `(source_schema, source_key, extractor_name)` so a re-run
//!   overwrites the previous mention / status in place instead of
//!   accumulating duplicates. This is a departure from the general
//!   "Mention is per-instance with a UUID" convention — deliberate,
//!   because migration runs must be safe to retry.
//!
//! ## Errors
//!
//! Propagates the first error from the underlying writer. On success,
//! returns a [`IngestionOutcome`] summarizing the face count and
//! whether the extractor ran empty (for observability).

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::fingerprints::extractors::face::{plan_face_extraction, DetectedFace};
use crate::fingerprints::face_ann_cache::{cache, ensure_cache_ready, FaceEmbedding};
use crate::fingerprints::keys::{edge_id, edge_kind, fingerprint_id_for_face_embedding};
use crate::fingerprints::planned_record::PlannedRecord;
use crate::fingerprints::schemas::{EDGE, EDGE_BY_FINGERPRINT};
use crate::fingerprints::writer::{write_records, WriteOutcome};
use crate::fold_node::FoldNode;
use fold_db::error::FoldDbResult;

/// Cosine similarity floor for emitting a face-to-face similarity
/// edge. Below this, the two faces are considered independent
/// observations with no persona-level relationship.
const MIN_SIMILARITY_EDGE: f32 = 0.85;

/// Cosine similarity cutoff that separates StrongMatch from
/// MediumMatch. At or above this the pair is treated as "basically
/// the same face"; between `MIN_SIMILARITY_EDGE` and this, the pair
/// is a softer signal that still clusters at the default threshold
/// but lets the user split them with a slider nudge.
const STRONG_MATCH_CUTOFF: f32 = 0.95;

/// Top-K neighbors to query from the cache per new face. K=5 is
/// enough to cover the "same face across a burst of photos" case
/// without blowing up the edge count; rarely-similar faces produce
/// fewer hits because the threshold filter cuts them.
const NEIGHBOR_QUERY_K: usize = 5;

/// Summary of a single-photo ingestion pass.
#[derive(Debug, Clone, Default)]
pub struct IngestionOutcome {
    /// Total number of records written (fingerprints, mentions,
    /// edges, junctions, ExtractionStatus).
    pub records_written: usize,
    /// Number of faces the extractor saw on this photo.
    pub face_count: usize,
    /// True when the extractor ran but found zero faces. Useful for
    /// distinguishing "we haven't processed this photo yet" from "we
    /// processed it and it contains no faces" at the UI layer.
    pub ran_empty: bool,
}

impl From<WriteOutcome> for IngestionOutcome {
    fn from(outcome: WriteOutcome) -> Self {
        Self {
            records_written: outcome.total(),
            face_count: 0,
            ran_empty: false,
        }
    }
}

/// Build a deterministic `mn_<hash>` mention id from the photo's
/// `(schema, key, extractor)` tuple. Using a deterministic id (rather
/// than a fresh UUID) makes migration re-runs idempotent: the second
/// run overwrites the first mention in place instead of creating a
/// duplicate row.
pub fn deterministic_mention_id(
    source_schema: &str,
    source_key: &str,
    extractor_name: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"Mention");
    hasher.update(source_schema.as_bytes());
    hasher.update(b":");
    hasher.update(source_key.as_bytes());
    hasher.update(b":");
    hasher.update(extractor_name.as_bytes());
    let full = format!("{:x}", hasher.finalize());
    // Truncate to 32 hex chars — still has 128 bits of entropy, more
    // than enough to avoid collisions across any realistic photo
    // corpus. Matches the visual weight of a typical UUID hex form.
    format!("mn_{}", &full[..32])
}

/// Build the composite ExtractionStatus key. Mirrors the convention
/// used elsewhere in the fingerprint subsystem: a colon-joined
/// `es_<schema>:<key>:<extractor>` triple that is both deterministic
/// and human-readable. A re-run with the same inputs produces the
/// same key and therefore overwrites the previous status in place.
pub fn extraction_status_id(source_schema: &str, source_key: &str, extractor_name: &str) -> String {
    format!("es_{source_schema}:{source_key}:{extractor_name}")
}

/// Plan and persist Phase 1 fingerprint records for the faces detected
/// on a single photo.
///
/// # Arguments
///
/// - `node` — target FoldNode. Canonical-names must already be
///   initialized via [`crate::fingerprints::registration::register_phase_1_schemas`].
/// - `source_schema` — descriptive name of the schema the photo lives
///   in, e.g. `"Photos"`. This is the schema that the Mention will
///   reference; it is NOT one of the Phase 1 schemas.
/// - `source_key` — primary key of the Photo record on the source
///   schema, e.g. `"IMG_1234"`.
/// - `faces` — face embeddings detected against this photo. Pass an
///   empty slice if extraction ran with no hits — the ExtractionStatus
///   record will still be written so the UI can distinguish "not yet
///   processed" from "processed, no faces found".
/// - `now_iso8601` — timestamp to embed in every created record.
///   Typically the caller's wall-clock.
///
/// # Returns
///
/// An [`IngestionOutcome`] summarizing the run.
pub async fn ingest_photo_faces(
    node: Arc<FoldNode>,
    source_schema: &str,
    source_key: &str,
    faces: &[DetectedFace],
    now_iso8601: &str,
) -> FoldDbResult<IngestionOutcome> {
    const EXTRACTOR_NAME: &str = "face_detect";

    let mention_id = deterministic_mention_id(source_schema, source_key, EXTRACTOR_NAME);
    let es_id = extraction_status_id(source_schema, source_key, EXTRACTOR_NAME);

    let plan = plan_face_extraction(
        source_schema,
        source_key,
        faces,
        &mention_id,
        &es_id,
        now_iso8601,
    );

    let write_outcome = write_records(node.clone(), &plan.records).await?;

    // Cross-photo similarity edges. This is the piece the design
    // doc (fingerprints.md §Resolution) describes as "for each
    // face-kind Fi: HNSW query against existing face Fingerprints"
    // and that the original `extractors/face.rs` module explicitly
    // deferred. See docs/findings/fingerprints_phase1_ui_walkthrough.md
    // §Gap 2 for the walkthrough finding that shipped without it.
    //
    // Best-effort: a similarity-edge failure logs loudly but does
    // NOT unwind the successful fingerprint/mention writes. The
    // fingerprint graph stays correct at the record level; the
    // persona view that depends on these edges degrades gracefully.
    let similarity_edges =
        emit_similarity_edges_for_faces(node, faces, &mention_id, now_iso8601).await;
    let similarity_records_written = match similarity_edges {
        Ok(n) => n,
        Err(e) => {
            log::warn!(
                "fingerprints.ingest: similarity-edge emission failed for {}:{} — \
                 Fingerprints/Mentions wrote successfully but similarity graph is stale. Error: {}",
                source_schema,
                source_key,
                e
            );
            0
        }
    };

    Ok(IngestionOutcome {
        records_written: write_outcome.total() + similarity_records_written,
        face_count: plan.face_count,
        ran_empty: plan.ran_empty,
    })
}

/// For every face in `faces`, find nearest neighbors in the face
/// ANN cache, emit Edge + EdgeByFingerprint junction records for
/// any pair above `MIN_SIMILARITY_EDGE`, then add the new face to
/// the cache so subsequent calls see it.
///
/// This runs **after** `write_records(plan.records)` succeeds so
/// the source Fingerprint records referenced by the new Edges
/// exist in the store. The resolver's dangling-edge diagnostic
/// already handles the race where an edge's endpoint disappears;
/// we'd rather leak a harmless edge than miss one.
///
/// The function returns the number of records written, or an
/// error on first write failure. Cache-insert and embedding-
/// normalization failures log and are skipped — they are
/// recoverable on the next face.
async fn emit_similarity_edges_for_faces(
    node: Arc<FoldNode>,
    faces: &[DetectedFace],
    mention_id: &str,
    now_iso8601: &str,
) -> FoldDbResult<usize> {
    if faces.is_empty() {
        return Ok(0);
    }

    ensure_cache_ready(&node).await?;
    let ann = cache();

    let mut new_records: Vec<PlannedRecord> = Vec::new();

    for face in faces {
        let fp_id = fingerprint_id_for_face_embedding(&face.embedding);

        // Normalize once; we reuse the result for both the nearest
        // query and the subsequent cache insert.
        let embedding = match FaceEmbedding::new(face.embedding.clone()) {
            Ok(e) => e,
            Err(e) => {
                log::warn!(
                    "fingerprints.ingest: skipping similarity-edge emission for fingerprint {} — malformed embedding: {}",
                    fp_id,
                    e
                );
                continue;
            }
        };

        // Query BEFORE insert so we don't find ourselves. Hits are
        // sorted descending by similarity; we walk through and emit
        // edges to each neighbor above the floor.
        let hits = ann.nearest(&embedding, NEIGHBOR_QUERY_K);

        for hit in hits {
            if hit.fingerprint_id == fp_id {
                // Defensive: nearest() may return self for
                // already-indexed ids if an ingestion retried.
                continue;
            }
            if hit.similarity < MIN_SIMILARITY_EDGE {
                continue;
            }
            let kind = if hit.similarity >= STRONG_MATCH_CUTOFF {
                edge_kind::STRONG_MATCH
            } else {
                edge_kind::MEDIUM_MATCH
            };
            let eg_id = edge_id(&fp_id, &hit.fingerprint_id, kind);

            new_records.push(PlannedRecord::hash(
                EDGE,
                eg_id.clone(),
                similarity_edge_fields(
                    &eg_id,
                    &fp_id,
                    &hit.fingerprint_id,
                    kind,
                    hit.similarity,
                    mention_id,
                    now_iso8601,
                ),
            ));
            new_records.push(PlannedRecord::hash_range(
                EDGE_BY_FINGERPRINT,
                fp_id.clone(),
                eg_id.clone(),
                edge_by_fingerprint_fields(&fp_id, &eg_id),
            ));
            new_records.push(PlannedRecord::hash_range(
                EDGE_BY_FINGERPRINT,
                hit.fingerprint_id.clone(),
                eg_id.clone(),
                edge_by_fingerprint_fields(&hit.fingerprint_id, &eg_id),
            ));
        }

        // Add AFTER querying so sibling faces in the same photo
        // also see this one on their own query pass.
        ann.add(fp_id, embedding);
    }

    if new_records.is_empty() {
        return Ok(0);
    }

    let outcome = write_records(node, &new_records).await?;
    Ok(outcome.total())
}

fn similarity_edge_fields(
    eg_id: &str,
    a: &str,
    b: &str,
    kind: &'static str,
    weight: f32,
    mention_id: &str,
    now: &str,
) -> HashMap<String, Value> {
    let mut m = HashMap::new();
    m.insert("id".to_string(), json!(eg_id));
    m.insert("a".to_string(), json!(a));
    m.insert("b".to_string(), json!(b));
    m.insert("kind".to_string(), json!(kind));
    m.insert("weight".to_string(), json!(weight));
    m.insert("evidence_mention_ids".to_string(), json!(vec![mention_id]));
    m.insert("created_at".to_string(), json!(now));
    m
}

fn edge_by_fingerprint_fields(fp_id: &str, eg_id: &str) -> HashMap<String, Value> {
    let mut m = HashMap::new();
    m.insert("fingerprint_id".to_string(), json!(fp_id));
    m.insert("edge_id".to_string(), json!(eg_id));
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mention_id_is_deterministic() {
        let a = deterministic_mention_id("Photos", "IMG_1234", "face_detect");
        let b = deterministic_mention_id("Photos", "IMG_1234", "face_detect");
        assert_eq!(a, b);
        assert!(a.starts_with("mn_"));
        assert_eq!(a.len(), 3 + 32);
    }

    #[test]
    fn mention_id_differs_across_photos() {
        let a = deterministic_mention_id("Photos", "IMG_1234", "face_detect");
        let b = deterministic_mention_id("Photos", "IMG_5678", "face_detect");
        assert_ne!(a, b);
    }

    #[test]
    fn mention_id_differs_across_schemas() {
        let a = deterministic_mention_id("Photos", "X", "face_detect");
        let b = deterministic_mention_id("ScreenCaps", "X", "face_detect");
        assert_ne!(a, b);
    }

    #[test]
    fn mention_id_differs_across_extractors() {
        let a = deterministic_mention_id("Photos", "X", "face_detect");
        let b = deterministic_mention_id("Photos", "X", "ner");
        assert_ne!(a, b);
    }

    #[test]
    fn extraction_status_id_is_human_readable() {
        let id = extraction_status_id("Photos", "IMG_1234", "face_detect");
        assert_eq!(id, "es_Photos:IMG_1234:face_detect");
    }

    #[test]
    fn similarity_edge_fields_roundtrips_core_fields() {
        let f = similarity_edge_fields(
            "eg_test",
            "fp_a",
            "fp_b",
            edge_kind::STRONG_MATCH,
            0.97,
            "mn_xyz",
            "2026-04-15T00:00:00Z",
        );
        assert_eq!(f.get("id").unwrap(), &json!("eg_test"));
        assert_eq!(f.get("a").unwrap(), &json!("fp_a"));
        assert_eq!(f.get("b").unwrap(), &json!("fp_b"));
        assert_eq!(f.get("kind").unwrap(), &json!("StrongMatch"));
        assert!((f.get("weight").unwrap().as_f64().unwrap() - 0.97_f64).abs() < 1e-5);
        assert_eq!(
            f.get("evidence_mention_ids").unwrap(),
            &json!(vec!["mn_xyz"])
        );
    }

    #[test]
    fn edge_by_fingerprint_fields_has_two_keys() {
        let f = edge_by_fingerprint_fields("fp_a", "eg_a");
        assert_eq!(f.get("fingerprint_id").unwrap(), &json!("fp_a"));
        assert_eq!(f.get("edge_id").unwrap(), &json!("eg_a"));
        assert_eq!(f.len(), 2);
    }

    // Compile-time sanity for the similarity cutoffs. These are
    // `const` expressions, so a runtime `assert!` would trip
    // clippy::assertions_on_constants. A top-level `const _: () =
    // assert!(...)` shifts the check to compile time and stays
    // silent when green.
    const _: () = assert!(MIN_SIMILARITY_EDGE > 0.0);
    const _: () = assert!(MIN_SIMILARITY_EDGE < 1.0);
    const _: () = assert!(STRONG_MATCH_CUTOFF > MIN_SIMILARITY_EDGE);
    const _: () = assert!(STRONG_MATCH_CUTOFF <= 1.0);
}
