//! Actix route handlers for Persona list + detail + update. Thin
//! wrappers over the framework-agnostic handlers in
//! `crate::handlers::fingerprints`.

use crate::handlers::fingerprints as fp_handlers;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_error_to_response, node_or_return};
use actix_web::{web, HttpResponse, Responder};
use serde::Deserialize;

/// Body payload for `PATCH /api/fingerprints/personas/{id}`.
///
/// Every field is optional — callers populate only the ops they
/// need. Multiple ops may coexist in a single request and are
/// applied together within one read-modify-write cycle (see
/// `handlers::fingerprints::personas::apply_persona_patch`).
///
/// The exclusion ops are idempotent: adding an id that's already
/// excluded is a no-op, and the same for remove — the request
/// still returns the freshly-resolved detail so the caller can
/// round-trip "refresh and re-render" cheaply.
#[derive(Debug, Deserialize, Default)]
pub struct UpdatePersonaRequest {
    pub threshold: Option<f32>,
    pub add_excluded_edge_id: Option<String>,
    pub remove_excluded_edge_id: Option<String>,
    pub add_excluded_mention_id: Option<String>,
    pub remove_excluded_mention_id: Option<String>,
    pub name: Option<String>,
    pub relationship: Option<String>,
    pub aliases: Option<Vec<String>>,
    pub user_confirmed: Option<bool>,
}

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

/// PATCH /api/fingerprints/personas/{id} — update mutable fields on
/// an existing Persona. Returns the freshly-resolved detail on success.
///
/// Currently accepts only `{ "threshold": f32 }`. The handler uses
/// a read-modify-write flow against the Persona record so the
/// update is atomic from the caller's point of view.
pub async fn update_persona(
    path: web::Path<String>,
    body: web::Json<UpdatePersonaRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);
    let persona_id = path.into_inner();
    let body = body.into_inner();

    let patch = fp_handlers::PersonaPatch {
        threshold: body.threshold,
        add_excluded_edge_id: body.add_excluded_edge_id,
        remove_excluded_edge_id: body.remove_excluded_edge_id,
        add_excluded_mention_id: body.add_excluded_mention_id,
        remove_excluded_mention_id: body.remove_excluded_mention_id,
        name: body.name,
        relationship: body.relationship,
        aliases: body.aliases,
        user_confirmed: body.user_confirmed,
    };

    match fp_handlers::apply_persona_patch(node, persona_id, patch).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}
