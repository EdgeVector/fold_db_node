//! Discovery opt-in / opt-out configuration endpoints.

use super::{get_auth_token, get_discovery_config};
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

    let (_url, key) = match get_discovery_config(&state).await {
        Ok(c) => c,
        Err(response) => return response,
    };

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

    let (url, key) = match get_discovery_config(&state).await {
        Ok(c) => c,
        Err(response) => return response,
    };

    let auth_token = match get_auth_token(&req) {
        Ok(t) => t,
        Err(response) => return response,
    };

    match discovery_handlers::opt_out(&body, &node, &url, &auth_token, &key).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}
