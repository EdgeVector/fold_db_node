//! Actix routes for Suggested Personas. Thin wrappers over
//! `crate::handlers::fingerprints::suggestions`.

use crate::handlers::fingerprints as fp_handlers;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_error_to_response, node_or_return};
use actix_web::{web, HttpResponse, Responder};
use serde::Deserialize;

/// GET /api/fingerprints/suggestions — run the dense-subgraph sweep
/// and return every candidate cluster that passes the
/// MIN_FINGERPRINTS / MIN_MENTIONS gates.
pub async fn list_suggested_personas(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);
    match fp_handlers::list_suggested_personas(node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/fingerprints/suggestions/count — cheap read of the cached
/// suggestion count populated by the post-ingest auto-propose sweep.
/// The frontend polls this every 30s to drive the People-tab badge.
pub async fn get_suggestion_count(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);
    match fp_handlers::get_suggestion_count(node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// Body payload for `POST /api/fingerprints/suggestions/accept`.
#[derive(Debug, Deserialize)]
pub struct AcceptSuggestedBody {
    pub fingerprint_ids: Vec<String>,
    pub name: String,
    pub relationship: Option<String>,
}

/// POST /api/fingerprints/suggestions/accept — promote a suggested
/// cluster into a real Persona and return its resolved detail.
pub async fn accept_suggested_persona(
    body: web::Json<AcceptSuggestedBody>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);
    let body = body.into_inner();
    let req = fp_handlers::AcceptSuggestedRequest {
        fingerprint_ids: body.fingerprint_ids,
        name: body.name,
        relationship: body.relationship,
    };
    match fp_handlers::accept_suggested_persona(node, req).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}
