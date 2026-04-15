//! Text-record → fingerprint ingestion helper.
//!
//! Ties together [`crate::fingerprints::extractors::text::plan_text_extraction`]
//! and [`crate::fingerprints::writer::write_records`] so callers can
//! turn a single text record's body into persisted Fingerprint +
//! Mention + Edge + junction + ExtractionStatus records in one
//! async call.
//!
//! Follows the same architecture as [`crate::fingerprints::ingest_photo`].

use std::sync::Arc;

use crate::fingerprints::extractors::text::plan_text_extraction;
use crate::fingerprints::ingest_photo::{deterministic_mention_id, extraction_status_id};
use crate::fingerprints::writer::write_records;
use crate::fold_node::FoldNode;
use fold_db::error::FoldDbResult;

/// Extractor name — must match `text::EXTRACTOR_NAME`.
const EXTRACTOR_NAME: &str = "text_regex";

/// Summary of a single-record text ingestion pass.
#[derive(Debug, Clone, Default)]
pub struct TextIngestionOutcome {
    pub records_written: usize,
    pub signal_count: usize,
    pub ran_empty: bool,
}

/// Plan and persist Phase 2 fingerprint records for the text body
/// of a single source record (note, message, etc.).
pub async fn ingest_text_signals(
    node: Arc<FoldNode>,
    source_schema: &str,
    source_key: &str,
    text: &str,
    now_iso8601: &str,
) -> FoldDbResult<TextIngestionOutcome> {
    let mention_id = deterministic_mention_id(source_schema, source_key, EXTRACTOR_NAME);
    let es_id = extraction_status_id(source_schema, source_key, EXTRACTOR_NAME);

    let plan = plan_text_extraction(
        source_schema,
        source_key,
        text,
        &mention_id,
        &es_id,
        now_iso8601,
    );

    let write_outcome = write_records(node, &plan.records).await?;

    Ok(TextIngestionOutcome {
        records_written: write_outcome.total(),
        signal_count: plan.signal_count,
        ran_empty: plan.ran_empty,
    })
}
