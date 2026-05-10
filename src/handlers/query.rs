//! Shared Query Handlers
//!
//! Framework-agnostic handlers for query operations.
//! These can be called by both HTTP server routes and Lambda handlers.

use crate::fold_node::node::FoldNode;
use crate::fold_node::OperationProcessor;
use crate::handlers::current_caller_pubkey;
use crate::handlers::handler_response;
use crate::handlers::response::{
    get_db_guard, ApiResponse, HandlerError, HandlerResult, IntoHandlerError, IntoTypedHandlerError,
};
use fold_db::schema::types::field::HashRangeFilter;
use fold_db::schema::types::operations::Query;
use serde::{Deserialize, Serialize};

/// Default `limit` applied when the request body omits one. Matches the
/// historical implicit cap of 100 records for unfiltered queries, so existing
/// callers see no behaviour change beyond gaining `total_count`/`has_more`.
pub const DEFAULT_QUERY_LIMIT: usize = 100;

/// Hard cap on a single page. Prevents a hostile client from asking for an
/// unbounded response.
pub const MAX_QUERY_LIMIT: usize = 1000;

/// Internal fetch cap used to override fold_db's default `SampleN(100)` when
/// the caller passes no filter. Lets us compute an accurate `total_count` for
/// typical user databases while bounding memory use. If a schema has more
/// records than this cap, `has_more` is `true` and `total_count` reports the
/// cap (a counts cache would be the next step — out of scope here).
pub const INTERNAL_FETCH_CAP: usize = 10_000;

handler_response! {
    /// Response for query execution. Includes pagination metadata so callers
    /// can detect truncation: `total_count` is the size of the post-filter
    /// result set fold_db_node observed, `returned_count` is `results.len()`
    /// after applying the caller's `offset`/`limit`, and `has_more` is true
    /// when more records exist beyond the returned page.
    pub struct QueryResponse {
        /// Query results (page).
        pub results: serde_json::Value,
        /// Total records matching the query before `offset`/`limit` are
        /// applied. Capped at `INTERNAL_FETCH_CAP` for unfiltered queries —
        /// when `has_more` is true and `total_count` equals that cap, there
        /// may be additional records that fold_db_node did not load.
        pub total_count: usize,
        /// Number of records actually returned in `results`.
        pub returned_count: usize,
        /// Page size applied (defaults to `DEFAULT_QUERY_LIMIT`).
        pub limit: usize,
        /// Page offset applied (defaults to 0).
        pub offset: usize,
        /// True when more records exist beyond the returned page.
        pub has_more: bool,
    }
}

handler_response! {
    /// Response for native index search
    pub struct IndexSearchResponse {
        /// Search results
        pub results: serde_json::Value,
    }
}

/// Execute a query with access control.
///
/// The caller's public key resolves trust distances across domains; fields
/// the caller lacks access to are filtered from results.
///
/// `query.schema_name` may be either the canonical hash or the schema's
/// `descriptive_name` (e.g. "Apple Reminders"). See
/// [`crate::handlers::schema_resolution`].
///
/// Pagination: `limit` (clamped to `MAX_QUERY_LIMIT`, default
/// `DEFAULT_QUERY_LIMIT`) and `offset` are applied here, after fold_db
/// returns. When the caller passes no `filter`, fold_db's default would
/// silently truncate at 100 records (`HashRangeFilter::SampleN(100)` in
/// `apply_*_filter`); we override that with `SampleN(INTERNAL_FETCH_CAP)`
/// so `total_count` is accurate up to the cap.
pub async fn execute_query(
    mut query: Query,
    limit: Option<usize>,
    offset: Option<usize>,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<QueryResponse> {
    let processor = OperationProcessor::from_ref(node);
    let caller_pub_key = current_caller_pubkey(node);

    query.schema_name =
        crate::handlers::schema_resolution::resolve_schema_name(&processor, &query.schema_name)
            .await?;

    let limit = limit.unwrap_or(DEFAULT_QUERY_LIMIT).min(MAX_QUERY_LIMIT);
    let offset = offset.unwrap_or(0);

    if query.filter.is_none() {
        query.filter = Some(HashRangeFilter::SampleN(INTERNAL_FETCH_CAP));
    }

    let all_results = processor
        .execute_query_json_with_access(query, &caller_pub_key)
        .await
        .typed_handler_err()?;
    let total_count = all_results.len();
    let page: Vec<serde_json::Value> = all_results.into_iter().skip(offset).take(limit).collect();
    let returned_count = page.len();
    let has_more = offset.saturating_add(returned_count) < total_count;

    Ok(ApiResponse::success_with_user(
        QueryResponse {
            results: serde_json::Value::Array(page),
            total_count,
            returned_count,
            limit,
            offset,
            has_more,
        },
        user_hash,
    ))
}

/// Execute a native index search.
///
/// `include_internal` toggles inclusion of internal/bookkeeping schemas
/// (`Mention`, `MentionBySource`, `ExtractionStatus`, `IngestionError`,
/// `TriggerFiring`, `ai_conversations`, `ExtractionRule`). The HTTP route
/// defaults this to `false`; pass `true` only for admin/debug callers.
pub async fn native_index_search(
    query_string: &str,
    include_internal: bool,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<IndexSearchResponse> {
    let processor = OperationProcessor::from_ref(node);

    let results = processor
        .native_index_search(query_string, include_internal)
        .await
        .typed_handler_err()?;
    let results_json = serde_json::to_value(&results).handler_err("serialize search results")?;
    Ok(ApiResponse::success_with_user(
        IndexSearchResponse {
            results: results_json,
        },
        user_hash,
    ))
}

/// Summary of a single mutation event in a molecule's history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationEventSummary {
    pub timestamp: String,
    pub version: u64,
    pub field_key: serde_json::Value,
    pub old_atom_uuid: Option<String>,
    pub new_atom_uuid: String,
}

/// Response for molecule history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoleculeHistoryResponse {
    pub molecule_uuid: String,
    pub events: Vec<MutationEventSummary>,
}

/// Response for atom content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtomContentResponse {
    pub atom_uuid: String,
    pub content: serde_json::Value,
    pub source_file_name: Option<String>,
    pub created_at: String,
}

/// Get mutation history for a molecule
pub async fn get_molecule_history(
    molecule_uuid: &str,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<MoleculeHistoryResponse> {
    let db_guard = get_db_guard(node)?;

    let db_ops = db_guard.get_db_ops();

    let events = db_ops
        .get_mutation_events(molecule_uuid, None)
        .await
        .typed_handler_err()?;

    let summaries: Vec<MutationEventSummary> = events
        .into_iter()
        .map(|e| MutationEventSummary {
            timestamp: e.timestamp.to_rfc3339(),
            version: e.version,
            field_key: serde_json::to_value(&e.field_key).unwrap_or_default(),
            old_atom_uuid: e.old_atom_uuid,
            new_atom_uuid: e.new_atom_uuid,
        })
        .collect();

    Ok(ApiResponse::success_with_user(
        MoleculeHistoryResponse {
            molecule_uuid: molecule_uuid.to_string(),
            events: summaries,
        },
        user_hash,
    ))
}

/// Get content of a specific atom by UUID
pub async fn get_atom_content(
    atom_uuid: &str,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<AtomContentResponse> {
    let db_guard = get_db_guard(node)?;

    let db_ops = db_guard.get_db_ops();

    let atom: fold_db::atom::Atom = db_ops
        .get_atom_by_uuid(atom_uuid, None)
        .await
        .handler_err("fetch atom")?
        .ok_or_else(|| HandlerError::NotFound(format!("Atom '{}' not found", atom_uuid)))?;

    Ok(ApiResponse::success_with_user(
        AtomContentResponse {
            atom_uuid: atom_uuid.to_string(),
            content: atom.content().clone(),
            source_file_name: atom.source_file_name().cloned(),
            created_at: atom.created_at().to_rfc3339(),
        },
        user_hash,
    ))
}

/// Response for process results query
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessResultsResponse {
    pub results: Vec<crate::fold_node::node::MutationOutcome>,
}

/// Get process results for a given progress_id (actual stored keys from mutations)
pub async fn get_process_results(
    progress_id: &str,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<ProcessResultsResponse> {
    let results = node
        .get_process_results(progress_id)
        .await
        .typed_handler_err()?;

    Ok(ApiResponse::success_with_user(
        ProcessResultsResponse { results },
        user_hash,
    ))
}

/// Summary of a sync conflict for API responses.
#[cfg_attr(feature = "ts-bindings", derive(ts_rs::TS))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictSummary {
    pub id: String,
    pub molecule_uuid: String,
    pub conflict_key: String,
    pub winner_atom: String,
    pub loser_atom: String,
    pub detected_at: String,
}

handler_response! {
    /// Response for listing sync conflicts
    pub struct ConflictsResponse {
        pub conflicts: Vec<ConflictSummary>,
    }
}

/// List unresolved sync conflicts.
pub async fn get_conflicts(
    molecule_uuid: Option<&str>,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<ConflictsResponse> {
    let db_guard = get_db_guard(node)?;
    let db_ops = db_guard.get_db_ops();

    let conflicts = db_ops
        .get_unresolved_conflicts(molecule_uuid, None)
        .await
        .typed_handler_err()?;

    let summaries: Vec<ConflictSummary> = conflicts
        .into_iter()
        .map(|c| ConflictSummary {
            id: c.id,
            molecule_uuid: c.molecule_uuid,
            conflict_key: c.conflict_key,
            winner_atom: c.winner_atom,
            loser_atom: c.loser_atom,
            detected_at: c.detected_at.to_rfc3339(),
        })
        .collect();

    Ok(ApiResponse::success_with_user(
        ConflictsResponse {
            conflicts: summaries,
        },
        user_hash,
    ))
}

/// Resolve (acknowledge) a sync conflict by ID.
pub async fn resolve_conflict(
    conflict_id: &str,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<serde_json::Value> {
    let db_guard = get_db_guard(node)?;
    let db_ops = db_guard.get_db_ops();

    db_ops
        .resolve_conflict(conflict_id, None)
        .await
        .typed_handler_err()?;

    Ok(ApiResponse::success_with_user(
        serde_json::json!({"resolved": conflict_id}),
        user_hash,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_response_serialization() {
        let response = QueryResponse {
            results: serde_json::json!([]),
            total_count: 0,
            returned_count: 0,
            limit: DEFAULT_QUERY_LIMIT,
            offset: 0,
            has_more: false,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("results"));
        assert!(json.contains("total_count"));
        assert!(json.contains("returned_count"));
        assert!(json.contains("has_more"));
    }
}
