//! Photo moment detection endpoints.

use super::get_discovery_config;
use crate::handlers::discovery as discovery_handlers;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_error_to_response, node_or_return};
use actix_web::{web, HttpResponse, Responder};

/// GET /api/discovery/moments/opt-ins — List all moment sharing opt-ins.
pub async fn moment_opt_in_list(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::moment_opt_in_list(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/moments/opt-in — Opt-in to photo moment sharing with a peer.
pub async fn moment_opt_in(
    body: web::Json<discovery_handlers::MomentOptInRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::moment_opt_in(&body, &node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/moments/opt-out — Opt-out of photo moment sharing with a peer.
pub async fn moment_opt_out(
    body: web::Json<discovery_handlers::MomentOptOutRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::moment_opt_out(&body, &node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/moments/scan — Scan local photos and generate moment hashes.
pub async fn moment_scan(
    body: web::Json<Vec<discovery_handlers::PhotoMetadata>>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    let (_url, key) = match get_discovery_config(&state).await {
        Ok(c) => c,
        Err(response) => return response,
    };

    match discovery_handlers::moment_scan(&node, &key, &body).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/moments/receive — Receive moment hashes from a peer.
pub async fn moment_receive_hashes(
    body: web::Json<discovery_handlers::MomentHashReceiveRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::moment_receive_hashes(&body, &node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/moments/detect — Detect shared moments from exchanged hashes.
pub async fn moment_detect(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::moment_detect(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/discovery/moments — List all detected shared moments.
pub async fn moment_list(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::moment_list(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}
