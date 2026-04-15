//! Actix routes for the Failed records panel. Thin wrappers over
//! `crate::handlers::fingerprints::ingestion_errors`.

use crate::handlers::fingerprints as fp_handlers;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_error_to_response, node_or_return};
use actix_web::{web, HttpResponse, Responder};
use serde::Deserialize;

/// Query string for `GET /api/fingerprints/ingestion-errors`.
#[derive(Debug, Deserialize, Default)]
pub struct ListIngestionErrorsQuery {
    /// When true, resolved rows are included alongside open failures.
    /// Defaults to false because the panel is primarily a to-do list
    /// of unresolved failures.
    #[serde(default)]
    pub include_resolved: bool,
}

/// GET /api/fingerprints/ingestion-errors — list every IngestionError
/// record. Accepts `?include_resolved=true` to pull archived rows too.
pub async fn list_ingestion_errors(
    query: web::Query<ListIngestionErrorsQuery>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);
    let include_resolved = query.include_resolved;

    match fp_handlers::list_ingestion_errors(node, include_resolved).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// Body payload for `PATCH /api/fingerprints/ingestion-errors/{id}`.
#[derive(Debug, Deserialize)]
pub struct UpdateIngestionErrorBody {
    /// `true` to dismiss (default when omitted), `false` to restore
    /// a previously-dismissed row back into the active Failed panel.
    #[serde(default = "default_resolved")]
    pub resolved: bool,
}

fn default_resolved() -> bool {
    true
}

/// PATCH /api/fingerprints/ingestion-errors/{id} — set the resolved
/// flag. Pass `{ "resolved": false }` to un-dismiss a previously
/// dismissed row.
pub async fn resolve_ingestion_error(
    path: web::Path<String>,
    body: web::Json<UpdateIngestionErrorBody>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);
    let error_id = path.into_inner();

    match fp_handlers::resolve_ingestion_error(node, error_id, body.resolved).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}
