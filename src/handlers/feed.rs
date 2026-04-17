//! Social Feed Handlers
//!
//! Framework-agnostic handlers for the social feed endpoint.
//! Queries a schema for records authored by friends, filters by
//! field-level access policies, and returns results sorted by timestamp.

use crate::fold_node::node::FoldNode;
use crate::fold_node::OperationProcessor;
use crate::handlers::handler_response;
use crate::handlers::response::{ApiResponse, HandlerResult, IntoTypedHandlerError};
use fold_db::fold_db_core::query::records_from_field_map;
use fold_db::schema::types::operations::{Query, SortOrder};
use serde::Deserialize;
use std::collections::HashSet;

/// Request for the social feed endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct FeedRequest {
    /// Schema to query (optional — if omitted, queries all schemas)
    #[serde(default)]
    pub schema_name: Option<String>,
    /// Public keys of friends whose data to include
    pub friend_hashes: Vec<String>,
    /// Maximum number of results (default 50)
    pub limit: Option<usize>,
}

handler_response! {
    /// Response for the social feed endpoint
    pub struct FeedResponse {
        /// Feed items sorted by timestamp descending
        pub items: Vec<serde_json::Value>,
        /// Total number of matching items (before limit)
        pub total: usize,
    }
}

/// Default feed limit
const DEFAULT_FEED_LIMIT: usize = 50;

/// Get the social feed for a user.
///
/// When `schema_name` is provided, queries only that schema. When omitted,
/// queries all registered schemas. Filters to records authored by friends
/// with publicly-readable fields, sorted by timestamp descending.
pub async fn get_feed(
    request: FeedRequest,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<FeedResponse> {
    let processor = OperationProcessor::new(std::sync::Arc::new(node.clone()));
    let limit = request.limit.unwrap_or(DEFAULT_FEED_LIMIT);
    let friends: HashSet<&str> = request.friend_hashes.iter().map(|s| s.as_str()).collect();

    if friends.is_empty() {
        return Ok(ApiResponse::success_with_user(
            FeedResponse {
                items: vec![],
                total: 0,
            },
            user_hash,
        ));
    }

    // Determine which schemas to query
    let schema_names: Vec<String> = match &request.schema_name {
        Some(name) if !name.is_empty() => vec![name.clone()],
        _ => {
            // Query all registered schemas
            let schemas = processor.list_schemas().await.typed_handler_err()?;
            schemas.into_iter().map(|s| s.schema.name).collect()
        }
    };

    let mut all_items = Vec::new();

    for schema_name in &schema_names {
        // Get schema to inspect field access policies
        let (all_fields, public_fields) = {
            let db = processor.get_db_public().typed_handler_err()?;
            let schema = match db.schema_manager().get_schema(schema_name).await {
                Ok(Some(s)) => s,
                // Skip schemas that don't exist or fail to load
                _ => continue,
            };

            let all_fields: Vec<String> = schema.fields.clone().unwrap_or_default();
            let public_fields: HashSet<String> = schema
                .runtime_fields
                .iter()
                .filter(|(_, field)| {
                    use fold_db::schema::types::field::Field;
                    match &field.common().access_policy {
                        None => false, // No policy = owner-only, not public
                        Some(policy) => policy.min_read_tier == fold_db::access::AccessTier::Public,
                    }
                })
                .map(|(name, _)| name.clone())
                .collect();

            (all_fields, public_fields)
        };

        if public_fields.is_empty() {
            continue;
        }

        let query = Query {
            schema_name: schema_name.clone(),
            fields: all_fields,
            filter: None,
            as_of: None,
            rehydrate_depth: None,
            sort_order: Some(SortOrder::Desc),
            value_filters: None,
        };

        // SAFETY: feed handler is owner-only by construction.
        //
        // - Route: POST /api/feed (server/routes/feed.rs) runs against the local
        //   node's own Sled via `node_or_return!(state)`; there is no
        //   caller-identity input in `FeedRequest` (only `friend_hashes`, which
        //   name the *authors* whose records to surface, not a querying caller).
        // - The caller is always the owner of this node. If we routed through
        //   `execute_query_json_with_access(query, owner_pub_key)`, the access
        //   layer's owner short-circuit (see `OperationProcessor::build_access_context`)
        //   would produce `AccessContext::owner` and apply no field filtering —
        //   equivalent to `execute_query_map` here.
        // - Access enforcement in this handler is performed explicitly above:
        //   only fields whose `min_read_tier == AccessTier::Public` are retained
        //   in `public_fields`, and records are filtered by friend authorship
        //   via `writer_pubkey`. This is strictly tighter than the trust-tier
        //   check would apply for the owner.
        //
        // See `handlers::query` / `handlers::discovery` for the
        // caller-identity-aware path using `execute_query_json_with_access`.
        let owner_ctx = processor.owner_access_context();
        let result_map = match processor.execute_query_map(query, &owner_ctx).await {
            Ok(m) => m,
            // Skip schemas that fail to query
            Err(_) => continue,
        };

        let records = records_from_field_map(&result_map);

        for (key, record) in records {
            let author = record
                .metadata
                .values()
                .find_map(|meta| meta.writer_pubkey.as_deref());

            let author = match author {
                Some(a) if friends.contains(a) => a.to_string(),
                _ => continue,
            };

            let mut filtered_fields = serde_json::Map::new();
            for (field_name, value) in &record.fields {
                if public_fields.contains(field_name) {
                    filtered_fields.insert(field_name.clone(), value.clone());
                }
            }

            let timestamp = key.range.clone().unwrap_or_default();

            all_items.push(serde_json::json!({
                "key": key,
                "fields": filtered_fields,
                "author": author,
                "timestamp": timestamp,
                "schema_name": schema_name,
            }));
        }
    }

    // Sort all items by timestamp descending
    all_items.sort_by(|a, b| {
        let ts_a = a["timestamp"].as_str().unwrap_or("");
        let ts_b = b["timestamp"].as_str().unwrap_or("");
        ts_b.cmp(ts_a)
    });

    let total = all_items.len();
    all_items.truncate(limit);

    Ok(ApiResponse::success_with_user(
        FeedResponse {
            items: all_items,
            total,
        },
        user_hash,
    ))
}
