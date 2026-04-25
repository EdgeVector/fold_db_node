//! Interest-category detection and toggling endpoints.

use crate::handlers::discovery as discovery_handlers;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_result_to_response, node_or_return};
use actix_web::{web, Responder};

/// GET /api/discovery/interests — Get detected interest categories.
pub async fn get_interests(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    handler_result_to_response(discovery_handlers::get_interests(&node).await)
}

/// POST /api/discovery/interests/toggle — Toggle an interest category.
pub async fn toggle_interest(
    body: web::Json<discovery_handlers::ToggleInterestRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    handler_result_to_response(discovery_handlers::toggle_interest(&body, &node).await)
}

/// POST /api/discovery/interests/detect — Manually trigger interest detection.
pub async fn detect_interests(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    handler_result_to_response(discovery_handlers::detect_interests(&node).await)
}
