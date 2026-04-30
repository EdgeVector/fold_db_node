//! Auto-propose — post-ingest sweep that **creates** tentative
//! Personas directly from dense fingerprint clusters.
//!
//! ## Design shift vs. the earlier "suggestion count" approach
//!
//! An earlier iteration (closed PR #493) surfaced the sweep's output
//! as a badge count and left the user to manually click [Name it] in
//! a Suggestions panel. The current iteration takes the design to its
//! natural conclusion: a Persona is just a named query over the
//! fingerprint graph. Creating one commits no identity — that
//! boundary belongs to Identity Cards, not Personas. So the sweep
//! can safely **create** tentative Personas automatically without
//! ever crossing the "assumed vs verified" line.
//!
//! Auto-created Personas have `user_confirmed: false`. The UI renders
//! them with a "tentative" badge. The user can:
//!
//! - **Confirm** it (PATCH `user_confirmed: true`, optionally rename)
//! - **Delete** it (normal DELETE — not implemented here, out of scope)
//! - **Edit** it like any other Persona (threshold, relationship,
//!   exclusions, etc.) — all via existing PersonaPatch ops.
//!
//! The graph grows naturally over time: a new fingerprint connected
//! to an existing component via a strong edge shows up in the
//! tentative Persona's resolved cluster on the next query. No
//! persona-level update is needed because the resolver BFS-walks
//! from seeds every time.
//!
//! ## Idempotency
//!
//! The sweep uses `list_suggested_personas` internally, which
//! already filters out components covered by any existing Persona
//! (via `component_covered`). So re-running the sweep only creates
//! Personas for *new* uncovered components. Safe to call repeatedly.
//!
//! ## Debounce
//!
//! A 30-second debounce prevents back-to-back sweeps during a large
//! migration (ingesting 500 photos in one batch would otherwise
//! trigger 500 sweeps). First sweep in the window runs; the rest
//! are skipped until the window expires.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use fold_db::schema::types::key_value::KeyValue;
use fold_db::schema::types::operations::MutationType;
use serde_json::{json, Value};

use crate::fingerprints::canonical_names;
use crate::fingerprints::schemas::PERSONA;
use crate::fold_node::{FoldNode, OperationProcessor};
use crate::handlers::fingerprints::suggestions::list_suggested_personas;

/// Minimum time between two sweeps, in milliseconds. Back-to-back
/// ingests within this window share a single sweep.
const DEBOUNCE_MS: u64 = 30_000;

/// Default threshold for auto-created Personas. Matches the user-
/// accepted suggestion path in `accept_suggested_persona` so the
/// resolved cluster looks the same whether the Persona was created
/// by the sweep or by explicit user action.
const DEFAULT_THRESHOLD: f32 = 0.85;

/// Millisecond timestamp of the last sweep. 0 means "never swept".
static LAST_SWEEP_MS: AtomicU64 = AtomicU64::new(0);

/// Spawn the post-ingest persona sweep as a fire-and-forget background
/// task on the current tracing span. The sweep itself is internally
/// debounced (`DEBOUNCE_MS`), so back-to-back callers within that window
/// share a single sweep — which makes this safe to call from every
/// ingestion path without coordination.
///
/// This is the single entry point the rest of the codebase should use
/// after writing fingerprint records. Inlining `tokio::spawn` would
/// drift between callsites; centralizing here keeps the spawn pattern
/// (and any future enrichment, e.g. metrics) consistent.
pub fn maybe_spawn_persona_sweep(node: Arc<FoldNode>) {
    use tracing::Instrument;
    tokio::spawn(
        async move {
            run_sweep_and_create_personas(node).await;
        }
        .instrument(tracing::Span::current()),
    );
}

/// Summary returned by `run_sweep_and_create_personas` — useful for
/// logging and for tests. Not exposed over HTTP.
#[derive(Debug, Clone, Default)]
pub struct SweepOutcome {
    /// Number of candidate clusters the sweep found (post-gate).
    pub candidates: usize,
    /// Number of Personas actually created (may be less than
    /// `candidates` if some were already covered after a rebuild).
    pub created: usize,
    /// True when the debounce guard suppressed the sweep.
    pub skipped_debounced: bool,
}

/// Run the sweep and auto-create a Persona for every uncovered
/// candidate cluster. Fire-and-forget callers ignore the return
/// value; tests can observe it.
pub async fn run_sweep_and_create_personas(node: Arc<FoldNode>) -> SweepOutcome {
    let now_ms = now_millis();
    let last_ms = LAST_SWEEP_MS.load(Ordering::Acquire);
    if now_ms.saturating_sub(last_ms) < DEBOUNCE_MS {
        tracing::debug!(
            "auto_propose: skipping sweep — last ran {}ms ago (debounce {}ms)",
            now_ms - last_ms,
            DEBOUNCE_MS
        );
        return SweepOutcome {
            skipped_debounced: true,
            ..Default::default()
        };
    }
    // Claim the slot BEFORE running so concurrent callers bail.
    LAST_SWEEP_MS.store(now_ms, Ordering::Release);

    let started = std::time::Instant::now();
    let response = match list_suggested_personas(node.clone()).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("auto_propose: sweep failed: {}", e);
            return SweepOutcome::default();
        }
    };
    let suggestions = match response.data {
        Some(d) => d.suggestions,
        None => return SweepOutcome::default(),
    };
    let candidates = suggestions.len();

    let processor = OperationProcessor::new(node.clone());
    let persona_canonical = match canonical_names::lookup(PERSONA) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                "auto_propose: canonical_names missing '{}'; skipping create phase: {}",
                PERSONA,
                e
            );
            return SweepOutcome {
                candidates,
                ..Default::default()
            };
        }
    };

    let now = chrono::Utc::now().to_rfc3339();
    let mut created = 0usize;
    for sug in suggestions {
        let persona_id = format!("ps_auto_{}", uuid::Uuid::new_v4().simple());
        let fields = build_tentative_persona_fields(
            &persona_id,
            &sug.suggested_name,
            &sug.fingerprint_ids,
            &now,
        );
        match processor
            .execute_mutation(
                persona_canonical.clone(),
                fields,
                KeyValue::new(Some(persona_id.clone()), None),
                MutationType::Create,
            )
            .await
        {
            Ok(_) => {
                created += 1;
                tracing::info!(
                    "auto_propose: auto-created tentative persona '{}' ({}): {} seeds",
                    persona_id,
                    sug.suggested_name,
                    sug.fingerprint_ids.len()
                );
            }
            Err(e) => {
                tracing::warn!(
                    "auto_propose: failed to auto-create persona for cluster {}: {}",
                    sug.suggested_id,
                    e
                );
            }
        }
    }

    tracing::info!(
        "auto_propose: sweep complete — {} candidates, {} created in {:?}",
        candidates,
        created,
        started.elapsed()
    );

    SweepOutcome {
        candidates,
        created,
        skipped_debounced: false,
    }
}

/// Build the field map for a tentative Persona record. Mirrors the
/// layout in `accept_suggested_persona` except:
///
/// - `user_confirmed: false` (vs. true for explicit accept)
/// - `aliases: ["__auto__"]` — lightweight provenance marker so the
///   UI can distinguish "the sweep made this" from "the user made
///   this via accept" without a schema field change
pub fn build_tentative_persona_fields(
    persona_id: &str,
    suggested_name: &str,
    fingerprint_ids: &[String],
    now: &str,
) -> HashMap<String, Value> {
    let mut m = HashMap::new();
    m.insert("id".to_string(), json!(persona_id));
    m.insert("name".to_string(), json!(suggested_name));
    m.insert("seed_fingerprint_ids".to_string(), json!(fingerprint_ids));
    m.insert("threshold".to_string(), json!(DEFAULT_THRESHOLD));
    m.insert(
        "excluded_mention_ids".to_string(),
        json!(Vec::<String>::new()),
    );
    m.insert("excluded_edge_ids".to_string(), json!(Vec::<String>::new()));
    m.insert(
        "included_mention_ids".to_string(),
        json!(Vec::<String>::new()),
    );
    m.insert("aliases".to_string(), json!(vec!["__auto__"]));
    m.insert("relationship".to_string(), json!("unknown"));
    m.insert("trust_tier".to_string(), json!(0));
    m.insert("identity_id".to_string(), Value::Null);
    m.insert("user_confirmed".to_string(), json!(false));
    m.insert("built_in".to_string(), json!(false));
    m.insert("created_at".to_string(), json!(now));
    m
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tentative_persona_fields_have_correct_flags() {
        let fields = build_tentative_persona_fields(
            "ps_auto_xyz",
            "Tom Tang",
            &["fp_a".to_string(), "fp_b".to_string()],
            "2026-04-17T00:00:00Z",
        );
        assert_eq!(fields.get("user_confirmed").unwrap(), &json!(false));
        assert_eq!(fields.get("built_in").unwrap(), &json!(false));
        assert_eq!(fields.get("relationship").unwrap(), &json!("unknown"));
        assert_eq!(fields.get("name").unwrap(), &json!("Tom Tang"));
        assert_eq!(fields.get("id").unwrap(), &json!("ps_auto_xyz"));
        assert_eq!(fields.get("aliases").unwrap(), &json!(vec!["__auto__"]));
    }

    #[test]
    fn tentative_persona_id_starts_with_ps_auto_prefix() {
        // The auto-create path uses `ps_auto_<uuid>` prefixes so the
        // persona list page can distinguish tentative from
        // user-confirmed at a glance without fetching full records.
        let fields = build_tentative_persona_fields(
            "ps_auto_abc123",
            "Cluster",
            &["fp_1".to_string()],
            "2026-04-17T00:00:00Z",
        );
        assert!(fields
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap()
            .starts_with("ps_auto_"));
    }

    #[test]
    fn seed_fingerprint_ids_preserved_verbatim() {
        let seeds = vec!["fp_a".to_string(), "fp_b".to_string(), "fp_c".to_string()];
        let fields =
            build_tentative_persona_fields("ps_auto_test", "X", &seeds, "2026-04-17T00:00:00Z");
        assert_eq!(
            fields.get("seed_fingerprint_ids").unwrap(),
            &json!(vec!["fp_a", "fp_b", "fp_c"])
        );
    }

    // Compile-time debounce sanity.
    const _: () = assert!(DEBOUNCE_MS > 0);
    const _: () = assert!(DEBOUNCE_MS < 300_000);
}
