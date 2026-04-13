//! Notification endpoints — list, count, dismiss.

use crate::handlers::discovery as discovery_handlers;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_error_to_response, node_or_return};
use actix_web::{web, HttpResponse, Responder};

/// GET /api/notifications — List all notifications.
pub async fn list_notifications(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::list_notifications(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/notifications/count — Lightweight notification count for polling.
pub async fn notification_count(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::notification_count(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// DELETE /api/notifications/{id} — Dismiss a notification.
pub async fn dismiss_notification(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);
    let notification_id = path.into_inner();

    match discovery_handlers::dismiss_notification(&node, &notification_id).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}
