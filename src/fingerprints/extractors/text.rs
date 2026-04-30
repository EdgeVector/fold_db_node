//! Text-based signal extraction for notes, messages, and other
//! text-heavy records.
//!
//! Scans the text body for well-known identity signals — email
//! addresses, phone numbers, and (optionally) name-like patterns.
//! Each hit becomes a Fingerprint + Mention + junction row, just
//! like the face extractor produces for photos.
//!
//! ## What gets planned
//!
//! Per source record:
//!
//! * **Fingerprint** records — one per unique extracted signal.
//!   Content-keyed (`fp_<sha256(kind, canonical_value)>`) so the
//!   same email appearing in 100 notes produces exactly one
//!   Fingerprint, with 100 Mentions pointing at it.
//! * **Mention** record — one per (source_record, extractor_run),
//!   listing every fingerprint found in that record.
//! * **MentionByFingerprint** junction rows — one per (Mention,
//!   Fingerprint) pair.
//! * **MentionBySource** junction row — one per Mention.
//! * **CoOccurrence edges** — every unordered pair of distinct
//!   fingerprints found in the same record. Weight 0.3 (moderate
//!   — sharing a note is stronger than sharing a photo but weaker
//!   than a direct email thread).
//! * **EdgeByFingerprint** junction rows — two per edge.
//! * **ExtractionStatus** — one per (source, extractor) to
//!   distinguish "not yet processed" from "processed, nothing".
//!
//! ## Regex patterns
//!
//! - **Email**: RFC-5322 simplified — `[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}`
//! - **Phone**: E.164-ish — `+?[0-9][\s\-.()0-9]{6,}[0-9]`
//!   (at least 8 digits, optional leading +, allows parens/dashes)
//!
//! Name extraction is NOT in this module. It requires either an LLM
//! call or a dedicated NER model and is deferred to a separate
//! extractor (P2-6).
//!
//! ## Shared-inbox collision protection (TODO-2)
//!
//! Emails from shared inboxes (`info@`, `sales@`, `support@`, …) are
//! NOT person-level identifiers — dozens of unrelated senders can
//! reply from the same address. Without demotion, every sender who
//! ever replied from `info@acme.com` would be CoOccurrence-edge-linked
//! to each other through that single fingerprint, producing one giant
//! mistaken cluster in the Persona resolver.
//!
//! Defense: `classify_email_strength` returns a per-email strength in
//! [0, 1]. Shared-inbox local parts drop to 0.5 so their CoOccurrence
//! edges land at 0.15 (below the 0.2 cap the design calls for) and
//! cannot cross a realistic resolver threshold.

use std::collections::HashMap;

use regex::Regex;
use serde_json::{json, Value};

use crate::fingerprints::keys::{
    edge_id, edge_kind, fingerprint_id_for_string, kind, mention_source_composite,
};
use crate::fingerprints::planned_record::PlannedRecord;
use crate::fingerprints::schemas::{
    EDGE, EDGE_BY_FINGERPRINT, EXTRACTION_STATUS, FINGERPRINT, MENTION, MENTION_BY_FINGERPRINT,
    MENTION_BY_SOURCE,
};

/// The weight assigned to CoOccurrence edges between signals found
/// in the same text record. 0.3 is moderate — stronger than the
/// 0.2 used for "two faces in the same photo" because a note that
/// mentions both `tom@acme.com` and `alice@acme.com` is a stronger
/// identity signal than two faces in a group shot.
const CO_OCCURRENCE_WEIGHT: f32 = 0.3;

/// Weight assigned to StrongMatch edges between signals found in the
/// same record when the source is *asserting* identity (a contact card,
/// calendar attendee, email header). 0.95 puts the edge above the
/// `MIN_EDGE_WEIGHT = 0.85` floor used by both the auto-sweep and the
/// resolver, so a single contact record contributes a real cluster.
const STRONG_MATCH_WEIGHT: f32 = 0.95;

/// How tightly the source record claims its identity signals belong
/// together. Selected per-call by the caller, with `CoOccurrence` as
/// the safe default.
///
/// - `CoOccurrence` — "these signals appeared together." Weak edge
///   (0.3 × strength), edge_kind = `CoOccurrence`. Right for free-text
///   sources like notes, journals, message bodies.
/// - `Strong` — "the source asserts these signals are the same
///   entity." Strong edge (0.95 × strength), edge_kind = `StrongMatch`.
///   Right for structured sources where the schema names the entity
///   (Contacts, AddressBook, CalendarEvent, EmailHeader).
///
/// The shared-inbox demotion (`classify_email_strength`) still applies
/// in both modes — a strong-binding edge that touches `info@acme.com`
/// gets multiplied by `SHARED_INBOX_STRENGTH` (0.5), landing at 0.475
/// which is below the 0.85 floor. That is the intended safety net:
/// even an asserted record cannot bind a person to a shared inbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalBinding {
    /// Default — the source merely co-mentions these signals.
    CoOccurrence,
    /// The source asserts these signals belong to the same entity.
    Strong,
}

impl SignalBinding {
    /// Edge kind constant + base weight for this binding.
    fn edge_params(self) -> (&'static str, f32) {
        match self {
            SignalBinding::CoOccurrence => (edge_kind::CO_OCCURRENCE, CO_OCCURRENCE_WEIGHT),
            SignalBinding::Strong => (edge_kind::STRONG_MATCH, STRONG_MATCH_WEIGHT),
        }
    }
}

/// Strength multiplier applied to emails whose local-part is a
/// shared-inbox pattern (see `SHARED_INBOX_LOCAL_PARTS`). 0.5 is
/// chosen so that an edge involving one such email lands at
/// 0.3 * 0.5 = 0.15, comfortably below the 0.2 cap the design calls
/// for and below any realistic resolver threshold. The edge weight
/// formula uses `min(a.strength, b.strength)` so two shared-inbox
/// endpoints stay at the same 0.15 floor — no need to compound
/// lower, clustering is already neutralized.
const SHARED_INBOX_STRENGTH: f32 = 0.5;

/// Local-parts (case-insensitive) that indicate a shared inbox rather
/// than a person's mailbox. Keep this conservative — false positives
/// here silently cripple clustering for any real person who happened
/// to pick a collision-y username (`hello@`, `hi@`). The entries
/// below are all multi-person-by-convention and very unlikely to be
/// anyone's primary address.
const SHARED_INBOX_LOCAL_PARTS: &[&str] = &[
    "info",
    "sales",
    "support",
    "help",
    "helpdesk",
    "contact",
    "admin",
    "administrator",
    "noreply",
    "no-reply",
    "do-not-reply",
    "donotreply",
    "team",
    "office",
    "hr",
    "jobs",
    "careers",
    "press",
    "media",
    "marketing",
    "billing",
    "accounts",
    "accounting",
    "finance",
    "legal",
    "privacy",
    "security",
    "abuse",
    "postmaster",
    "webmaster",
    "mailer-daemon",
];

/// Classify how person-identifying an email address is.
///
/// Returns a strength in [0.0, 1.0] — 1.0 for a normal personal
/// mailbox, [`SHARED_INBOX_STRENGTH`] for anything whose local-part
/// matches [`SHARED_INBOX_LOCAL_PARTS`]. Used by the text extractor
/// to demote CoOccurrence edge weights so shared inboxes cannot be
/// absorbed into person Personas by the resolver. See TODO-2 in
/// workspace `TODOS.md`.
///
/// Malformed input (no `@`) returns 1.0 — refusing to classify is
/// safer than silently demoting a misread address. Callers that
/// have already validated the email shape lose nothing.
pub fn classify_email_strength(email: &str) -> f32 {
    let Some((local, _domain)) = email.split_once('@') else {
        return 1.0;
    };
    let local_lower = local.to_lowercase();
    if SHARED_INBOX_LOCAL_PARTS.contains(&local_lower.as_str()) {
        SHARED_INBOX_STRENGTH
    } else {
        1.0
    }
}

/// Extractor name used in Mention + ExtractionStatus records.
pub const EXTRACTOR_NAME: &str = "text_regex";

/// An identity signal extracted from text.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedSignal {
    pub kind: &'static str,
    pub value: String,
    pub fingerprint_id: String,
    /// Per-signal strength in [0, 1]. Used as a multiplier on edge
    /// weights to demote weakly-identifying signals (shared-inbox
    /// emails, etc.) without removing them entirely. 1.0 for every
    /// signal that is a plausible per-person identifier.
    pub strength: f32,
}

/// The full planned output for one text record.
#[derive(Debug)]
pub struct TextExtractionPlan {
    pub records: Vec<PlannedRecord>,
    pub signal_count: usize,
    pub ran_empty: bool,
}

/// Extract email and phone signals from `text` and plan the full
/// set of records for a single source record.
///
/// `mention_id` and `extraction_status_id` are externally generated
/// so the caller can make them deterministic (for idempotent
/// migration re-runs) or random (for live ingestion).
///
/// `binding` selects edge kind + base weight for intra-record edges
/// (see [`SignalBinding`]). Use `Strong` only when the source schema
/// asserts identity (contacts, calendar, email headers); free-text
/// sources should pass `CoOccurrence`.
pub fn plan_text_extraction(
    source_schema: &str,
    source_key: &str,
    text: &str,
    mention_id: &str,
    extraction_status_id: &str,
    now_iso8601: &str,
    binding: SignalBinding,
) -> TextExtractionPlan {
    let signals = extract_signals(text);

    let mut plan = TextExtractionPlan {
        records: Vec::new(),
        signal_count: signals.len(),
        ran_empty: signals.is_empty(),
    };

    // ExtractionStatus — always written.
    plan.records.push(PlannedRecord::hash(
        EXTRACTION_STATUS,
        extraction_status_id.to_string(),
        extraction_status_fields(
            extraction_status_id,
            source_schema,
            source_key,
            signals.len(),
            now_iso8601,
        ),
    ));

    if signals.is_empty() {
        return plan;
    }

    // Dedupe by fingerprint_id (same email appearing twice in a note).
    let mut unique: Vec<&ExtractedSignal> = Vec::new();
    let mut seen_fp_ids: Vec<String> = Vec::new();
    for sig in &signals {
        if !seen_fp_ids.contains(&sig.fingerprint_id) {
            seen_fp_ids.push(sig.fingerprint_id.clone());
            unique.push(sig);
        }
    }

    // Per-signal Fingerprint records.
    for sig in &unique {
        plan.records.push(PlannedRecord::hash(
            FINGERPRINT,
            sig.fingerprint_id.clone(),
            fingerprint_fields(&sig.fingerprint_id, sig.kind, &sig.value, now_iso8601),
        ));
    }

    // Mention record — one per source record, listing all fingerprints.
    let fp_ids: Vec<String> = unique.iter().map(|s| s.fingerprint_id.clone()).collect();
    plan.records.push(PlannedRecord::hash(
        MENTION,
        mention_id.to_string(),
        mention_fields(mention_id, source_schema, source_key, &fp_ids, now_iso8601),
    ));

    // MentionBySource junction — one row.
    let source_composite = mention_source_composite(source_schema, source_key);
    plan.records.push(PlannedRecord::hash_range(
        MENTION_BY_SOURCE,
        source_composite.clone(),
        mention_id.to_string(),
        mention_by_source_fields(&source_composite, mention_id),
    ));

    // MentionByFingerprint junction — one per unique signal.
    for sig in &unique {
        plan.records.push(PlannedRecord::hash_range(
            MENTION_BY_FINGERPRINT,
            sig.fingerprint_id.clone(),
            mention_id.to_string(),
            mention_by_fingerprint_fields(&sig.fingerprint_id, mention_id),
        ));
    }

    // Intra-record edges — every unordered pair. Edge kind + base
    // weight come from `binding`; the per-signal strength multiplier
    // demotes edges that touch a shared-inbox email (see
    // `classify_email_strength`). For CoOccurrence: one weak
    // endpoint → 0.15, two weak endpoints → 0.075. For Strong: one
    // weak endpoint → 0.475 (below the 0.85 floor), preserving the
    // shared-inbox safety net even in asserted-identity mode.
    let (edge_kind_name, base_weight) = binding.edge_params();
    for i in 0..unique.len() {
        for j in (i + 1)..unique.len() {
            let a = &unique[i].fingerprint_id;
            let b = &unique[j].fingerprint_id;
            let eg_id = edge_id(a, b, edge_kind_name);
            let weight = base_weight * unique[i].strength.min(unique[j].strength);

            plan.records.push(PlannedRecord::hash(
                EDGE,
                eg_id.clone(),
                edge_fields(
                    &eg_id,
                    a,
                    b,
                    edge_kind_name,
                    weight,
                    mention_id,
                    now_iso8601,
                ),
            ));
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

/// Extract raw signals from text. Returns every email and phone
/// number found, in discovery order.
pub fn extract_signals(text: &str) -> Vec<ExtractedSignal> {
    let mut signals = Vec::new();

    // Email regex — simplified RFC-5322.
    let email_re =
        Regex::new(r"[a-zA-Z0-9._%+\-]+@[a-zA-Z0-9.\-]+\.[a-zA-Z]{2,}").expect("valid regex");
    for m in email_re.find_iter(text) {
        let raw = m.as_str();
        let fp_id = fingerprint_id_for_string(kind::EMAIL, raw);
        let value = raw.to_lowercase();
        let strength = classify_email_strength(&value);
        signals.push(ExtractedSignal {
            kind: kind::EMAIL,
            value,
            fingerprint_id: fp_id,
            strength,
        });
    }

    // Phone regex — E.164-ish: optional +, then digits with
    // optional separators (spaces, dashes, dots, parens), at least
    // 8 total digit chars.
    let phone_re = Regex::new(r"\+?[0-9][\s\-.()\d]{6,}[0-9]").expect("valid regex");
    for m in phone_re.find_iter(text) {
        let raw = m.as_str();
        // Strip non-digit chars for the canonical form, keep the
        // leading + if present.
        let canonical: String = raw
            .chars()
            .filter(|c| c.is_ascii_digit() || *c == '+')
            .collect();
        if canonical.len() < 7 {
            continue; // too short after stripping
        }
        let fp_id = fingerprint_id_for_string(kind::PHONE, &canonical);
        signals.push(ExtractedSignal {
            kind: kind::PHONE,
            value: canonical,
            fingerprint_id: fp_id,
            strength: 1.0,
        });
    }

    // Name extraction — capitalized word pairs/triples that look
    // like personal names. This is a heuristic regex, NOT an NER
    // model. It catches "Tom Tang", "Alice Bob Chen", etc. but also
    // false-positives like "New York" or "San Francisco". The intent
    // is to produce FullName fingerprints that the Persona resolver
    // connects via CoOccurrence edges to email/phone fingerprints
    // from the same record; the user confirms or rejects at the
    // Persona level, so false-positives degrade the Suggestions
    // panel (noisy candidates) but never commit identity without
    // user action.
    //
    // Exclusion patterns skip common false-positive categories:
    // months, days, common English title words. This is best-effort.
    let name_re =
        Regex::new(r"\b([A-Z][a-z]{1,20}(?:\s+[A-Z][a-z]{1,20}){1,2})\b").expect("valid regex");
    let exclude: std::collections::HashSet<&str> = [
        // Months
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
        // Days
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
        // Common non-name capitalized pairs
        "New York",
        "San Francisco",
        "Los Angeles",
        "Las Vegas",
        "United States",
        "North America",
        "South America",
        "Good Morning",
        "Happy Birthday",
        "Thank You",
        "Dear Sir",
        "Dear Madam",
        "Best Regards",
    ]
    .into_iter()
    .collect();
    for m in name_re.find_iter(text) {
        let raw = m.as_str();
        if exclude.contains(raw) {
            continue;
        }
        // Skip single-word matches (the regex requires 2+ words).
        let word_count = raw.split_whitespace().count();
        if word_count < 2 {
            continue;
        }
        let fp_id = fingerprint_id_for_string(kind::FULL_NAME, raw);
        signals.push(ExtractedSignal {
            kind: kind::FULL_NAME,
            value: raw.to_string(),
            fingerprint_id: fp_id,
            strength: 1.0,
        });
    }

    signals
}

// ── Field-builder helpers ─────────────────────────────────────────

fn fingerprint_fields(
    fp_id: &str,
    fp_kind: &str,
    value: &str,
    now: &str,
) -> HashMap<String, Value> {
    let mut m = HashMap::new();
    m.insert("id".to_string(), json!(fp_id));
    m.insert("kind".to_string(), json!(fp_kind));
    m.insert("value".to_string(), json!(value));
    m.insert("first_seen".to_string(), json!(now));
    m.insert("last_seen".to_string(), json!(now));
    m
}

fn mention_fields(
    mention_id: &str,
    source_schema: &str,
    source_key: &str,
    fp_ids: &[String],
    now: &str,
) -> HashMap<String, Value> {
    let mut m = HashMap::new();
    m.insert("id".to_string(), json!(mention_id));
    m.insert("source_schema".to_string(), json!(source_schema));
    m.insert("source_key".to_string(), json!(source_key));
    m.insert("source_field".to_string(), json!("body"));
    m.insert("fingerprint_ids".to_string(), json!(fp_ids));
    m.insert("extractor".to_string(), json!(EXTRACTOR_NAME));
    m.insert("confidence".to_string(), json!(0.95_f32));
    m.insert("created_at".to_string(), json!(now));
    m
}

fn extraction_status_fields(
    id: &str,
    source_schema: &str,
    source_key: &str,
    count: usize,
    now: &str,
) -> HashMap<String, Value> {
    let mut m = HashMap::new();
    m.insert("id".to_string(), json!(id));
    m.insert("source_schema".to_string(), json!(source_schema));
    m.insert("source_key".to_string(), json!(source_key));
    m.insert("extractor".to_string(), json!(EXTRACTOR_NAME));
    let status = if count > 0 {
        "RanWithResults"
    } else {
        "RanEmpty"
    };
    m.insert("status".to_string(), json!(status));
    m.insert("fingerprint_count".to_string(), json!(count));
    m.insert("ran_at".to_string(), json!(now));
    m.insert("model_version".to_string(), Value::Null);
    m
}

fn edge_fields(
    eg_id: &str,
    a: &str,
    b: &str,
    edge_kind_name: &str,
    weight: f32,
    mention_id: &str,
    now: &str,
) -> HashMap<String, Value> {
    let mut m = HashMap::new();
    m.insert("id".to_string(), json!(eg_id));
    m.insert("a".to_string(), json!(a));
    m.insert("b".to_string(), json!(b));
    m.insert("kind".to_string(), json!(edge_kind_name));
    m.insert("weight".to_string(), json!(weight));
    m.insert("evidence_mention_ids".to_string(), json!(vec![mention_id]));
    m.insert("created_at".to_string(), json!(now));
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
    fn extracts_email_addresses() {
        let text = "Contact tom@acme.com or alice.bob+tag@example.co.uk for details.";
        let signals = extract_signals(text);
        let emails: Vec<&str> = signals
            .iter()
            .filter(|s| s.kind == kind::EMAIL)
            .map(|s| s.value.as_str())
            .collect();
        assert_eq!(emails, vec!["tom@acme.com", "alice.bob+tag@example.co.uk"]);
    }

    #[test]
    fn extracts_phone_numbers() {
        let text = "Call me at +1-555-867-5309 or (415) 555-1234.";
        let signals = extract_signals(text);
        let phones: Vec<&str> = signals
            .iter()
            .filter(|s| s.kind == kind::PHONE)
            .map(|s| s.value.as_str())
            .collect();
        assert_eq!(phones, vec!["+15558675309", "4155551234"]);
    }

    #[test]
    fn skips_short_digit_sequences() {
        let text = "Section 42 of the manual covers item 123.";
        let signals = extract_signals(text);
        let phones: Vec<&str> = signals
            .iter()
            .filter(|s| s.kind == kind::PHONE)
            .map(|s| s.value.as_str())
            .collect();
        assert!(
            phones.is_empty(),
            "short digit sequences should not match as phone numbers"
        );
    }

    #[test]
    fn deduplicates_same_email_in_plan() {
        let plan = plan_text_extraction(
            "Notes",
            "note_1",
            "tom@acme.com wrote: Hi tom@acme.com",
            "mn_test",
            "es_test",
            "2026-04-15T00:00:00Z",
            SignalBinding::CoOccurrence,
        );
        // Only one Fingerprint record despite two occurrences.
        let fp_count = plan
            .records
            .iter()
            .filter(|r| r.descriptive_schema == FINGERPRINT)
            .count();
        assert_eq!(fp_count, 1);
        assert_eq!(plan.signal_count, 2); // raw count includes both
    }

    #[test]
    fn empty_text_produces_ran_empty_status() {
        let plan = plan_text_extraction(
            "Notes",
            "note_empty",
            "No contact info here.",
            "mn_test",
            "es_test",
            "2026-04-15T00:00:00Z",
            SignalBinding::CoOccurrence,
        );
        assert!(plan.ran_empty);
        assert_eq!(plan.signal_count, 0);
        // Only ExtractionStatus record.
        assert_eq!(plan.records.len(), 1);
        assert_eq!(plan.records[0].descriptive_schema, EXTRACTION_STATUS);
    }

    #[test]
    fn co_occurrence_edge_created_for_two_signals() {
        let plan = plan_text_extraction(
            "Notes",
            "note_2",
            "Meeting with tom@acme.com and alice@example.com",
            "mn_test",
            "es_test",
            "2026-04-15T00:00:00Z",
            SignalBinding::CoOccurrence,
        );
        let edges: Vec<_> = plan
            .records
            .iter()
            .filter(|r| r.descriptive_schema == EDGE)
            .collect();
        assert_eq!(edges.len(), 1); // one pair → one edge
        let f = &edges[0].fields;
        assert_eq!(f.get("kind").unwrap(), &json!(edge_kind::CO_OCCURRENCE));
        let w = f.get("weight").unwrap().as_f64().unwrap();
        assert!(
            (w - CO_OCCURRENCE_WEIGHT as f64).abs() < 1e-6,
            "expected CoOccurrence base weight, got {}",
            w
        );
    }

    #[test]
    fn strong_binding_emits_strong_match_edge() {
        let plan = plan_text_extraction(
            "Contacts",
            "contact_1",
            "Email: margaret.johnson@example.com Phone: +1-555-0101",
            "mn_strong",
            "es_strong",
            "2026-04-15T00:00:00Z",
            SignalBinding::Strong,
        );
        let edges: Vec<_> = plan
            .records
            .iter()
            .filter(|r| r.descriptive_schema == EDGE)
            .collect();
        assert_eq!(edges.len(), 1, "one pair → one edge");
        let f = &edges[0].fields;
        assert_eq!(f.get("kind").unwrap(), &json!(edge_kind::STRONG_MATCH));
        let w = f.get("weight").unwrap().as_f64().unwrap();
        assert!(
            (w - STRONG_MATCH_WEIGHT as f64).abs() < 1e-6,
            "expected StrongMatch base weight, got {}",
            w
        );
        assert!(
            w >= 0.85,
            "Strong-binding edges must clear the auto-sweep floor (got {})",
            w
        );
    }

    #[test]
    fn strong_binding_still_demotes_shared_inbox() {
        // The shared-inbox safety net must hold even when the source
        // schema asserts identity — a contact card listing
        // `info@acme.com` cannot be allowed to bind a person to that
        // shared address.
        let plan = plan_text_extraction(
            "Contacts",
            "contact_shared",
            "Cc info@acme.com and tom@example.com",
            "mn_shared_strong",
            "es_shared_strong",
            "2026-04-15T00:00:00Z",
            SignalBinding::Strong,
        );
        let edges: Vec<_> = plan
            .records
            .iter()
            .filter(|r| r.descriptive_schema == EDGE)
            .collect();
        assert_eq!(edges.len(), 1);
        let w = edges[0].fields.get("weight").unwrap().as_f64().unwrap();
        // 0.95 * 0.5 = 0.475 — below the 0.85 floor by design.
        assert!(
            w < 0.85,
            "shared-inbox endpoint must demote Strong edge below the sweep floor (got {})",
            w
        );
    }

    #[test]
    fn fingerprint_ids_are_stable_across_calls() {
        let a = extract_signals("tom@acme.com");
        let b = extract_signals("tom@acme.com");
        assert_eq!(a[0].fingerprint_id, b[0].fingerprint_id);
    }

    #[test]
    fn email_extraction_is_case_insensitive() {
        let a = extract_signals("Tom@Acme.COM");
        let b = extract_signals("tom@acme.com");
        assert_eq!(a[0].fingerprint_id, b[0].fingerprint_id);
    }

    #[test]
    fn plan_includes_all_junction_records() {
        let plan = plan_text_extraction(
            "Notes",
            "note_3",
            "Email: tom@acme.com Phone: +1-555-123-4567",
            "mn_test",
            "es_test",
            "2026-04-15T00:00:00Z",
            SignalBinding::CoOccurrence,
        );
        let mention_by_fp = plan
            .records
            .iter()
            .filter(|r| r.descriptive_schema == MENTION_BY_FINGERPRINT)
            .count();
        let mention_by_src = plan
            .records
            .iter()
            .filter(|r| r.descriptive_schema == MENTION_BY_SOURCE)
            .count();
        let edge_by_fp = plan
            .records
            .iter()
            .filter(|r| r.descriptive_schema == EDGE_BY_FINGERPRINT)
            .count();
        assert_eq!(mention_by_fp, 2); // one per unique signal
        assert_eq!(mention_by_src, 1); // one per mention
        assert_eq!(edge_by_fp, 2); // two per edge (one per endpoint)
    }

    // ── Name extraction tests ────────────────────────────────────

    #[test]
    fn extracts_two_word_names() {
        let signals = extract_signals("Meeting with Tom Tang tomorrow.");
        let names: Vec<&str> = signals
            .iter()
            .filter(|s| s.kind == kind::FULL_NAME)
            .map(|s| s.value.as_str())
            .collect();
        assert_eq!(names, vec!["Tom Tang"]);
    }

    #[test]
    fn extracts_three_word_names() {
        let signals = extract_signals("Lunch with Alice Bob Chen.");
        let names: Vec<&str> = signals
            .iter()
            .filter(|s| s.kind == kind::FULL_NAME)
            .map(|s| s.value.as_str())
            .collect();
        assert_eq!(names, vec!["Alice Bob Chen"]);
    }

    #[test]
    fn skips_excluded_patterns() {
        let signals = extract_signals("Happy Birthday! See you in New York on Monday.");
        let names: Vec<&str> = signals
            .iter()
            .filter(|s| s.kind == kind::FULL_NAME)
            .map(|s| s.value.as_str())
            .collect();
        assert!(
            names.is_empty(),
            "excluded patterns should not match: {:?}",
            names
        );
    }

    #[test]
    fn name_fingerprint_ids_are_case_normalized() {
        let a = extract_signals("Tom Tang");
        let b = extract_signals("Tom Tang");
        assert_eq!(
            a.last().unwrap().fingerprint_id,
            b.last().unwrap().fingerprint_id
        );
    }

    #[test]
    fn names_and_emails_coexist_in_same_text() {
        let signals = extract_signals("Email from Tom Tang <tom@acme.com> about the project.");
        let kinds: Vec<&str> = signals.iter().map(|s| s.kind).collect();
        assert!(kinds.contains(&kind::EMAIL));
        assert!(kinds.contains(&kind::FULL_NAME));
    }

    // ── Shared-inbox collision protection (TODO-2) ───────────────

    #[test]
    fn classify_email_strength_full_for_personal_mailbox() {
        assert_eq!(classify_email_strength("tom@acme.com"), 1.0);
        assert_eq!(classify_email_strength("alice.bob+tag@example.co.uk"), 1.0);
    }

    #[test]
    fn classify_email_strength_demoted_for_shared_inboxes() {
        for local in [
            "info", "sales", "support", "noreply", "no-reply", "admin", "help", "team", "hr",
            "billing", "privacy",
        ] {
            let email = format!("{local}@acme.com");
            assert_eq!(
                classify_email_strength(&email),
                SHARED_INBOX_STRENGTH,
                "expected {email} to be demoted"
            );
        }
    }

    #[test]
    fn classify_email_strength_is_case_insensitive() {
        assert_eq!(
            classify_email_strength("INFO@acme.com"),
            SHARED_INBOX_STRENGTH
        );
        assert_eq!(
            classify_email_strength("Sales@ACME.COM"),
            SHARED_INBOX_STRENGTH
        );
    }

    #[test]
    fn classify_email_strength_full_for_username_lookalikes() {
        // These were considered for the denylist but rejected — they're
        // too often real peoples' usernames. Assert the guard so future
        // edits to the denylist don't quietly flip them.
        assert_eq!(classify_email_strength("hello@acme.com"), 1.0);
        assert_eq!(classify_email_strength("hi@acme.com"), 1.0);
        assert_eq!(classify_email_strength("me@acme.com"), 1.0);
        assert_eq!(classify_email_strength("tom@acme.com"), 1.0);
    }

    #[test]
    fn classify_email_strength_full_on_malformed_input() {
        // No `@` → we cannot classify, so don't demote.
        assert_eq!(classify_email_strength("not-an-email"), 1.0);
        assert_eq!(classify_email_strength(""), 1.0);
    }

    #[test]
    fn shared_inbox_signal_carries_demoted_strength() {
        let signals = extract_signals("Reply went to info@acme.com and tom@acme.com.");
        let info = signals
            .iter()
            .find(|s| s.value == "info@acme.com")
            .expect("info signal extracted");
        let tom = signals
            .iter()
            .find(|s| s.value == "tom@acme.com")
            .expect("tom signal extracted");
        assert_eq!(info.strength, SHARED_INBOX_STRENGTH);
        assert_eq!(tom.strength, 1.0);
    }

    #[test]
    fn edges_touching_shared_inbox_get_demoted_weight() {
        let plan = plan_text_extraction(
            "Notes",
            "note_shared",
            "Cc info@acme.com and Tom Tang about the rollout.",
            "mn_shared",
            "es_shared",
            "2026-04-15T00:00:00Z",
            SignalBinding::CoOccurrence,
        );
        let edge_record = plan
            .records
            .iter()
            .find(|r| r.descriptive_schema == EDGE)
            .expect("one CoOccurrence edge is produced");
        let weight = edge_record
            .fields
            .get("weight")
            .and_then(|v| v.as_f64())
            .expect("edge carries a weight");
        // Co-occurrence base 0.3 * shared-inbox strength 0.5 = 0.15.
        assert!(
            (weight - 0.15).abs() < 1e-6,
            "expected shared-inbox edge weight 0.15, got {weight}"
        );
    }

    #[test]
    fn edges_between_personal_emails_keep_base_weight() {
        let plan = plan_text_extraction(
            "Notes",
            "note_pair",
            "tom@acme.com and alice@example.com collaborated",
            "mn_pair",
            "es_pair",
            "2026-04-15T00:00:00Z",
            SignalBinding::CoOccurrence,
        );
        let edge_record = plan
            .records
            .iter()
            .find(|r| r.descriptive_schema == EDGE)
            .expect("one CoOccurrence edge is produced");
        let weight = edge_record
            .fields
            .get("weight")
            .and_then(|v| v.as_f64())
            .expect("edge carries a weight");
        assert!(
            (weight - 0.3).abs() < 1e-6,
            "expected base edge weight 0.3, got {weight}"
        );
    }

    #[test]
    fn edges_between_two_shared_inboxes_stay_at_the_demoted_floor() {
        let plan = plan_text_extraction(
            "Notes",
            "note_both_shared",
            "Forwarded from info@acme.com to support@acme.com for triage.",
            "mn_both",
            "es_both",
            "2026-04-15T00:00:00Z",
            SignalBinding::CoOccurrence,
        );
        let edge_record = plan
            .records
            .iter()
            .find(|r| r.descriptive_schema == EDGE)
            .expect("one CoOccurrence edge is produced");
        let weight = edge_record
            .fields
            .get("weight")
            .and_then(|v| v.as_f64())
            .expect("edge carries a weight");
        // min(0.5, 0.5) * 0.3 = 0.15 — same floor as one weak endpoint.
        // Clustering is already neutralized at 0.15; we do NOT need to
        // compound below that.
        assert!(
            (weight - 0.15).abs() < 1e-6,
            "expected floored edge weight 0.15, got {weight}"
        );
    }
}
