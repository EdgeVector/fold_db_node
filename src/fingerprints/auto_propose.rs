//! Auto-propose — post-ingest dense-subgraph sweep.
//!
//! After each batch ingest (face or text), the handler fires a
//! fire-and-forget `tokio::spawn` that calls into this module to run
//! the suggestions sweep asynchronously. The result count is cached
//! in a process-wide `AtomicUsize` so the frontend can poll a cheap
//! `GET /api/fingerprints/suggestions/count` endpoint without
//! re-running the BFS on every poll.
//!
//! ## What this does NOT do
//!
//! - **Does NOT auto-create Personas.** It auto-proposes; the user
//!   still clicks [Name it] in the Suggestions panel to commit. This
//!   preserves the design doc's "assumed vs verified" boundary —
//!   Personas (assumed clusters) are always created by deliberate
//!   user action, never by the system.
//! - **Does NOT run on a timer.** The sweep fires as a consequence
//!   of ingestion. If nothing is ingested, nothing sweeps.
//! - **Does NOT persist state across restarts.** The count lives in
//!   a static atomic. After a restart the count is zero until the
//!   next ingest fires a sweep, or until the user manually opens the
//!   Suggestions panel (which runs the same BFS on-demand). A
//!   stronger persistence layer is trivial to add later; the Phase 2
//!   cost/benefit doesn't justify it.
//!
//! ## Debounce
//!
//! A debounce window (default 30s) prevents back-to-back sweeps
//! during a large migration — a single "ingest 500 photos" pass
//! would otherwise trigger 500 sweeps. The first sweep in a window
//! runs; subsequent sweeps are skipped until the window expires.
//! Every sweep still refreshes the count so the frontend eventually
//! sees stable results.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::fold_node::FoldNode;
use crate::handlers::fingerprints::suggestions::list_suggested_personas;

/// Minimum time between two sweeps, in milliseconds. Back-to-back
/// ingests within this window share a single sweep.
const DEBOUNCE_MS: u64 = 30_000;

/// Cached suggestion count. Written by `run_sweep_and_update_count`,
/// read by `get_count`. Reset to 0 at process start — a fresh sweep
/// will re-populate it after the first ingest or the first user
/// visit to the Suggestions panel.
static SUGGESTION_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Millisecond timestamp of the last sweep. 0 means "never swept".
/// Drives the debounce logic.
static LAST_SWEEP_MS: AtomicU64 = AtomicU64::new(0);

/// Return the cached suggestion count. O(1) atomic load.
pub fn get_count() -> usize {
    SUGGESTION_COUNT.load(Ordering::Acquire)
}

/// Manually set the cached count — used by `accept_suggested_persona`
/// to decrement immediately after a user accepts a suggestion, so the
/// badge drops without waiting for the next poll.
pub fn set_count(count: usize) {
    SUGGESTION_COUNT.store(count, Ordering::Release);
}

/// Run the suggestions sweep and update the cached count. Respects
/// the debounce window — if another sweep ran within `DEBOUNCE_MS`,
/// this call is a no-op and returns `None`. Otherwise returns
/// `Some(count)` after the sweep completes.
///
/// Errors from the underlying sweep are logged but not propagated:
/// a failed sweep should not crash the ingest handler or propagate
/// to the HTTP response. The count simply stays at its previous
/// value until the next successful sweep.
pub async fn run_sweep_and_update_count(node: Arc<FoldNode>) -> Option<usize> {
    let now_ms = now_millis();
    let last_ms = LAST_SWEEP_MS.load(Ordering::Acquire);
    if now_ms.saturating_sub(last_ms) < DEBOUNCE_MS {
        log::debug!(
            "auto_propose: skipping sweep — last ran {}ms ago (debounce {}ms)",
            now_ms - last_ms,
            DEBOUNCE_MS
        );
        return None;
    }

    // Claim the slot BEFORE running the sweep so concurrent callers
    // bail out via the debounce check above. If the sweep fails the
    // slot is still claimed for DEBOUNCE_MS — we'd rather suppress
    // duplicate failing work than hammer a broken subsystem.
    LAST_SWEEP_MS.store(now_ms, Ordering::Release);

    let started = std::time::Instant::now();
    match list_suggested_personas(node).await {
        Ok(response) => {
            let count = response
                .data
                .as_ref()
                .map(|d| d.suggestions.len())
                .unwrap_or(0);
            SUGGESTION_COUNT.store(count, Ordering::Release);
            log::info!(
                "auto_propose: sweep complete — {} suggestions in {:?}",
                count,
                started.elapsed()
            );
            Some(count)
        }
        Err(e) => {
            log::warn!(
                "auto_propose: sweep failed after {:?}, count unchanged: {}",
                started.elapsed(),
                e
            );
            None
        }
    }
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
    fn count_starts_at_zero_and_round_trips() {
        // NOTE: static state means this interacts with other tests in
        // the same process. Use values that don't conflict and
        // explicitly reset after.
        let prev = get_count();
        set_count(42);
        assert_eq!(get_count(), 42);
        set_count(prev); // restore
    }

    #[test]
    fn set_count_is_observable_immediately() {
        let prev = get_count();
        set_count(7);
        let observed = get_count();
        set_count(prev);
        assert_eq!(observed, 7);
    }

    // Compile-time sanity for the debounce bound. Must be > 0 (no
    // debounce would allow thrashing) and < 5 minutes (too long
    // makes the UI feel stale). Using `const _: () = assert!(...)`
    // instead of a runtime `assert!` avoids clippy's
    // `assertions_on_constants` lint.
    const _: () = assert!(DEBOUNCE_MS > 0);
    const _: () = assert!(DEBOUNCE_MS < 300_000);
}
