//! Discovery opt-in / opt-out configuration endpoints.

use super::{auth_token_or_return, discovery_config_or_return};
use crate::handlers::discovery as discovery_handlers;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_error_to_response, node_or_return};
use actix_web::{web, HttpRequest, HttpResponse, Responder};

/// GET /api/discovery/opt-ins — List all discovery opt-in configs.
pub async fn list_opt_ins(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::list_opt_ins(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/opt-in — Opt-in a schema for discovery.
pub async fn opt_in(
    body: web::Json<discovery_handlers::OptInRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::opt_in(&body, &node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/discovery/my-pseudonyms — List all pseudonyms this node publishes.
/// Used by the E2E test framework for cleanup.
pub async fn my_pseudonyms(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    let (_url, key) = discovery_config_or_return!(state);

    match discovery_handlers::my_pseudonyms(&node, &key).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/opt-out-all — Clear all discovery opt-ins (test cleanup).
pub async fn opt_out_all(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::opt_out_all(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/opt-out — Opt-out a schema from discovery.
pub async fn opt_out(
    req: HttpRequest,
    body: web::Json<discovery_handlers::OptOutRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    let (url, key) = discovery_config_or_return!(state);

    let auth_token = auth_token_or_return!(req);

    match discovery_handlers::opt_out(&body, &node, &url, &auth_token, &key).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}
