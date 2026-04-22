//! In-process AI call metrics, keyed by [`Role`].
//!
//! Every LLM call through [`AiBackend`](crate::ingestion::ai::client::AiBackend)
//! records a single entry here via the [`MeteredBackend`](crate::ingestion::ai::metered::MeteredBackend)
//! decorator. Counters are atomic, sharded per-role, and bounded (seven roles,
//! fixed size). No background thread, no persistence — counts reset on process
//! restart. The `/api/ingestion/stats` endpoint (PR 4) reads snapshots for the UI.
//!
//! Not a rolling window. The eng plan documents this explicitly: "since process
//! start, not 24h rolling" — the UI surfaces that caveat in the badge text.
//! If a real rolling window becomes necessary later, wrap each role entry in
//! `parking_lot::Mutex<VecDeque<CallRecord>>` and prune on read. Deferred.

use crate::ingestion::roles::Role;
use dashmap::DashMap;
use parking_lot::Mutex;
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// Process-wide AI metrics singleton. Lazily initialized on first call.
/// Every service (`IngestionService`, `LlmQueryService`, `anthropic_vision`,
/// etc.) records against the same store so `/api/ingestion/stats` returns
/// unified counts. Tests that need isolation build their own store via
/// [`AiMetricsStore::new`] and record directly — no global state leakage.
static GLOBAL: OnceLock<Arc<AiMetricsStore>> = OnceLock::new();

/// Per-role counters. Lock-free on the hot path (counters are atomics); the
/// mutex is only acquired to update `last_called_at` which is not on the
/// critical path of any user-visible latency.
#[derive(Debug, Default)]
pub struct RoleMetrics {
    /// Total LLM calls dispatched for this role since process start.
    pub call_count: AtomicU64,
    /// Sum of latencies in nanoseconds. Divided by `call_count` on read to
    /// produce `avg_latency_ms`.
    pub total_latency_ns: AtomicU64,
    /// Count of calls that returned Err.
    pub error_count: AtomicU64,
    /// Wall-clock time of the most recent call (any outcome). `None` if the
    /// role has not been exercised yet this process lifetime.
    last_called_at: Mutex<Option<Instant>>,
}

impl RoleMetrics {
    /// Record a completed call. Incrementing `call_count` second-to-last is
    /// deliberate: readers that race `snapshot` against `record_call` see a
    /// consistent moment (latency matches count).
    pub fn record(&self, latency: Duration, succeeded: bool) {
        // Use `as u64` with saturating_cast behavior — f64 would lose precision
        // but u128::min works. For realistic LLM call latencies (up to ~10min
        // = 6e11ns) this always fits in u64 with huge margin.
        let latency_ns = u64::try_from(latency.as_nanos()).unwrap_or(u64::MAX);
        self.total_latency_ns
            .fetch_add(latency_ns, Ordering::Relaxed);
        if !succeeded {
            self.error_count.fetch_add(1, Ordering::Relaxed);
        }
        // Increment call_count last so snapshots that see a non-zero count
        // also see a non-zero total_latency_ns (not the other way around).
        self.call_count.fetch_add(1, Ordering::Relaxed);
        *self.last_called_at.lock() = Some(Instant::now());
    }

    /// Produce a serializable snapshot. Computed on read, not stored.
    pub fn snapshot(&self, role: Role) -> RoleMetricsSnapshot {
        let count = self.call_count.load(Ordering::Relaxed);
        let total_ns = self.total_latency_ns.load(Ordering::Relaxed);
        let errors = self.error_count.load(Ordering::Relaxed);
        let avg_latency_ms = if count == 0 {
            0.0
        } else {
            // total_ns / count = avg_ns; divide by 1e6 for ms.
            (total_ns as f64 / count as f64) / 1_000_000.0
        };
        RoleMetricsSnapshot {
            role,
            call_count: count,
            avg_latency_ms,
            error_count: errors,
            last_called_elapsed_s: self
                .last_called_at
                .lock()
                .map(|t| t.elapsed().as_secs_f64()),
        }
    }
}

/// JSON-friendly snapshot of [`RoleMetrics`] for the `/api/ingestion/stats`
/// endpoint. `last_called_elapsed_s` is seconds since the role was last
/// exercised, or `None` if never — stable across clock changes unlike an
/// absolute timestamp.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct RoleMetricsSnapshot {
    pub role: Role,
    pub call_count: u64,
    pub avg_latency_ms: f64,
    pub error_count: u64,
    /// Seconds elapsed since the last call for this role. `None` if never called.
    pub last_called_elapsed_s: Option<f64>,
}

/// Process-lifetime AI metrics store. Shared via `Arc<AiMetricsStore>` and
/// injected into [`MeteredBackend`](crate::ingestion::ai::metered::MeteredBackend).
/// Small enough (at most 7 entries) that cloning the whole snapshot map is
/// cheap for the stats endpoint.
#[derive(Debug, Default)]
pub struct AiMetricsStore {
    inner: DashMap<Role, Arc<RoleMetrics>>,
}

impl AiMetricsStore {
    /// Build an empty store. Usually wrapped in `Arc` and shared with the
    /// ingestion service.
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the process-wide singleton metrics store, initializing on
    /// first call. Every production caller should use this. Tests that need
    /// isolation can build their own store via [`Self::new`] instead.
    pub fn global() -> Arc<AiMetricsStore> {
        GLOBAL
            .get_or_init(|| Arc::new(AiMetricsStore::new()))
            .clone()
    }

    /// Record a single call. Creates the per-role entry on first use.
    pub fn record_call(&self, role: Role, latency: Duration, succeeded: bool) {
        let entry = self
            .inner
            .entry(role)
            .or_insert_with(|| Arc::new(RoleMetrics::default()))
            .value()
            .clone();
        entry.record(latency, succeeded);
    }

    /// Snapshot the metrics for a single role. Returns a zero-valued snapshot
    /// for roles that have never been called (so the UI can render all seven
    /// rows without special-casing empty state).
    pub fn snapshot(&self, role: Role) -> RoleMetricsSnapshot {
        self.inner
            .get(&role)
            .map(|m| m.value().snapshot(role))
            .unwrap_or(RoleMetricsSnapshot {
                role,
                call_count: 0,
                avg_latency_ms: 0.0,
                error_count: 0,
                last_called_elapsed_s: None,
            })
    }

    /// Snapshot every role in [`Role::ALL`] order. Zero-valued entries for
    /// uncalled roles — always returns all 7 rows.
    pub fn snapshot_all(&self) -> Vec<RoleMetricsSnapshot> {
        Role::ALL.iter().map(|r| self.snapshot(*r)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn empty_store_returns_zero_snapshots_for_all_roles() {
        let store = AiMetricsStore::new();
        let all = store.snapshot_all();
        assert_eq!(all.len(), 7);
        for snap in &all {
            assert_eq!(snap.call_count, 0);
            assert_eq!(snap.avg_latency_ms, 0.0);
            assert_eq!(snap.error_count, 0);
            assert!(snap.last_called_elapsed_s.is_none());
        }
    }

    #[test]
    fn snapshot_all_returns_roles_in_canonical_order() {
        let store = AiMetricsStore::new();
        let all = store.snapshot_all();
        let order: Vec<Role> = all.iter().map(|s| s.role).collect();
        assert_eq!(order.as_slice(), Role::ALL);
    }

    #[test]
    fn record_call_creates_entry_and_increments_counters() {
        let store = AiMetricsStore::new();
        store.record_call(Role::IngestionText, Duration::from_millis(100), true);
        let snap = store.snapshot(Role::IngestionText);
        assert_eq!(snap.call_count, 1);
        assert_eq!(snap.error_count, 0);
        // 100ms within rounding tolerance
        assert!(
            (snap.avg_latency_ms - 100.0).abs() < 5.0,
            "got {}",
            snap.avg_latency_ms
        );
        assert!(snap.last_called_elapsed_s.is_some());
    }

    #[test]
    fn record_call_error_path_increments_error_count_and_call_count() {
        let store = AiMetricsStore::new();
        store.record_call(Role::SmartFolder, Duration::from_millis(50), false);
        let snap = store.snapshot(Role::SmartFolder);
        assert_eq!(snap.call_count, 1);
        assert_eq!(snap.error_count, 1);
    }

    #[test]
    fn average_latency_is_total_divided_by_count() {
        let store = AiMetricsStore::new();
        store.record_call(Role::QueryChat, Duration::from_millis(100), true);
        store.record_call(Role::QueryChat, Duration::from_millis(300), true);
        let snap = store.snapshot(Role::QueryChat);
        assert_eq!(snap.call_count, 2);
        // (100ms + 300ms) / 2 = 200ms
        assert!(
            (snap.avg_latency_ms - 200.0).abs() < 5.0,
            "got {}",
            snap.avg_latency_ms
        );
    }

    #[test]
    fn metrics_for_different_roles_are_independent() {
        let store = AiMetricsStore::new();
        store.record_call(Role::IngestionText, Duration::from_millis(100), true);
        store.record_call(Role::QueryChat, Duration::from_millis(500), false);
        let ingest = store.snapshot(Role::IngestionText);
        let query = store.snapshot(Role::QueryChat);
        assert_eq!(ingest.call_count, 1);
        assert_eq!(ingest.error_count, 0);
        assert_eq!(query.call_count, 1);
        assert_eq!(query.error_count, 1);
    }

    #[test]
    fn concurrent_records_from_100_threads_see_correct_total_count() {
        let store = Arc::new(AiMetricsStore::new());
        let threads: Vec<_> = (0..100)
            .map(|_| {
                let s = store.clone();
                thread::spawn(move || {
                    for _ in 0..50 {
                        s.record_call(Role::IngestionText, Duration::from_micros(1), true);
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let snap = store.snapshot(Role::IngestionText);
        // 100 threads × 50 calls = 5000 total. Atomic ordering guarantees.
        assert_eq!(snap.call_count, 5000);
        assert_eq!(snap.error_count, 0);
    }

    #[test]
    fn concurrent_mixed_roles_dont_race_between_role_entries() {
        // DashMap sharding per key — different roles must never clobber each
        // other's counts even under heavy contention.
        let store = Arc::new(AiMetricsStore::new());
        let roles = [Role::IngestionText, Role::Vision, Role::QueryChat];
        let mut threads = Vec::new();
        for role in roles {
            for _ in 0..20 {
                let s = store.clone();
                threads.push(thread::spawn(move || {
                    for _ in 0..25 {
                        s.record_call(role, Duration::from_micros(1), true);
                    }
                }));
            }
        }
        for t in threads {
            t.join().unwrap();
        }
        for role in roles {
            let snap = store.snapshot(role);
            assert_eq!(snap.call_count, 500, "role {:?} has wrong count", role);
        }
    }
}
