use crate::handlers::query as query_handlers;
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use fold_db::schema::types::operations::{Operation, Query};
use crate::server::http_server::AppState;
use crate::server::routes::{handler_error_to_response, handler_result_to_response, node_or_return};
use actix_web::{web, HttpResponse, Responder};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct MutationResponse {
    pub mutation_id: String,
}

/// Execute a query.
#[utoipa::path(
    post,
    path = "/api/query",
    tag = "query",
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Array of query result records"),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Server error")
    )
)]
pub async fn execute_query(query: web::Json<Query>, state: web::Data<AppState>) -> impl Responder {
    let query_inner = query.into_inner();
    log_feature!(
        LogFeature::HttpServer,
        info,
        "execute_query: schema={}, fields={:?}, filter={:?}",
        query_inner.schema_name,
        query_inner.fields,
        query_inner.filter
    );

    let (user_hash, node) = node_or_return!(state);

    match query_handlers::execute_query(query_inner, &user_hash, &node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => {
            log_feature!(LogFeature::HttpServer, error, "Query failed: {}", e);
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
                log_feature!(
                    LogFeature::HttpServer,
                    info,
                    "Parsed mutation: schema={}, type={:?}, fields={}",
                    schema,
                    mutation_type,
                    fields_and_values.len()
                );
                (schema, fields_and_values, key_value, mutation_type)
            }
            Err(e) => {
                log_feature!(LogFeature::HttpServer, error, "Failed to parse mutation: {}", e);
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
            log_feature!(LogFeature::HttpServer, error, "Mutation failed: {}", e);
            handler_error_to_response(e)
        }
    }
}

/// Execute multiple mutations in a batch for improved performance.
#[utoipa::path(
    post,
    path = "/api/mutations/batch",
    tag = "query",
    request_body = Vec<serde_json::Value>,
    responses(
        (status = 200, description = "Array of mutation IDs"),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Server error")
    )
)]
pub async fn execute_mutations_batch(
    mutations_data: web::Json<Vec<Value>>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);

    handler_result_to_response(
        crate::handlers::mutation::execute_mutations_batch_from_json(
            mutations_data.into_inner(),
            &user_hash,
            &node,
        )
        .await,
    )
}

/// Search the native word index for a term.
#[utoipa::path(
    get,
    path = "/api/native-index/search",
    tag = "query",
    params(
        ("term" = String, Query, description = "Search term for native word index")
    ),
    responses(
        (status = 200, description = "Array of native index results", body = [fold_db::db_operations::IndexResult]),
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
            log_feature!(LogFeature::HttpServer, warn, "native_index_search: missing or empty term");
            return HttpResponse::BadRequest()
                .json(json!({"error": "Missing required 'term' query parameter"}));
        }
    };

    let (user_hash, node) = node_or_return!(state);

    log_feature!(
        LogFeature::HttpServer,
        info,
        "native_index_search: term='{}', user='{}'",
        term,
        user_hash
    );

    match query_handlers::native_index_search(&term, &user_hash, &node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => {
            log_feature!(LogFeature::HttpServer, error, "native_index_search failed: {}", e);
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

    handler_result_to_response(query_handlers::get_molecule_history(&molecule_uuid, &user_hash, &node).await)
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

    handler_result_to_response(query_handlers::get_atom_content(&atom_uuid, &user_hash, &node).await)
}

/// Get process results for a progress_id (actual stored keys from ingestion mutations).
pub async fn get_process_results(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let progress_id = path.into_inner();
    let (user_hash, node) = node_or_return!(state);

    handler_result_to_response(query_handlers::get_process_results(&progress_id, &user_hash, &node).await)
}
