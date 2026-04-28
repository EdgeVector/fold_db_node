//! Batch-ingest text-based signals (emails, phones) from records
//! whose body field contains natural text (Notes, Messages, etc.).
//!
//! ## Endpoint this backs
//!
//! `POST /api/fingerprints/ingest-text-signals`
//!
//! Accepts a batch of `{ source_schema, records: [{ source_key, text }] }`
//! and runs the text regex extractor over each record's text body.
//! Follows the same partial-success pattern as the face ingest
//! handler — a per-record error does not abort the batch.

use crate::fingerprints::ingest_text::{ingest_text_signals, TextIngestionOutcome};
use crate::fold_node::FoldNode;
use crate::handlers::response::{require_non_empty, ApiResponse, HandlerResult};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::Instrument;

// ── Request / response types ─────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct TextRecordDto {
    pub source_key: String,
    pub text: String,
    /// Caller signals that this record is expected to contain
    /// identity content (e.g. an email record with a structured
    /// sender address, not a freeform note that says "coffee
    /// good"). When `true` and the extractor ran empty with no
    /// IngestionError written, a meta-level `ZeroExtractorYield`
    /// IngestionError is emitted so the silent-gap case is
    /// surfaced in the Failed panel. Absent/false means "no claim"
    /// — the zero-yield check is skipped and the record is treated
    /// as legitimately empty. See TODO-6 in the workspace backlog.
    #[serde(default)]
    pub expected_to_yield: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IngestTextSignalsRequest {
    pub source_schema: String,
    pub records: Vec<TextRecordDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TextRecordResult {
    pub source_key: String,
    pub ok: bool,
    pub signal_count: usize,
    pub records_written: usize,
    pub ran_empty: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IngestTextSignalsResponse {
    pub total_records: usize,
    pub successful_records: usize,
    pub total_signals: usize,
    pub total_records_written: usize,
    pub per_record: Vec<TextRecordResult>,
}

// ── Handler ─────────────────────────────────────────────────────

pub async fn ingest_text_signals_batch(
    node: Arc<FoldNode>,
    request: IngestTextSignalsRequest,
) -> HandlerResult<IngestTextSignalsResponse> {
    require_non_empty(
        &request.source_schema,
        "source_schema must be a non-empty string",
    )?;

    let now_iso8601 = Utc::now().to_rfc3339();
    let total_records = request.records.len();

    let mut per_record: Vec<TextRecordResult> = Vec::with_capacity(total_records);
    let mut successful_records = 0usize;
    let mut total_signals = 0usize;
    let mut total_records_written = 0usize;

    tracing::info!(
        "fingerprints.ingest_text: starting batch ingest of {} records under schema '{}'",
        total_records,
        request.source_schema
    );

    for rec in request.records {
        let source_key = rec.source_key.clone();
        let expected_to_yield = rec.expected_to_yield;

        match ingest_text_signals(
            node.clone(),
            &request.source_schema,
            &source_key,
            &rec.text,
            &now_iso8601,
        )
        .await
        {
            Ok(TextIngestionOutcome {
                records_written,
                signal_count,
                ran_empty,
            }) => {
                successful_records += 1;
                total_signals += signal_count;
                total_records_written += records_written;
                // TODO-6: surface the silent-gap case. The writer
                // succeeded and the extractor saw zero signals; if
                // the caller flagged this record as expected to
                // yield, emit the meta-level ZeroExtractorYield row.
                if expected_to_yield && ran_empty {
                    crate::fingerprints::ingestion_error_writer::emit_zero_yield_meta_error(
                        node.clone(),
                        &request.source_schema,
                        &source_key,
                        "text_regex ran with zero fingerprints despite expected_to_yield=true",
                    )
                    .await;
                }
                per_record.push(TextRecordResult {
                    source_key,
                    ok: true,
                    signal_count,
                    records_written,
                    ran_empty,
                    error: None,
                });
            }
            Err(e) => {
                let msg = format!("{}", e);
                tracing::warn!(
                    "fingerprints.ingest_text: record '{}' on schema '{}' failed: {}",
                    source_key,
                    request.source_schema,
                    msg
                );
                per_record.push(TextRecordResult {
                    source_key,
                    ok: false,
                    signal_count: 0,
                    records_written: 0,
                    ran_empty: false,
                    error: Some(msg),
                });
            }
        }
    }

    tracing::info!(
        "fingerprints.ingest_text: batch complete: {}/{} successful, {} signals, {} records written",
        successful_records,
        total_records,
        total_signals,
        total_records_written,
    );

    // Fire-and-forget post-ingest sweep — mirrors the face ingest
    // path. See src/fingerprints/auto_propose.rs for the create logic.
    if total_records_written > 0 {
        let node_bg = node.clone();
        tokio::spawn(
            async move {
                crate::fingerprints::auto_propose::run_sweep_and_create_personas(node_bg).await;
            }
            .instrument(tracing::Span::current()),
        );
    }

    Ok(ApiResponse::success(IngestTextSignalsResponse {
        total_records,
        successful_records,
        total_signals,
        total_records_written,
        per_record,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_deserializes_from_json() {
        let raw = json!({
            "source_schema": "Notes",
            "records": [
                {"source_key": "note_1", "text": "Email: tom@acme.com"},
                {"source_key": "note_2", "text": "No signals here."}
            ]
        });
        let req: IngestTextSignalsRequest = serde_json::from_value(raw).expect("deserialize");
        assert_eq!(req.source_schema, "Notes");
        assert_eq!(req.records.len(), 2);
        assert_eq!(req.records[0].source_key, "note_1");
        // TODO-6 field is optional; both records omit it, so both
        // default to false and the zero-yield check is a no-op.
        assert!(!req.records[0].expected_to_yield);
        assert!(!req.records[1].expected_to_yield);
    }

    #[test]
    fn expected_to_yield_round_trips_true() {
        let raw = json!({
            "source_key": "email_1",
            "text": "no parseable signal",
            "expected_to_yield": true
        });
        let dto: TextRecordDto = serde_json::from_value(raw).expect("deserialize");
        assert!(dto.expected_to_yield);
    }

    #[test]
    fn response_serializes_compactly() {
        let resp = IngestTextSignalsResponse {
            total_records: 2,
            successful_records: 2,
            total_signals: 3,
            total_records_written: 15,
            per_record: vec![
                TextRecordResult {
                    source_key: "note_1".into(),
                    ok: true,
                    signal_count: 2,
                    records_written: 10,
                    ran_empty: false,
                    error: None,
                },
                TextRecordResult {
                    source_key: "note_2".into(),
                    ok: true,
                    signal_count: 1,
                    records_written: 5,
                    ran_empty: false,
                    error: None,
                },
            ],
        };
        let json = serde_json::to_value(resp).unwrap();
        assert_eq!(json["total_signals"], 3);
        assert!(
            json["per_record"][0]["error"].is_null()
                || json["per_record"][0].get("error").is_none()
        );
    }
}
