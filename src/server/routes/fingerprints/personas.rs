//! Actix route handlers for Persona list + detail. Thin wrappers
//! over the framework-agnostic handlers in
//! `crate::handlers::fingerprints`.

use crate::handlers::fingerprints as fp_handlers;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_error_to_response, node_or_return};
use actix_web::{web, HttpResponse, Responder};

/// GET /api/fingerprints/personas — list every Persona with
/// resolved-cluster summary counts.
pub async fn list_personas(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    match fp_handlers::list_personas(node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/fingerprints/personas/{id} — return a single Persona's
/// full resolved cluster plus diagnostics.
pub async fn get_persona(path: web::Path<String>, state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    let persona_id = path.into_inner();

    match fp_handlers::get_persona(node, persona_id).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}
