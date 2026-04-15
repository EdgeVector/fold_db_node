//! Best-effort writer for IngestionError records.
//!
//! When an extractor fails on a source record, the design doc's
//! failure posture (fingerprints.md §"Failure posture — non-
//! negotiable") says an `IngestionError` row MUST be written so the
//! UI Failed panel can surface the failure and the user can retry or
//! dismiss it. Until this module existed, nothing in the codebase
//! ever emitted an IngestionError row, so the Failed panel was
//! permanently empty even when extractors were silently dropping
//! records.
//!
//! The write path is intentionally **best-effort**: if the error
//! record itself cannot be written (e.g. canonical_names registry
//! not populated, Sled full, schema missing) we log loudly and
//! return `Ok(())` to the caller. The whole point of this module
//! is to surface failures, not to add a new class of failures on
//! top of the original one — a broken error-writer that panics on
//! the unhappy path would make the overall experience strictly
//! worse than swallowing the error silently.
//!
//! The caller is responsible for:
//! - categorizing the failure (error_class string)
//! - composing the full error context (error_msg string)
//! - supplying source_schema + source_key + extractor
//!
//! This module is NOT responsible for retry logic. Retry count
//! starts at 0 on every new row; the user-visible retry action
//! updates the row in place via the PATCH endpoint in
//! `handlers::fingerprints::ingestion_errors`.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use serde_json::{json, Value};

use crate::fingerprints::canonical_names;
use crate::fingerprints::planned_record::PlannedRecord;
use crate::fingerprints::schemas::INGESTION_ERROR;
use crate::fingerprints::writer::write_records;
use crate::fold_node::FoldNode;
use fold_db::error::FoldDbResult;

/// Arguments for a single IngestionError record.
#[derive(Debug, Clone)]
pub struct IngestionErrorRecord<'a> {
    pub source_schema: &'a str,
    pub source_key: &'a str,
    pub extractor: &'a str,
    pub error_class: &'a str,
    pub error_msg: &'a str,
}

/// Write a single IngestionError row. Best-effort: failures are
/// logged but do not propagate — see module docs for rationale.
///
/// The returned `Ok` carries no data because callers in the happy
/// path don't need the row id. Internal callers that want to link
/// the error to a follow-up record can use
/// [`build_ingestion_error_record`] directly and write it through
/// the normal `write_records` path.
pub async fn write_ingestion_error(node: Arc<FoldNode>, row: IngestionErrorRecord<'_>) {
    let planned = build_ingestion_error_record(&row);
    match write_records(node, &[planned]).await {
        Ok(outcome) => {
            log::info!(
                "fingerprints.ingestion_error_writer: recorded failure \
                 (source={}:{}, extractor={}, class={}, written={})",
                row.source_schema,
                row.source_key,
                row.extractor,
                row.error_class,
                outcome.total()
            );
        }
        Err(e) => {
            // Loud but non-fatal — we cannot make the situation
            // worse than it already is by propagating.
            log::error!(
                "fingerprints.ingestion_error_writer: FAILED TO RECORD FAILURE \
                 (source={}:{}, extractor={}, original_error={}): {}",
                row.source_schema,
                row.source_key,
                row.extractor,
                row.error_msg,
                e
            );
        }
    }
}

/// Construct a [`PlannedRecord`] for the IngestionError schema.
/// Broken out so unit tests can exercise the field shape without a
/// live node.
pub fn build_ingestion_error_record(row: &IngestionErrorRecord<'_>) -> PlannedRecord {
    let id = deterministic_error_id(row);
    let now = Utc::now().to_rfc3339();
    let mut fields: HashMap<String, Value> = HashMap::new();
    fields.insert("id".to_string(), json!(id));
    fields.insert("source_schema".to_string(), json!(row.source_schema));
    fields.insert("source_key".to_string(), json!(row.source_key));
    fields.insert("extractor".to_string(), json!(row.extractor));
    fields.insert("error_class".to_string(), json!(row.error_class));
    fields.insert("error_msg".to_string(), json!(row.error_msg));
    fields.insert("retry_count".to_string(), json!(0));
    fields.insert("resolved".to_string(), json!(false));
    fields.insert("created_at".to_string(), json!(now));
    fields.insert("last_retry_at".to_string(), Value::Null);

    PlannedRecord::hash(INGESTION_ERROR, id, fields)
}

/// Deterministic id so re-running an extractor against the same
/// source + extractor overwrites the previous error row in place
/// instead of accumulating a history. Users of the Failed panel
/// want "this record failed" to show exactly once per failure mode,
/// not once per retry attempt.
fn deterministic_error_id(row: &IngestionErrorRecord<'_>) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"IngestionError");
    h.update(row.source_schema.as_bytes());
    h.update(b":");
    h.update(row.source_key.as_bytes());
    h.update(b":");
    h.update(row.extractor.as_bytes());
    format!("ie_{:x}", h.finalize())
        .chars()
        .take(35) // ie_ + 32 hex chars
        .collect()
}

/// Sanity-check that the canonical_names registry has the
/// IngestionError schema before we try to write. Returns an error
/// that is intended to be swallowed by the caller — it only exists
/// so internal call sites can decide whether to bother issuing a
/// `write_ingestion_error` at all when the registry hasn't been
/// populated yet (e.g. during early startup).
pub fn canonical_names_ready_for_ingestion_error() -> FoldDbResult<()> {
    canonical_names::lookup(INGESTION_ERROR)
        .map(|_| ())
        .map_err(|e| {
            fold_db::error::FoldDbError::Config(format!(
                "ingestion_error_writer: canonical_names registry missing '{}' — \
             register_phase_1_schemas() must run first. Error: {}",
                INGESTION_ERROR, e
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row<'a>() -> IngestionErrorRecord<'a> {
        IngestionErrorRecord {
            source_schema: "Photos",
            source_key: "IMG_1234",
            extractor: "face_detect",
            error_class: "FaceDetectorTimeout",
            error_msg: "timed out after 30s",
        }
    }

    #[test]
    fn deterministic_error_id_is_stable_across_calls() {
        let a = deterministic_error_id(&row());
        let b = deterministic_error_id(&row());
        assert_eq!(a, b);
        assert!(a.starts_with("ie_"));
        assert_eq!(a.len(), 35);
    }

    #[test]
    fn deterministic_error_id_changes_with_source_key() {
        let mut row_a = row();
        row_a.source_key = "IMG_A";
        let mut row_b = row();
        row_b.source_key = "IMG_B";
        assert_ne!(
            deterministic_error_id(&row_a),
            deterministic_error_id(&row_b)
        );
    }

    #[test]
    fn deterministic_error_id_changes_with_extractor() {
        let mut row_a = row();
        row_a.extractor = "face_detect";
        let mut row_b = row();
        row_b.extractor = "ner_llm";
        assert_ne!(
            deterministic_error_id(&row_a),
            deterministic_error_id(&row_b)
        );
    }

    #[test]
    fn build_ingestion_error_record_populates_every_required_field() {
        let rec = build_ingestion_error_record(&row());
        assert_eq!(rec.descriptive_schema, INGESTION_ERROR);
        assert!(rec.hash_key.starts_with("ie_"));
        assert_eq!(rec.fields.get("source_schema").unwrap(), &json!("Photos"));
        assert_eq!(rec.fields.get("source_key").unwrap(), &json!("IMG_1234"));
        assert_eq!(rec.fields.get("extractor").unwrap(), &json!("face_detect"));
        assert_eq!(
            rec.fields.get("error_class").unwrap(),
            &json!("FaceDetectorTimeout")
        );
        assert_eq!(rec.fields.get("retry_count").unwrap(), &json!(0));
        assert_eq!(rec.fields.get("resolved").unwrap(), &json!(false));
        assert!(rec.fields.get("last_retry_at").unwrap().is_null());
    }

    #[test]
    fn build_ingestion_error_record_includes_full_error_message() {
        let multiline = IngestionErrorRecord {
            source_schema: "Photos",
            source_key: "IMG_1",
            extractor: "face_detect",
            error_class: "DecodeError",
            error_msg: "line 1\nline 2\n  nested",
        };
        let rec = build_ingestion_error_record(&multiline);
        assert_eq!(
            rec.fields.get("error_msg").unwrap(),
            &json!("line 1\nline 2\n  nested")
        );
    }
}
