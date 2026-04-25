//! Calendar-sharing endpoints — opt-in, sync events, detect shared events.

use crate::handlers::discovery as discovery_handlers;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_result_to_response, node_or_return};
use actix_web::{web, Responder};

/// GET /api/discovery/calendar-sharing/status — Get calendar sharing opt-in status.
pub async fn calendar_sharing_status(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    handler_result_to_response(discovery_handlers::calendar_sharing_status(&node).await)
}

/// POST /api/discovery/calendar-sharing/opt-in — Enable calendar sharing.
pub async fn calendar_sharing_opt_in(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    handler_result_to_response(discovery_handlers::calendar_sharing_opt_in(&node).await)
}

/// POST /api/discovery/calendar-sharing/opt-out — Disable calendar sharing.
pub async fn calendar_sharing_opt_out(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    handler_result_to_response(discovery_handlers::calendar_sharing_opt_out(&node).await)
}

/// POST /api/discovery/calendar-sharing/sync — Sync calendar events for comparison.
pub async fn sync_calendar_events(
    body: web::Json<discovery_handlers::SyncCalendarEventsRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    handler_result_to_response(discovery_handlers::sync_calendar_events(&body, &node).await)
}

/// POST /api/discovery/calendar-sharing/peer-events — Store peer event fingerprints.
pub async fn store_peer_events(
    body: web::Json<discovery_handlers::StorePeerEventsRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    handler_result_to_response(discovery_handlers::store_peer_events(&body, &node).await)
}

/// GET /api/discovery/shared-events — Detect and return shared events with connections.
pub async fn get_shared_events(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    handler_result_to_response(discovery_handlers::get_shared_events(&node).await)
}
