//! Photo moment detection endpoints.

use super::discovery_config_or_return;
use crate::handlers::discovery as discovery_handlers;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_result_to_response, node_or_return};
use actix_web::{web, Responder};

/// GET /api/discovery/moments/opt-ins — List all moment sharing opt-ins.
pub async fn moment_opt_in_list(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    handler_result_to_response(discovery_handlers::moment_opt_in_list(&node).await)
}

/// POST /api/discovery/moments/opt-in — Opt-in to photo moment sharing with a peer.
pub async fn moment_opt_in(
    body: web::Json<discovery_handlers::MomentOptInRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    handler_result_to_response(discovery_handlers::moment_opt_in(&body, &node).await)
}

/// POST /api/discovery/moments/opt-out — Opt-out of photo moment sharing with a peer.
pub async fn moment_opt_out(
    body: web::Json<discovery_handlers::MomentOptOutRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    handler_result_to_response(discovery_handlers::moment_opt_out(&body, &node).await)
}

/// POST /api/discovery/moments/scan — Scan local photos and generate moment hashes.
pub async fn moment_scan(
    body: web::Json<Vec<discovery_handlers::PhotoMetadata>>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    let (_url, key) = discovery_config_or_return!(state);

    handler_result_to_response(discovery_handlers::moment_scan(&node, &key, &body).await)
}

/// POST /api/discovery/moments/receive — Receive moment hashes from a peer.
pub async fn moment_receive_hashes(
    body: web::Json<discovery_handlers::MomentHashReceiveRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    handler_result_to_response(discovery_handlers::moment_receive_hashes(&body, &node).await)
}

/// POST /api/discovery/moments/detect — Detect shared moments from exchanged hashes.
pub async fn moment_detect(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    handler_result_to_response(discovery_handlers::moment_detect(&node).await)
}

/// GET /api/discovery/moments — List all detected shared moments.
pub async fn moment_list(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    handler_result_to_response(discovery_handlers::moment_list(&node).await)
}
