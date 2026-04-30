//! Generic-ingest fingerprint hook — the single point that connects
//! ordinary user-data ingestion (Notes, Contacts, Photos, …) to the
//! fingerprint subsystem.
//!
//! Without this hook, only the dedicated `/api/fingerprints/*` endpoints
//! feed the identity graph. With it, every record written through
//! `IngestionService::execute_mutations_with_tracking` (which is the
//! ground floor for `/api/ingestion/process`, `/api/ingestion/batch-folder`,
//! `/api/ingestion/smart-folder/*`, the file-upload route, and the
//! Apple-import handlers) gets its text fields scanned for emails and
//! phone numbers, and any signals discovered are persisted as
//! `Fingerprint` / `Mention` / `Edge` records. After the loop, the
//! persona auto-sweep is fired off (debounced 30s in
//! `auto_propose::maybe_spawn_persona_sweep`), so a tentative Persona
//! emerges from the new identity graph as soon as one crosses the
//! `MIN_EDGE_WEIGHT` (0.85) floor.
//!
//! ## Invariants
//!
//! 1. **Best-effort.** Any per-record extraction failure logs at
//!    `warn` and does not propagate. The generic-ingest write already
//!    committed before this runs; we are purely opportunistic about
//!    identity extraction. A broken extractor must not break user
//!    data ingestion.
//!
//! 2. **Recursion-safe.** Mutations on the fingerprints subsystem's
//!    own schemas (Mention, Edge, Persona, …) are skipped — see
//!    [`crate::fingerprints::schemas::is_system_descriptive_schema`].
//!    So writing a Mention does not trigger another extraction pass
//!    over the Mention's text fields.
//!
//! 3. **Idempotent.** [`ingest_text_signals`] keys its Mention and
//!    ExtractionStatus on `(source_schema, source_key, "text_regex")`
//!    via the deterministic-id helpers, so re-ingesting the same
//!    record converges to the same row.
//!
//! 4. **Strong-binding allowlist.** Records on schemas that
//!    *structurally* assert identity (Contacts, CalendarEvent, …)
//!    emit `StrongMatch` (0.95) edges instead of the default
//!    `CoOccurrence` (0.3). See [`crate::fingerprints::schema_policy`].

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use fold_db::schema::types::Mutation;
use fold_db::schema::SchemaCore;
use serde_json::Value;

use crate::fingerprints::auto_propose;
use crate::fingerprints::ingest_text::ingest_text_signals;
use crate::fingerprints::schema_policy::binding_for_schema;
use crate::fingerprints::schemas::is_system_descriptive_schema;
use crate::fold_node::FoldNode;

/// Run the fingerprint extractor over every mutation in `mutations`
/// that targets a non-system schema, then spawn the persona sweep if
/// any extraction yielded ≥1 signal.
///
/// Returns the number of records that produced fingerprints — useful
/// for tests; runtime callers ignore it.
pub async fn run_after_batch(
    node: Arc<FoldNode>,
    schema_manager: Arc<SchemaCore>,
    mutations: &[Mutation],
) -> usize {
    if mutations.is_empty() {
        return 0;
    }

    let now_iso = Utc::now().to_rfc3339();
    let mut yielded = 0usize;

    for mutation in mutations {
        let descriptive = resolve_descriptive(&schema_manager, &mutation.schema_name);

        if is_system_descriptive_schema(&descriptive) {
            continue;
        }

        let text = collect_string_fields(&mutation.fields_and_values);
        if text.trim().is_empty() {
            continue;
        }

        let source_key = source_key_for(mutation);
        let binding = binding_for_schema(&descriptive);

        match ingest_text_signals(
            node.clone(),
            &descriptive,
            &source_key,
            &text,
            &now_iso,
            binding,
        )
        .await
        {
            Ok(outcome) => {
                if !outcome.ran_empty {
                    yielded += 1;
                }
            }
            Err(e) => {
                tracing::warn!(
                    target: "fold_node::ingestion::fingerprint_hook",
                    descriptive_schema = %descriptive,
                    source_key = %source_key,
                    error = %e,
                    "fingerprint extraction failed for record"
                );
            }
        }
    }

    if yielded > 0 {
        auto_propose::maybe_spawn_persona_sweep(node);
    }

    yielded
}

/// Resolve a mutation's runtime (canonical) schema name to its
/// descriptive name via the schema manager, falling back to the
/// runtime name itself if no schema is loaded under that name. The
/// fallback only matters for malformed mutations or schemas that
/// genuinely lack a `descriptive_name`; in both cases the worst case
/// is "we treated this like a non-system user schema and ran the
/// default CoOccurrence extractor over it" — recoverable, not
/// dangerous.
fn resolve_descriptive(schema_manager: &SchemaCore, runtime_name: &str) -> String {
    schema_manager
        .get_schema_metadata(runtime_name)
        .ok()
        .flatten()
        .and_then(|s| s.descriptive_name)
        .unwrap_or_else(|| runtime_name.to_string())
}

/// Build the text blob fed to the regex extractor by walking every
/// string-valued leaf in a mutation's fields. Non-string scalars
/// (numbers, bools) and nulls are skipped — the extractor scans for
/// emails and phone numbers, both of which are textual. Nested
/// objects and arrays are recursed.
///
/// Field names are deliberately *not* included. The extractor is
/// content-agnostic; including `"email"` in the haystack would never
/// match the email regex by itself.
fn collect_string_fields(fields: &HashMap<String, Value>) -> String {
    let mut buf = String::new();
    for value in fields.values() {
        push_strings(value, &mut buf);
    }
    buf
}

fn push_strings(value: &Value, buf: &mut String) {
    match value {
        Value::String(s) => {
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(s);
        }
        Value::Array(arr) => {
            for item in arr {
                push_strings(item, buf);
            }
        }
        Value::Object(map) => {
            for v in map.values() {
                push_strings(v, buf);
            }
        }
        _ => {}
    }
}

/// Stable source_key for the Mention this mutation produces. Uses the
/// mutation's `KeyValue` (formatted as `hash` or `hash:range` via its
/// Display impl) when set; otherwise falls back to the mutation uuid
/// so the extraction still proceeds — though that path loses
/// idempotency since the uuid is per-call.
fn source_key_for(mutation: &Mutation) -> String {
    let kv = format!("{}", mutation.key_value);
    if !kv.is_empty() {
        kv
    } else {
        mutation.uuid.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fields_with(values: &[(&str, Value)]) -> HashMap<String, Value> {
        values
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn collect_string_fields_concatenates_top_level_strings() {
        let fields = fields_with(&[
            ("name", json!("Margaret Johnson")),
            ("email", json!("margaret@example.com")),
            ("age", json!(62)), // non-string ignored
        ]);
        let text = collect_string_fields(&fields);
        assert!(text.contains("Margaret Johnson"));
        assert!(text.contains("margaret@example.com"));
        assert!(!text.contains("62"));
    }

    #[test]
    fn collect_string_fields_walks_nested_objects() {
        let fields = fields_with(&[
            (
                "address",
                json!({"street": "42 Oak Street", "city": "Springfield"}),
            ),
            ("contact", json!({"primary": "tom@acme.com"})),
        ]);
        let text = collect_string_fields(&fields);
        assert!(text.contains("42 Oak Street"));
        assert!(text.contains("Springfield"));
        assert!(text.contains("tom@acme.com"));
    }

    #[test]
    fn collect_string_fields_walks_arrays_of_objects() {
        let fields = fields_with(&[(
            "attendees",
            json!([
                {"name": "Alice", "email": "alice@example.com"},
                {"name": "Bob", "email": "bob@example.com"},
            ]),
        )]);
        let text = collect_string_fields(&fields);
        assert!(text.contains("alice@example.com"));
        assert!(text.contains("bob@example.com"));
    }

    #[test]
    fn collect_string_fields_returns_empty_for_no_strings() {
        let fields = fields_with(&[
            ("count", json!(7)),
            ("active", json!(true)),
            ("ratio", json!(0.5)),
        ]);
        let text = collect_string_fields(&fields);
        assert!(text.trim().is_empty());
    }

    #[test]
    fn collect_string_fields_skips_nulls() {
        let fields = fields_with(&[("name", json!("Tom")), ("optional", json!(null))]);
        let text = collect_string_fields(&fields);
        assert!(text.contains("Tom"));
    }

    #[test]
    fn source_key_uses_keyvalue_display_when_present() {
        use fold_db::schema::types::key_value::KeyValue;
        use fold_db::schema::types::operations::MutationType;

        let mutation = Mutation::new(
            "Contacts".to_string(),
            HashMap::new(),
            KeyValue::new(Some("contact_42".to_string()), None),
            "test_pubkey".to_string(),
            MutationType::Create,
        );
        assert_eq!(source_key_for(&mutation), "contact_42");
    }

    #[test]
    fn source_key_falls_back_to_uuid_when_keyvalue_empty() {
        use fold_db::schema::types::key_value::KeyValue;
        use fold_db::schema::types::operations::MutationType;

        let mutation = Mutation::new(
            "Contacts".to_string(),
            HashMap::new(),
            KeyValue::new(None, None),
            "test_pubkey".to_string(),
            MutationType::Create,
        );
        // Falls back to mutation uuid — non-empty, even though
        // idempotency-per-content is lost.
        assert_eq!(source_key_for(&mutation), mutation.uuid);
    }

    #[test]
    fn source_key_combines_hash_and_range() {
        use fold_db::schema::types::key_value::KeyValue;
        use fold_db::schema::types::operations::MutationType;

        let mutation = Mutation::new(
            "RangeSchema".to_string(),
            HashMap::new(),
            KeyValue::new(Some("user_a".to_string()), Some("2026-04-15".to_string())),
            "test_pubkey".to_string(),
            MutationType::Create,
        );
        assert_eq!(source_key_for(&mutation), "user_a:2026-04-15");
    }
}
