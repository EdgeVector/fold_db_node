//! HTTP routes for org sync conflict resolution.

use crate::handlers::conflict as conflict_handlers;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_result_to_response, node_or_return};
use actix_web::{web, Responder};

/// GET /api/org/{org_hash}/conflicts?limit=50&offset=0
pub async fn list_conflicts(
    path: web::Path<String>,
    query: web::Query<conflict_handlers::ListConflictsQuery>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let org_hash = path.into_inner();
    handler_result_to_response(
        conflict_handlers::list_conflicts(&org_hash, &query, &user_hash, &node).await,
    )
}

/// GET /api/org/{org_hash}/conflicts/{conflict_id}
pub async fn get_conflict(
    path: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let (org_hash, conflict_id) = path.into_inner();
    handler_result_to_response(
        conflict_handlers::get_conflict(&org_hash, &conflict_id, &user_hash, &node).await,
    )
}

/// POST /api/org/{org_hash}/conflicts/{conflict_id}/resolve
pub async fn resolve_conflict(
    path: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let (org_hash, conflict_id) = path.into_inner();
    handler_result_to_response(
        conflict_handlers::resolve_conflict(&org_hash, &conflict_id, &user_hash, &node).await,
    )
}
