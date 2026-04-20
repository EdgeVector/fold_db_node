//! Snapshot backup/restore handlers.
//!
//! Framework-agnostic orchestration around `SyncEngine::backup_snapshot` and
//! `SyncEngine::bootstrap_all`. The HTTP routes in `server/routes/snapshot.rs`
//! wrap these; CLI commands (M3) will wrap them the same way.
//!
//! A backup uploads an encrypted snapshot of the current local Sled store to
//! `{user_hash}/snapshots/latest.enc` (and `{seq}.enc`). A restore downloads
//! `latest.enc`, decrypts, and replays into Sled — the same bootstrap path
//! that runs on a new device after BIP39 identity restore.
//!
//! Sync must already be enabled (cloud mode). Callers in local-only mode
//! get a BadRequest back.
//!
//! Boot-time empty-store restore is handled by the existing identity-restore
//! pathway in `handlers::auth::bootstrap_from_cloud`, which is invoked after
//! `folddb restore --phrase`. These handlers are for on-demand backup and
//! for re-running restore against a running node.

use crate::fold_node::node::FoldNode;
use crate::handlers::handler_response;
use crate::handlers::response::{
    ApiResponse, HandlerError, HandlerResult, IntoHandlerError, IntoTypedHandlerError,
};

handler_response! {
    pub struct SnapshotBackupResponse {
        pub success: bool,
        pub seq: u64,
        pub message: String,
    }
}

handler_response! {
    pub struct SnapshotRestoreResponse {
        pub success: bool,
        pub targets_restored: usize,
        pub entries_replayed: usize,
        pub message: String,
    }
}

/// Create a snapshot of the current local store and upload it to the cloud.
///
/// Requires sync to be enabled (Exemem credentials configured).
pub async fn backup(user_hash: &str, node: &FoldNode) -> HandlerResult<SnapshotBackupResponse> {
    let db = node.get_fold_db().typed_handler_err()?;
    let engine = db.sync_engine().ok_or_else(|| {
        HandlerError::BadRequest(
            "Snapshot backup requires cloud sync to be enabled. \
             Configure Exemem credentials first."
                .to_string(),
        )
    })?;

    let seq = engine
        .backup_snapshot()
        .await
        .handler_err("backup snapshot")?;

    Ok(ApiResponse::success_with_user(
        SnapshotBackupResponse {
            success: true,
            seq,
            message: format!("Snapshot uploaded at seq {seq}"),
        },
        user_hash,
    ))
}

/// Restore from the most recent snapshot in the cloud and replay log deltas.
///
/// Uses `SyncEngine::bootstrap_all` so personal + any configured org targets
/// are all restored in one call.
pub async fn restore(user_hash: &str, node: &FoldNode) -> HandlerResult<SnapshotRestoreResponse> {
    let db = node.get_fold_db().typed_handler_err()?;
    let engine = db.sync_engine().ok_or_else(|| {
        HandlerError::BadRequest(
            "Snapshot restore requires cloud sync to be enabled. \
             Configure Exemem credentials first."
                .to_string(),
        )
    })?;

    let outcomes = engine
        .bootstrap_all()
        .await
        .handler_err("restore from snapshot")?;

    let entries_replayed: usize = outcomes.iter().map(|o| o.entries_replayed).sum();
    let targets_restored = outcomes.len();

    // bootstrap_all only invokes the SchemaCore reloader when post-snapshot
    // log entries included schemas. When the restore is snapshot-only (0 log
    // deltas on top — the common case) the reloader never fires, leaving the
    // in-memory schema cache stale so `GET /api/schemas` can't see restored
    // org-tagged schemas until the node is restarted. Reload unconditionally
    // here — the operation is a cheap no-op when cache matches disk. Papercut
    // 99e8a from Alpha E2E Dogfood Run 5, Flow 2.
    db.schema_manager()
        .reload_from_store()
        .await
        .handler_err("reload schema cache after restore")?;

    let last_seqs: Vec<u64> = outcomes.iter().map(|o| o.last_seq).collect();
    let seq_summary = match last_seqs.as_slice() {
        [seq] => format!("seq={seq}"),
        seqs => format!("seqs={seqs:?}"),
    };

    Ok(ApiResponse::success_with_user(
        SnapshotRestoreResponse {
            success: true,
            targets_restored,
            entries_replayed,
            message: format!(
                "Restored snapshot for {targets_restored} target(s) ({seq_summary}); \
                 {entries_replayed} additional log entries applied on top"
            ),
        },
        user_hash,
    ))
}
