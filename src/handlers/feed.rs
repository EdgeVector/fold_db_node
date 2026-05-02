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
use fold_db::schema::types::field::{Field, FieldVariant};
use fold_db::schema::types::key_value::KeyValue;
use fold_db::schema::types::operations::{Query, SortOrder};
use fold_db::schema::types::schema::Schema;
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
    let processor = OperationProcessor::from_ref(node);
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
        // Get schema (refreshed) so we can both inspect access policies and
        // resolve per-key writer_pubkey from molecule AtomEntry. For Hash,
        // Range, and HashRange fields, `FieldValue.writer_pubkey` is not
        // populated by `resolve_value` — it only works for Single molecules.
        // So author attribution must be read directly from the per-entry
        // AtomEntry.writer_pubkey on each field's molecule.
        let (all_fields, public_fields, schema) = {
            let db = processor.get_db_public().typed_handler_err()?;
            let mut schema = match db.schema_manager().get_schema(schema_name).await {
                Ok(Some(s)) => s,
                // Skip schemas that don't exist or fail to load
                _ => continue,
            };

            // Refresh each field's molecule from DB so we can read per-entry
            // writer_pubkey below.
            for field in schema.runtime_fields.values_mut() {
                field.refresh_from_db(db.db_ops()).await;
            }

            let all_fields: Vec<String> = schema.fields.clone().unwrap_or_default();
            let public_fields: HashSet<String> = schema
                .runtime_fields
                .iter()
                .filter(|(_, field)| match &field.common().access_policy {
                    None => false, // No policy = owner-only, not public
                    Some(policy) => policy.min_read_tier == fold_db::access::AccessTier::Public,
                })
                .map(|(name, _)| name.clone())
                .collect();

            (all_fields, public_fields, schema)
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
            // Resolve writer_pubkey per-key from the molecule's AtomEntry.
            // This handles Hash/Range/HashRange fields where per-entry
            // writer_pubkey is not plumbed through FieldValue.
            let author_owned = find_writer_pubkey(&schema, &key);
            let author = match author_owned.as_deref() {
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

/// Look up the writer_pubkey for a record key by inspecting the per-entry
/// AtomEntry on the schema's field molecules. Returns the first non-empty
/// writer_pubkey found across fields (they should all agree for a given key
/// when the record was written by a single mutation).
fn find_writer_pubkey(schema: &Schema, key: &KeyValue) -> Option<String> {
    for field in schema.runtime_fields.values() {
        let pk = match field {
            FieldVariant::Single(f) => f.molecule
                .as_ref()
                .map(|m| m.writer_pubkey().to_string()),
            FieldVariant::Hash(f) => key.hash.as_ref().and_then(|h| {
                f.molecule
                    .as_ref()
                    .and_then(|m| m.get_atom_entry(h).map(|e| e.writer_pubkey.clone()))
            }),
            FieldVariant::Range(f) => key.range.as_ref().and_then(|r| {
                f.molecule
                    .as_ref()
                    .and_then(|m| m.get_atom_entry(r).map(|e| e.writer_pubkey.clone()))
            }),
            FieldVariant::HashRange(f) => {
                key.hash
                    .as_ref()
                    .zip(key.range.as_ref())
                    .and_then(|(h, r)| {
                        f.molecule
                            .as_ref()
                            .and_then(|m| m.get_atom_entry(h, r).map(|e| e.writer_pubkey.clone()))
                    })
            }
        };
        if let Some(pk) = pk {
            if !pk.is_empty() {
                return Some(pk);
            }
        }
    }
    None
}
