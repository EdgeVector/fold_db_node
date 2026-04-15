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
/// Only the `threshold` field is mutable today; future fields
/// (e.g. `name`, `relationship`) can be added alongside without
/// breaking existing clients because every field is `Option`.
#[derive(Debug, Deserialize)]
pub struct UpdatePersonaRequest {
    pub threshold: Option<f32>,
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

    let Some(threshold) = body.threshold else {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "error": "at least one mutable field must be supplied; threshold is required today"
        }));
    };

    match fp_handlers::update_persona_threshold(node, persona_id, threshold).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}
