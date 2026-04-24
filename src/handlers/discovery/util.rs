//! Shared utility helpers for the discovery handlers.
//!
//! This module holds state-machine and dedup-marker helpers that are used by
//! both the inbound message pipeline (`inbound.rs`) and outbound operations
//! such as `connect` (in `mod.rs`). Keeping them in a dedicated module avoids
//! a circular `inbound -> outbound -> inbound` dependency.

use super::get_metadata_store;
use crate::discovery::config;
use crate::fold_node::node::FoldNode;
use crate::handlers::response::{get_db_guard, HandlerError, IntoHandlerError};

// =========================================================================
// Connect in-flight sentinel (shared between outbound `connect` and tests)
// =========================================================================

pub(super) const CONNECT_IN_FLIGHT_PREFIX: &str = "discovery:connect_in_flight:";

/// TTL on the in-flight sentinel. If the guarded flow dies mid-way (crash,
/// panic, dropped future) without releasing, the next attempt after this
/// many seconds will treat the sentinel as stale and overwrite it. Long
/// enough to cover a network round trip, short enough that a legitimate
/// retry isn't blocked.
pub(super) const CONNECT_IN_FLIGHT_TTL_SECS: i64 = 60;

/// Outcome of attempting to acquire the per-target connect sentinel.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SentinelAcquire {
    /// Sentinel acquired (either fresh or stale-overwritten). Caller owns it
    /// and must release on every exit path.
    Acquired,
    /// Another fresh sentinel already exists — caller must reject the request.
    InFlight,
}

/// Try to acquire the per-target in-flight sentinel in the Sled store.
/// See `connect` for rationale. Pure helper, unit-testable.
pub(crate) async fn try_acquire_connect_sentinel(
    store: &dyn fold_db::storage::traits::KvStore,
    target_pseudonym: &str,
    now_ts: i64,
    ttl_secs: i64,
) -> Result<SentinelAcquire, HandlerError> {
    let key = format!("{}{}", CONNECT_IN_FLIGHT_PREFIX, target_pseudonym);
    if let Some(existing) = store
        .get(key.as_bytes())
        .await
        .map_err(|e| HandlerError::Internal(format!("sentinel read: {e}")))?
    {
        let existing_ts = std::str::from_utf8(&existing)
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
            .ok_or_else(|| {
                HandlerError::Internal("corrupt connect_in_flight sentinel value".to_string())
            })?;
        if now_ts - existing_ts < ttl_secs {
            return Ok(SentinelAcquire::InFlight);
        }
        // Stale: previous attempt died mid-flight. Fall through and overwrite.
    }
    store
        .put(key.as_bytes(), now_ts.to_string().into_bytes())
        .await
        .map_err(|e| HandlerError::Internal(format!("sentinel write: {e}")))?;
    Ok(SentinelAcquire::Acquired)
}

/// Release the per-target in-flight sentinel. Best-effort — a release failure
/// is logged but does not mask the caller's primary result, because the
/// sentinel self-expires after `CONNECT_IN_FLIGHT_TTL_SECS`.
pub(crate) async fn release_connect_sentinel(
    store: &dyn fold_db::storage::traits::KvStore,
    target_pseudonym: &str,
) {
    let key = format!("{}{}", CONNECT_IN_FLIGHT_PREFIX, target_pseudonym);
    if let Err(e) = store.delete(key.as_bytes()).await {
        log::warn!(
            "Failed to release connect_in_flight sentinel for {}: {}",
            target_pseudonym,
            e
        );
    }
}

// =========================================================================
// Published pseudonym collection (shared between inbound poll + mod my_pseudonyms)
// =========================================================================

/// Collect all pseudonyms this node publishes. Used by the request poller and
/// by the `my_pseudonyms` handler (test-framework cleanup). The derivation
/// must stay in sync with `publisher.rs`.
pub(crate) async fn collect_our_pseudonyms(
    node: &FoldNode,
    master_key: &[u8],
) -> Result<Vec<uuid::Uuid>, HandlerError> {
    let db = get_db_guard(node)?;
    let db_ops = db.get_db_ops();
    let store = get_metadata_store(&db);

    let configs = config::list_opt_ins(&*store)
        .await
        .handler_err("list opt-ins")?;

    let mut pseudonyms = Vec::new();

    // Add our connection-sender pseudonym (fallback used by connect handler when no opt-ins)
    let hash = crate::discovery::pseudonym::content_hash("connection-sender");
    pseudonyms.push(crate::discovery::pseudonym::derive_pseudonym(
        master_key, &hash,
    ));

    // Add our schema-name-derived sender pseudonyms. The connect handler uses
    // derive_pseudonym(master_key, content_hash(first_opt_in.schema_name)) as
    // sender_pseudonym. When someone replies or shares data to that pseudonym,
    // we need to poll for it. Without this, data shares never reach their target.
    for cfg in &configs {
        let schema_hash = crate::discovery::pseudonym::content_hash(&cfg.schema_name);
        pseudonyms.push(crate::discovery::pseudonym::derive_pseudonym(
            master_key,
            &schema_hash,
        ));
    }

    // Add pseudonyms derived from actual published embeddings (same as publisher.rs)
    let native_index_mgr = db_ops.native_index_manager();
    if let Some(nim) = native_index_mgr {
        let embedding_store = nim.store().clone();
        for cfg in &configs {
            let prefix = format!("emb:{}:", cfg.schema_name);
            if let Ok(raw_entries) = embedding_store.scan_prefix(prefix.as_bytes()).await {
                for (_key, value) in &raw_entries {
                    if let Ok(stored) = serde_json::from_slice::<serde_json::Value>(value) {
                        if let Some(emb_arr) = stored.get("embedding").and_then(|e| e.as_array()) {
                            let embedding_bytes: Vec<u8> = emb_arr
                                .iter()
                                .filter_map(|v| v.as_f64().map(|f| f as f32))
                                .flat_map(|f| f.to_le_bytes())
                                .collect();
                            let content_hash =
                                crate::discovery::pseudonym::content_hash_bytes(&embedding_bytes);
                            pseudonyms.push(crate::discovery::pseudonym::derive_pseudonym(
                                master_key,
                                &content_hash,
                            ));
                        }
                    }
                }
            }
        }
    }

    pseudonyms.sort();
    pseudonyms.dedup();
    // NOTE: previously truncated to 1000 here with a "URL length limit" comment,
    // but that was misleading — the poll request only sends pseudonyms to the
    // server when `our_pseudonyms.len() <= 100` (otherwise it passes None and
    // filters client-side). The truncate silently dropped decrypt keys beyond
    // the 1000th, causing addressed messages to be missed. No cap needed:
    // pseudonyms are 16 bytes each, and the server-filter branch already guards
    // URL length.
    log::debug!(
        "our_pseudonyms[0..5]: {:?}",
        pseudonyms
            .iter()
            .take(5)
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
    );
    Ok(pseudonyms)
}

// =========================================================================
// Dedup marker helpers (G2 prune)
// =========================================================================
//
// Every processed bulletin-board message leaves a `msg_processed:{id}` marker
// so we don't re-dispatch it on subsequent polls. Without a bound the
// key-space grows forever. We prune entries older than
// `DEDUP_RETENTION_SECS` every `PRUNE_EVERY_N_POLLS` invocations of
// `poll_and_decrypt_requests`.
//
// The marker value is an 8-byte little-endian u64 seconds timestamp. The
// older marker format was `b"1"`; malformed/short values are treated as stale
// (age = 0 at deploy, then immediately older than retention on subsequent
// prunes) and are deleted on the next prune pass.

pub(super) const MSG_PROCESSED_PREFIX: &str = "msg_processed:";
/// Retain dedup markers for 7 days. Bulletin-board messages in DynamoDB have a
/// shorter TTL anyway; a reappearing 7-day-old message is safe to re-dispatch.
pub(super) const DEDUP_RETENTION_SECS: u64 = 7 * 24 * 60 * 60;
/// Run the prune scan every N-th call. A simple in-memory atomic counter avoids
/// a separate background task.
pub(super) const PRUNE_EVERY_N_POLLS: u64 = 50;

pub(super) static PRUNE_POLL_COUNTER: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

pub(super) fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub(super) fn encode_marker_timestamp(secs: u64) -> Vec<u8> {
    secs.to_le_bytes().to_vec()
}

/// Decode a dedup marker value into a wall-clock seconds timestamp.
/// Returns `None` for legacy/malformed markers (pre-G2 wrote `b"1"`).
pub(super) fn decode_marker_timestamp(value: &[u8]) -> Option<u64> {
    if value.len() != 8 {
        return None;
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(value);
    Some(u64::from_le_bytes(buf))
}

/// Delete dedup markers older than `DEDUP_RETENTION_SECS`. Legacy markers
/// (non-8-byte values written before this fix) are also deleted — they're
/// known to be at most ≤7 days old at deploy time and are safe to drop.
///
/// Uses the `KvStore::scan_prefix` method which loads all matching entries
/// at once (no streaming API exists on the trait). The marker key-space is
/// bounded by recent bulletin-board traffic, so a full load is fine.
pub(crate) async fn prune_msg_processed_markers(
    store: &dyn fold_db::storage::traits::KvStore,
    now: u64,
    retention_secs: u64,
) -> Result<usize, String> {
    let entries = store
        .scan_prefix(MSG_PROCESSED_PREFIX.as_bytes())
        .await
        .map_err(|e| format!("scan_prefix failed: {e}"))?;

    let mut deleted = 0usize;
    for (key, value) in entries {
        let stale = match decode_marker_timestamp(&value) {
            Some(ts) => now.saturating_sub(ts) > retention_secs,
            // Legacy/malformed marker — drop it.
            None => true,
        };
        if stale {
            store
                .delete(&key)
                .await
                .map_err(|e| format!("delete failed: {e}"))?;
            deleted += 1;
        }
    }
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fold_db::storage::inmemory_backend::InMemoryKvStore;
    use fold_db::storage::traits::KvStore;

    #[tokio::test]
    async fn prune_deletes_old_markers_and_keeps_fresh_ones() {
        let store = InMemoryKvStore::default();
        let now: u64 = 1_700_000_000;

        // Fresh marker (1 hour old)
        let fresh_key = format!("{}fresh", MSG_PROCESSED_PREFIX);
        store
            .put(fresh_key.as_bytes(), encode_marker_timestamp(now - 3_600))
            .await
            .unwrap();

        // Stale marker (8 days old)
        let stale_key = format!("{}stale", MSG_PROCESSED_PREFIX);
        store
            .put(
                stale_key.as_bytes(),
                encode_marker_timestamp(now - 8 * 24 * 60 * 60),
            )
            .await
            .unwrap();

        // Legacy marker (pre-G2 `b"1"`) — treated as malformed and deleted.
        let legacy_key = format!("{}legacy", MSG_PROCESSED_PREFIX);
        store
            .put(legacy_key.as_bytes(), b"1".to_vec())
            .await
            .unwrap();

        // Unrelated key — must not be touched by prefix scan.
        store.put(b"other:untouched", b"x".to_vec()).await.unwrap();

        let deleted = prune_msg_processed_markers(&store, now, DEDUP_RETENTION_SECS)
            .await
            .unwrap();
        assert_eq!(deleted, 2, "stale + legacy should be deleted");

        assert!(store.get(fresh_key.as_bytes()).await.unwrap().is_some());
        assert!(store.get(stale_key.as_bytes()).await.unwrap().is_none());
        assert!(store.get(legacy_key.as_bytes()).await.unwrap().is_none());
        assert!(store.get(b"other:untouched").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn prune_noop_when_all_fresh() {
        let store = InMemoryKvStore::default();
        let now: u64 = 1_700_000_000;
        for i in 0..5 {
            let k = format!("{}msg-{}", MSG_PROCESSED_PREFIX, i);
            store
                .put(k.as_bytes(), encode_marker_timestamp(now - 60))
                .await
                .unwrap();
        }
        let deleted = prune_msg_processed_markers(&store, now, DEDUP_RETENTION_SECS)
            .await
            .unwrap();
        assert_eq!(deleted, 0);
    }

    // ===== connect sentinel tests (FU-2) =====

    #[tokio::test]
    async fn sentinel_first_acquire_succeeds() {
        let store = InMemoryKvStore::default();
        let store_ref: &dyn KvStore = &store;
        let target = "11111111-1111-1111-1111-111111111111";
        let outcome = try_acquire_connect_sentinel(store_ref, target, 1_000_000, 60)
            .await
            .expect("first acquire must not error");
        assert_eq!(outcome, SentinelAcquire::Acquired);
        // Sentinel key present.
        let key = format!("{}{}", CONNECT_IN_FLIGHT_PREFIX, target);
        assert!(store_ref.get(key.as_bytes()).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn sentinel_second_acquire_within_ttl_rejects() {
        let store = InMemoryKvStore::default();
        let store_ref: &dyn KvStore = &store;
        let target = "22222222-2222-2222-2222-222222222222";
        let first = try_acquire_connect_sentinel(store_ref, target, 1_000_000, 60)
            .await
            .unwrap();
        assert_eq!(first, SentinelAcquire::Acquired);
        // Only 10 seconds have passed — still in flight.
        let second = try_acquire_connect_sentinel(store_ref, target, 1_000_010, 60)
            .await
            .unwrap();
        assert_eq!(second, SentinelAcquire::InFlight);
    }

    #[tokio::test]
    async fn sentinel_stale_is_overwritten() {
        let store = InMemoryKvStore::default();
        let store_ref: &dyn KvStore = &store;
        let target = "33333333-3333-3333-3333-333333333333";
        try_acquire_connect_sentinel(store_ref, target, 1_000_000, 60)
            .await
            .unwrap();
        // Clock has advanced 2 minutes. Previous sentinel is stale.
        let second = try_acquire_connect_sentinel(store_ref, target, 1_000_120, 60)
            .await
            .unwrap();
        assert_eq!(second, SentinelAcquire::Acquired);
        // The stored timestamp should be the new one.
        let key = format!("{}{}", CONNECT_IN_FLIGHT_PREFIX, target);
        let value = store_ref.get(key.as_bytes()).await.unwrap().unwrap();
        assert_eq!(std::str::from_utf8(&value).unwrap(), "1000120");
    }

    #[tokio::test]
    async fn sentinel_release_clears_key_so_next_acquire_succeeds() {
        let store = InMemoryKvStore::default();
        let store_ref: &dyn KvStore = &store;
        let target = "44444444-4444-4444-4444-444444444444";
        try_acquire_connect_sentinel(store_ref, target, 1_000_000, 60)
            .await
            .unwrap();
        release_connect_sentinel(store_ref, target).await;
        let key = format!("{}{}", CONNECT_IN_FLIGHT_PREFIX, target);
        assert!(store_ref.get(key.as_bytes()).await.unwrap().is_none());
        // And a fresh acquire at the same logical time now succeeds.
        let again = try_acquire_connect_sentinel(store_ref, target, 1_000_005, 60)
            .await
            .unwrap();
        assert_eq!(again, SentinelAcquire::Acquired);
    }

    #[tokio::test]
    async fn sentinel_corrupt_value_errors_loudly() {
        let store = InMemoryKvStore::default();
        let store_ref: &dyn KvStore = &store;
        let target = "55555555-5555-5555-5555-555555555555";
        let key = format!("{}{}", CONNECT_IN_FLIGHT_PREFIX, target);
        store_ref
            .put(key.as_bytes(), b"not-a-timestamp".to_vec())
            .await
            .unwrap();
        let err = try_acquire_connect_sentinel(store_ref, target, 1_000_000, 60)
            .await
            .expect_err("corrupt sentinel must surface as an error, not silently overwrite");
        match err {
            HandlerError::Internal(msg) => assert!(msg.contains("corrupt")),
            other => panic!("expected Internal error, got {other:?}"),
        }
    }
}
