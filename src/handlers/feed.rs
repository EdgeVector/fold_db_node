//! Social Feed Handlers
//!
//! Framework-agnostic handlers for the social feed endpoint.
//! Queries a schema for records authored by friends, filters by
//! field-level access policies, and returns results sorted by timestamp.

use crate::fold_node::node::FoldNode;
use crate::fold_node::OperationProcessor;
use crate::handlers::handler_response;
use crate::handlers::response::{ApiResponse, HandlerResult, IntoHandlerError};
use fold_db::fold_db_core::query::records_from_field_map;
use fold_db::schema::types::operations::{Query, SortOrder};
use serde::Deserialize;
use std::collections::HashSet;

/// Request for the social feed endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct FeedRequest {
    /// Schema to query for photo data (e.g. "Photo")
    pub schema_name: String,
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

/// Get the social photo feed for a user.
///
/// Queries the specified schema for records authored by users in `friend_hashes`,
/// filters to only publicly-readable fields, and returns results sorted by
/// timestamp (range key) descending.
pub async fn get_feed(
    request: FeedRequest,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<FeedResponse> {
    let processor = OperationProcessor::new(node.clone());
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

    // Get schema to inspect field access policies
    let (all_fields, public_fields) = {
        let db = processor
            .get_db_public()
            .await
            .handler_err("acquire database lock")?;
        let schema = db
            .schema_manager
            .get_schema(&request.schema_name)
            .await
            .handler_err("get schema")?
            .ok_or_else(|| {
                crate::handlers::response::HandlerError::NotFound(format!(
                    "Schema '{}' not found",
                    request.schema_name
                ))
            })?;

        let all_fields: Vec<String> = schema.fields.clone().unwrap_or_default();
        let public_fields: HashSet<String> = schema
            .runtime_fields
            .iter()
            .filter(|(_, field)| {
                use fold_db::schema::types::field::Field;
                match &field.common().access_policy {
                    // No policy = legacy behavior, allow read
                    None => true,
                    // Has policy: check if publicly readable (any trust distance)
                    Some(policy) => policy.trust_distance.can_read(u64::MAX),
                }
            })
            .map(|(name, _)| name.clone())
            .collect();

        (all_fields, public_fields)
    };

    if public_fields.is_empty() {
        return Ok(ApiResponse::success_with_user(
            FeedResponse {
                items: vec![],
                total: 0,
            },
            user_hash,
        ));
    }

    // Query all fields (we need metadata from all for author filtering),
    // sort descending by range key (timestamp)
    let query = Query {
        schema_name: request.schema_name.clone(),
        fields: all_fields,
        filter: None,
        as_of: None,
        rehydrate_depth: None,
        sort_order: Some(SortOrder::Desc),
    };

    let result_map = processor
        .execute_query_map(query)
        .await
        .handler_err("execute feed query")?;

    let records = records_from_field_map(&result_map);

    // Sort records by range key descending
    let mut sorted_records: Vec<_> = records.into_iter().collect();
    sorted_records.sort_by(|(key_a, _), (key_b, _)| {
        let range_a = key_a.range.as_deref().unwrap_or("");
        let range_b = key_b.range.as_deref().unwrap_or("");
        range_b.cmp(range_a) // descending
    });

    // Filter by author and format response
    let mut items = Vec::new();
    for (key, record) in &sorted_records {
        // Determine the author from any field's source_pub_key in metadata
        let author = record
            .metadata
            .values()
            .find_map(|meta| meta.source_pub_key.as_deref());

        let author = match author {
            Some(a) if friends.contains(a) => a.to_string(),
            // Skip records not authored by a friend (or with unknown author)
            _ => continue,
        };

        // Build fields map with only public fields
        let mut filtered_fields = serde_json::Map::new();
        for (field_name, value) in &record.fields {
            if public_fields.contains(field_name) {
                filtered_fields.insert(field_name.clone(), value.clone());
            }
        }

        let timestamp = key.range.clone().unwrap_or_default();

        items.push(serde_json::json!({
            "key": key,
            "fields": filtered_fields,
            "author": author,
            "timestamp": timestamp,
        }));
    }

    let total = items.len();
    items.truncate(limit);

    Ok(ApiResponse::success_with_user(
        FeedResponse { items, total },
        user_hash,
    ))
}
