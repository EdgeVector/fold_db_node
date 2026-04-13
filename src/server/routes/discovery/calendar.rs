//! Calendar-sharing endpoints — opt-in, sync events, detect shared events.

use crate::handlers::discovery as discovery_handlers;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_error_to_response, node_or_return};
use actix_web::{web, HttpResponse, Responder};

/// GET /api/discovery/calendar-sharing/status — Get calendar sharing opt-in status.
pub async fn calendar_sharing_status(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::calendar_sharing_status(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/calendar-sharing/opt-in — Enable calendar sharing.
pub async fn calendar_sharing_opt_in(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::calendar_sharing_opt_in(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/calendar-sharing/opt-out — Disable calendar sharing.
pub async fn calendar_sharing_opt_out(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::calendar_sharing_opt_out(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/calendar-sharing/sync — Sync calendar events for comparison.
pub async fn sync_calendar_events(
    body: web::Json<discovery_handlers::SyncCalendarEventsRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::sync_calendar_events(&body, &node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/calendar-sharing/peer-events — Store peer event fingerprints.
pub async fn store_peer_events(
    body: web::Json<discovery_handlers::StorePeerEventsRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::store_peer_events(&body, &node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/discovery/shared-events — Detect and return shared events with connections.
pub async fn get_shared_events(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::get_shared_events(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}
