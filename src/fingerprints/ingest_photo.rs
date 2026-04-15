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

use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::fingerprints::extractors::face::{plan_face_extraction, DetectedFace};
use crate::fingerprints::writer::{write_records, WriteOutcome};
use crate::fold_node::FoldNode;
use fold_db::error::FoldDbResult;

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

    let write_outcome = write_records(node, &plan.records).await?;

    Ok(IngestionOutcome {
        records_written: write_outcome.total(),
        face_count: plan.face_count,
        ran_empty: plan.ran_empty,
    })
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
}
