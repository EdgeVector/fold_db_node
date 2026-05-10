use crate::handlers::query as query_handlers;
// Imported so the bare `QueryResponse` token in the `#[utoipa::path]`
// annotation resolves to the same type registered via `components(schemas(...))`.
#[allow(unused_imports)]
use crate::handlers::query::QueryResponse;
use crate::server::http_server::AppState;
use crate::server::routes::{
    handler_error_to_response, handler_result_to_response, node_or_return,
};
use actix_web::{web, HttpResponse, Responder};
use fold_db::schema::types::operations::{Operation, Query};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct MutationResponse {
    pub mutation_id: String,
}

/// Execute a query.
///
/// Body is a [`Query`]-shaped JSON object with two optional pagination
/// siblings: `limit` (default 100, max 1000) and `offset` (default 0). They
/// are stripped from the body before deserialising into `Query`, which has
/// `#[serde(deny_unknown_fields)]` and no native pagination fields. The
/// response carries `total_count`, `returned_count`, `limit`, `offset`, and
/// `has_more` so callers can detect truncation.
#[utoipa::path(
    post,
    path = "/api/query",
    tag = "query",
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Page of query results plus pagination metadata", body = QueryResponse),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Server error")
    )
)]
pub async fn execute_query(body: web::Json<Value>, state: web::Data<AppState>) -> impl Responder {
    let mut body = body.into_inner();
    let (limit, offset) = match body.as_object_mut() {
        Some(obj) => (
            obj.remove("limit")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize),
            obj.remove("offset")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize),
        ),
        None => (None, None),
    };

    let query_inner: Query = match serde_json::from_value(body) {
        Ok(q) => q,
        Err(e) => {
            tracing::warn!(
                target: "fold_node::http_server",
                "execute_query: failed to parse query body: {}",
                e
            );
            return HttpResponse::BadRequest()
                .json(json!({"error": format!("Invalid query body: {}", e)}));
        }
    };

    tracing::info!(
            target: "fold_node::http_server",
        "execute_query: schema={}, fields={:?}, filter={:?}, limit={:?}, offset={:?}",
        query_inner.schema_name,
        query_inner.fields,
        query_inner.filter,
        limit,
        offset,
    );

    let (user_hash, node) = node_or_return!(state);

    match query_handlers::execute_query(query_inner, limit, offset, &user_hash, &node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => {
            tracing::error!(
            target: "fold_node::http_server", "Query failed: {}", e);
            handler_error_to_response(e)
        }
    }
}

/// Execute a mutation.
#[utoipa::path(
    post,
    path = "/api/mutation",
    tag = "query",
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Mutation accepted", body = MutationResponse),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Server error")
    )
)]
pub async fn execute_mutation(
    mutation_data: web::Json<Value>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (schema, fields_and_values, key_value, mutation_type) =
        match serde_json::from_value::<Operation>(mutation_data.into_inner()) {
            Ok(Operation::Mutation {
                schema,
                fields_and_values,
                key_value,
                mutation_type,
                source_file_name: _,
            }) => {
                tracing::info!(
                target: "fold_node::http_server",
                        "Parsed mutation: schema={}, type={:?}, fields={}",
                        schema,
                        mutation_type,
                        fields_and_values.len()
                    );
                (schema, fields_and_values, key_value, mutation_type)
            }
            Err(e) => {
                tracing::error!(
                target: "fold_node::http_server",
                        "Failed to parse mutation: {}",
                        e
                    );
                return HttpResponse::BadRequest()
                    .json(json!({"error": format!("Failed to parse mutation: {}", e)}));
            }
        };

    let (user_hash, node) = node_or_return!(state);

    match crate::handlers::mutation::execute_mutation_from_components(
        schema,
        fields_and_values,
        key_value,
        mutation_type,
        &user_hash,
        &node,
    )
    .await
    {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => {
            tracing::error!(
            target: "fold_node::http_server", "Mutation failed: {}", e);
            handler_error_to_response(e)
        }
    }
}

/// Search the native word index for a term.
#[utoipa::path(
    get,
    path = "/api/native-index/search",
    tag = "query",
    params(
        ("term" = String, Query, description = "Search term for native word index"),
        ("include_internal" = Option<bool>, Query, description = "Include internal/bookkeeping schemas (Mention, MentionBySource, ExtractionStatus, IngestionError, TriggerFiring, ai_conversations, ExtractionRule). Defaults to false."),
    ),
    responses(
        // body = [serde_json::Value]: utoipa emits the $ref using the
        // full Rust path (`fold_db.db_operations.IndexResult`) but
        // components(schemas(...)) registers by simple name
        // (`IndexResult`), so the reference can't resolve. Surface as
        // opaque array until upstream registration is reconciled (gbrain
        // projects/api-typegen-unification Phase 3).
        (status = 200, description = "Array of native index results", body = [serde_json::Value]),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Server error")
    )
)]
pub async fn native_index_search(
    query: web::Query<std::collections::HashMap<String, String>>,
    state: web::Data<AppState>,
) -> impl Responder {
    let term = match query.get("term") {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => {
            tracing::warn!(
            target: "fold_node::http_server",
                "native_index_search: missing or empty term"
            );
            return HttpResponse::BadRequest()
                .json(json!({"error": "Missing required 'term' query parameter"}));
        }
    };

    let include_internal = query
        .get("include_internal")
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false);

    let (user_hash, node) = node_or_return!(state);

    tracing::info!(
            target: "fold_node::http_server",
        "native_index_search: term='{}', user='{}', include_internal={}",
        term,
        user_hash,
        include_internal
    );

    match query_handlers::native_index_search(&term, include_internal, &user_hash, &node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => {
            tracing::error!(
            target: "fold_node::http_server",
                "native_index_search failed: {}",
                e
            );
            handler_error_to_response(e)
        }
    }
}

/// Get indexing status
#[utoipa::path(
    get,
    path = "/api/indexing/status",
    tag = "system",
    responses(
        (status = 200, description = "Current indexing status", body = IndexingStatus),
        (status = 500, description = "Server error")
    )
)]
pub async fn get_indexing_status(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);

    match crate::handlers::system::get_indexing_status(&user_hash, &node).await {
        Ok(response) => {
            HttpResponse::Ok().json(response.data.map(|d| d.status).unwrap_or(json!(null)))
        }
        Err(e) => handler_error_to_response(e),
    }
}

/// Get mutation history for a molecule.
#[utoipa::path(
    get,
    path = "/api/history/{molecule_uuid}",
    tag = "query",
    params(
        ("molecule_uuid" = String, Path, description = "Molecule UUID")
    ),
    responses(
        (status = 200, description = "Molecule mutation history"),
        (status = 500, description = "Server error")
    )
)]
pub async fn get_molecule_history(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let molecule_uuid = path.into_inner();
    let (user_hash, node) = node_or_return!(state);

    handler_result_to_response(
        query_handlers::get_molecule_history(&molecule_uuid, &user_hash, &node).await,
    )
}

/// Get atom content by UUID.
#[utoipa::path(
    get,
    path = "/api/atom/{atom_uuid}",
    tag = "query",
    params(
        ("atom_uuid" = String, Path, description = "Atom UUID")
    ),
    responses(
        (status = 200, description = "Atom content"),
        (status = 404, description = "Atom not found"),
        (status = 500, description = "Server error")
    )
)]
pub async fn get_atom_content(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let atom_uuid = path.into_inner();
    let (user_hash, node) = node_or_return!(state);

    handler_result_to_response(
        query_handlers::get_atom_content(&atom_uuid, &user_hash, &node).await,
    )
}

/// Get process results for a progress_id (actual stored keys from ingestion mutations).
pub async fn get_process_results(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let progress_id = path.into_inner();
    let (user_hash, node) = node_or_return!(state);

    handler_result_to_response(
        query_handlers::get_process_results(&progress_id, &user_hash, &node).await,
    )
}

/// Optional query parameter for filtering conflicts by molecule.
#[derive(Debug, Deserialize)]
pub struct ConflictQuery {
    pub molecule_uuid: Option<String>,
}

/// List unresolved sync conflicts.
#[utoipa::path(
    get,
    path = "/api/conflicts",
    tag = "query",
    params(
        ("molecule_uuid" = Option<String>, Query, description = "Filter by molecule UUID")
    ),
    responses(
        (status = 200, description = "List of unresolved sync conflicts"),
        (status = 500, description = "Server error")
    )
)]
pub async fn get_conflicts(
    query: web::Query<ConflictQuery>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);

    handler_result_to_response(
        query_handlers::get_conflicts(query.molecule_uuid.as_deref(), &user_hash, &node).await,
    )
}

/// Resolve (acknowledge) a sync conflict.
#[utoipa::path(
    post,
    path = "/api/conflicts/{conflict_id}/resolve",
    tag = "query",
    params(
        ("conflict_id" = String, Path, description = "Conflict ID to resolve")
    ),
    responses(
        (status = 200, description = "Conflict resolved"),
        (status = 404, description = "Conflict not found"),
        (status = 500, description = "Server error")
    )
)]
pub async fn resolve_conflict(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let conflict_id = path.into_inner();
    let (user_hash, node) = node_or_return!(state);

    handler_result_to_response(
        query_handlers::resolve_conflict(&conflict_id, &user_hash, &node).await,
    )
}
