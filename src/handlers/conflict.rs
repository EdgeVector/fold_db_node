//! Shared Conflict Handlers
//!
//! Framework-agnostic handlers for org sync conflict resolution.

use crate::fold_node::node::FoldNode;
use crate::handlers::handler_response;
use crate::handlers::response::{ApiResponse, HandlerResult, IntoHandlerError};
use fold_db::sync::conflict::ConflictRecord;
use serde::Deserialize;

handler_response! {
    pub struct ListConflictsResponse {
        pub conflicts: Vec<ConflictRecord>,
    }
}

handler_response! {
    pub struct GetConflictResponse {
        pub conflict: ConflictRecord,
    }
}

handler_response! {
    pub struct ResolveConflictResponse {
        pub conflict: ConflictRecord,
    }
}

#[derive(Debug, Deserialize)]
pub struct ListConflictsQuery {
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

fn default_limit() -> usize {
    50
}

/// List conflict records for an org.
pub async fn list_conflicts(
    org_hash: &str,
    query: &ListConflictsQuery,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<ListConflictsResponse> {
    let db = node.get_fold_db().await.handler_err("lock database")?;
    let sync_engine = db.sync_engine().ok_or_else(|| {
        crate::handlers::HandlerError::BadRequest("Sync engine not configured".to_string())
    })?;

    let conflicts = sync_engine
        .list_conflicts(Some(org_hash), query.limit, query.offset)
        .await
        .handler_err("list conflicts")?;

    Ok(ApiResponse::success_with_user(
        ListConflictsResponse { conflicts },
        user_hash,
    ))
}

/// Get a single conflict record by ID.
pub async fn get_conflict(
    _org_hash: &str,
    conflict_id: &str,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<GetConflictResponse> {
    let db = node.get_fold_db().await.handler_err("lock database")?;
    let sync_engine = db.sync_engine().ok_or_else(|| {
        crate::handlers::HandlerError::BadRequest("Sync engine not configured".to_string())
    })?;

    let conflict = sync_engine
        .get_conflict(conflict_id)
        .await
        .handler_err("get conflict")?
        .ok_or_else(|| {
            crate::handlers::HandlerError::NotFound(format!("Conflict {conflict_id} not found"))
        })?;

    Ok(ApiResponse::success_with_user(
        GetConflictResponse { conflict },
        user_hash,
    ))
}

/// Manually resolve a conflict by applying the loser's value.
pub async fn resolve_conflict(
    _org_hash: &str,
    conflict_id: &str,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<ResolveConflictResponse> {
    let db = node.get_fold_db().await.handler_err("lock database")?;
    let sync_engine = db.sync_engine().ok_or_else(|| {
        crate::handlers::HandlerError::BadRequest("Sync engine not configured".to_string())
    })?;

    let conflict = sync_engine
        .resolve_conflict(conflict_id)
        .await
        .handler_err("resolve conflict")?;

    Ok(ApiResponse::success_with_user(
        ResolveConflictResponse { conflict },
        user_hash,
    ))
}
