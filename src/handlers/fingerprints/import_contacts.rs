//! Import the trust contact book into the fingerprint graph.
//!
//! Iterates every non-revoked contact and creates:
//! - A `FullName` fingerprint from `display_name`
//! - A `NodePubKey` fingerprint from `public_key`
//! - An Email or Phone fingerprint from `contact_hint` (if parseable)
//! - CoOccurrence edges between all signals for the same contact
//! - Mention records referencing "Contacts:<public_key>" as the source
//!
//! Idempotent: all fingerprints are content-keyed and mentions use
//! deterministic ids, so re-running the import produces the same
//! records and fold_db's upsert semantics handle dedup.
//!
//! ## Endpoint
//!
//! `POST /api/fingerprints/import-contacts` — no body required.
//! Reads contacts from the on-disk contact book.

use crate::fingerprints::ingest_photo::{deterministic_mention_id, extraction_status_id};
use crate::fingerprints::keys::{edge_id, edge_kind, fingerprint_id_for_string, kind};
use crate::fingerprints::planned_record::PlannedRecord;
use crate::fingerprints::schemas::{
    EDGE, EDGE_BY_FINGERPRINT, EXTRACTION_STATUS, FINGERPRINT, MENTION, MENTION_BY_FINGERPRINT,
    MENTION_BY_SOURCE,
};
use crate::fingerprints::writer::write_records;
use crate::fold_node::FoldNode;
use crate::handlers::response::{ApiResponse, HandlerError, HandlerResult};
use crate::trust::contact_book::ContactBook;
use chrono::Utc;
use regex::Regex;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

const SOURCE_SCHEMA: &str = "Contacts";
const EXTRACTOR_NAME: &str = "contact_import";

#[derive(Debug, Clone, Serialize)]
pub struct ImportContactsResponse {
    pub contacts_processed: usize,
    pub fingerprints_created: usize,
    pub edges_created: usize,
    pub total_records_written: usize,
}

pub async fn import_contacts(node: Arc<FoldNode>) -> HandlerResult<ImportContactsResponse> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("FoldDB not available: {}", e)))?;
    let book = ContactBook::load(&db)
        .await
        .map_err(|e| HandlerError::Internal(format!("failed to load contact book: {}", e)))?;

    let contacts: Vec<_> = book.active_contacts().into_iter().cloned().collect();
    let now = Utc::now().to_rfc3339();

    let mut all_records: Vec<PlannedRecord> = Vec::new();
    let mut fp_count = 0usize;
    let mut edge_count = 0usize;

    for contact in &contacts {
        let source_key = &contact.public_key;
        let mention_id = deterministic_mention_id(SOURCE_SCHEMA, source_key, EXTRACTOR_NAME);
        let es_id = extraction_status_id(SOURCE_SCHEMA, source_key, EXTRACTOR_NAME);

        // Collect fingerprints for this contact.
        let mut fp_ids: Vec<String> = Vec::new();

        // FullName from display_name.
        if !contact.display_name.trim().is_empty() {
            let fp_id = fingerprint_id_for_string(kind::FULL_NAME, &contact.display_name);
            all_records.push(PlannedRecord::hash(
                FINGERPRINT,
                fp_id.clone(),
                fp_fields(&fp_id, kind::FULL_NAME, &contact.display_name, &now),
            ));
            fp_ids.push(fp_id);
            fp_count += 1;
        }

        // NodePubKey from public_key.
        {
            let fp_id = fingerprint_id_for_string(kind::NODE_PUB_KEY, &contact.public_key);
            all_records.push(PlannedRecord::hash(
                FINGERPRINT,
                fp_id.clone(),
                fp_fields(&fp_id, kind::NODE_PUB_KEY, &contact.public_key, &now),
            ));
            fp_ids.push(fp_id);
            fp_count += 1;
        }

        // Try to extract email/phone from contact_hint.
        if let Some(ref hint) = contact.contact_hint {
            let trimmed = hint.trim();
            if !trimmed.is_empty() {
                let (hint_kind, hint_value) = classify_hint(trimmed);
                let fp_id = fingerprint_id_for_string(hint_kind, &hint_value);
                if !fp_ids.contains(&fp_id) {
                    all_records.push(PlannedRecord::hash(
                        FINGERPRINT,
                        fp_id.clone(),
                        fp_fields(&fp_id, hint_kind, &hint_value, &now),
                    ));
                    fp_ids.push(fp_id);
                    fp_count += 1;
                }
            }
        }

        // ExtractionStatus.
        all_records.push(PlannedRecord::hash(
            EXTRACTION_STATUS,
            es_id.clone(),
            es_fields(&es_id, SOURCE_SCHEMA, source_key, fp_ids.len(), &now),
        ));

        // Mention.
        all_records.push(PlannedRecord::hash(
            MENTION,
            mention_id.clone(),
            mn_fields(&mention_id, SOURCE_SCHEMA, source_key, &fp_ids, &now),
        ));

        // MentionBySource junction.
        let src_composite = format!("{}:{}", SOURCE_SCHEMA, source_key);
        all_records.push(PlannedRecord::hash_range(
            MENTION_BY_SOURCE,
            src_composite.clone(),
            mention_id.clone(),
            junction_fields(
                "source_composite",
                &src_composite,
                "mention_id",
                &mention_id,
            ),
        ));

        // MentionByFingerprint junctions.
        for fp_id in &fp_ids {
            all_records.push(PlannedRecord::hash_range(
                MENTION_BY_FINGERPRINT,
                fp_id.clone(),
                mention_id.clone(),
                junction_fields("fingerprint_id", fp_id, "mention_id", &mention_id),
            ));
        }

        // CoOccurrence edges between all pairs.
        for i in 0..fp_ids.len() {
            for j in (i + 1)..fp_ids.len() {
                let a = &fp_ids[i];
                let b = &fp_ids[j];
                let eg_id = edge_id(a, b, edge_kind::CO_OCCURRENCE);
                all_records.push(PlannedRecord::hash(
                    EDGE,
                    eg_id.clone(),
                    eg_fields(&eg_id, a, b, 0.5, &mention_id, &now),
                ));
                all_records.push(PlannedRecord::hash_range(
                    EDGE_BY_FINGERPRINT,
                    a.clone(),
                    eg_id.clone(),
                    junction_fields("fingerprint_id", a, "edge_id", &eg_id),
                ));
                all_records.push(PlannedRecord::hash_range(
                    EDGE_BY_FINGERPRINT,
                    b.clone(),
                    eg_id.clone(),
                    junction_fields("fingerprint_id", b, "edge_id", &eg_id),
                ));
                edge_count += 1;
            }
        }
    }

    if all_records.is_empty() {
        return Ok(ApiResponse::success(ImportContactsResponse {
            contacts_processed: 0,
            fingerprints_created: 0,
            edges_created: 0,
            total_records_written: 0,
        }));
    }

    let outcome = write_records(node.clone(), &all_records)
        .await
        .map_err(|e| HandlerError::Internal(format!("contact import write failed: {}", e)))?;

    tracing::info!(
        "fingerprints.import_contacts: {} contacts → {} fingerprints, {} edges, {} records written",
        contacts.len(),
        fp_count,
        edge_count,
        outcome.total()
    );

    // Fire-and-forget post-ingest sweep — matches the text and photo
    // ingest handlers so personas auto-form once enough fingerprints
    // accumulate. Internally debounced.
    if outcome.total() > 0 {
        crate::fingerprints::auto_propose::maybe_spawn_persona_sweep(node);
    }

    Ok(ApiResponse::success(ImportContactsResponse {
        contacts_processed: contacts.len(),
        fingerprints_created: fp_count,
        edges_created: edge_count,
        total_records_written: outcome.total(),
    }))
}

// ── Field helpers ────────────────────────────────────────────────

fn fp_fields(id: &str, fp_kind: &str, value: &str, now: &str) -> HashMap<String, Value> {
    let mut m = HashMap::new();
    m.insert("id".to_string(), json!(id));
    m.insert("kind".to_string(), json!(fp_kind));
    m.insert("value".to_string(), json!(value));
    m.insert("first_seen".to_string(), json!(now));
    m.insert("last_seen".to_string(), json!(now));
    m
}

fn mn_fields(
    id: &str,
    schema: &str,
    key: &str,
    fp_ids: &[String],
    now: &str,
) -> HashMap<String, Value> {
    let mut m = HashMap::new();
    m.insert("id".to_string(), json!(id));
    m.insert("source_schema".to_string(), json!(schema));
    m.insert("source_key".to_string(), json!(key));
    m.insert("source_field".to_string(), json!("contact"));
    m.insert("fingerprint_ids".to_string(), json!(fp_ids));
    m.insert("extractor".to_string(), json!(EXTRACTOR_NAME));
    m.insert("confidence".to_string(), json!(1.0_f32));
    m.insert("created_at".to_string(), json!(now));
    m
}

fn es_fields(id: &str, schema: &str, key: &str, count: usize, now: &str) -> HashMap<String, Value> {
    let mut m = HashMap::new();
    m.insert("id".to_string(), json!(id));
    m.insert("source_schema".to_string(), json!(schema));
    m.insert("source_key".to_string(), json!(key));
    m.insert("extractor".to_string(), json!(EXTRACTOR_NAME));
    m.insert(
        "status".to_string(),
        json!(if count > 0 {
            "RanWithResults"
        } else {
            "RanEmpty"
        }),
    );
    m.insert("fingerprint_count".to_string(), json!(count));
    m.insert("ran_at".to_string(), json!(now));
    m.insert("model_version".to_string(), Value::Null);
    m
}

fn eg_fields(
    id: &str,
    a: &str,
    b: &str,
    weight: f32,
    mention_id: &str,
    now: &str,
) -> HashMap<String, Value> {
    let mut m = HashMap::new();
    m.insert("id".to_string(), json!(id));
    m.insert("a".to_string(), json!(a));
    m.insert("b".to_string(), json!(b));
    m.insert("kind".to_string(), json!(edge_kind::CO_OCCURRENCE));
    m.insert("weight".to_string(), json!(weight));
    m.insert("evidence_mention_ids".to_string(), json!(vec![mention_id]));
    m.insert("created_at".to_string(), json!(now));
    m
}

/// Classify a contact_hint as email, phone, or handle.
fn classify_hint(hint: &str) -> (&'static str, String) {
    let email_re =
        Regex::new(r"^[a-zA-Z0-9._%+\-]+@[a-zA-Z0-9.\-]+\.[a-zA-Z]{2,}$").expect("valid regex");
    if email_re.is_match(hint) {
        return (kind::EMAIL, hint.to_lowercase());
    }
    // Looks like a phone number if it's mostly digits.
    let digit_count = hint.chars().filter(|c| c.is_ascii_digit()).count();
    if digit_count >= 7 {
        let canonical: String = hint
            .chars()
            .filter(|c| c.is_ascii_digit() || *c == '+')
            .collect();
        return (kind::PHONE, canonical);
    }
    // Fall back to handle (social media username, etc.)
    (kind::HANDLE, hint.to_lowercase())
}

fn junction_fields(
    hash_name: &str,
    hash_val: &str,
    range_name: &str,
    range_val: &str,
) -> HashMap<String, Value> {
    let mut m = HashMap::new();
    m.insert(hash_name.to_string(), json!(hash_val));
    m.insert(range_name.to_string(), json!(range_val));
    m
}
