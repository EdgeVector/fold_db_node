//! Actix routes for the messaging card inbox. Thin wrappers over
//! `handlers::fingerprints::received_cards`.

use crate::handlers::fingerprints as fp_handlers;
use crate::handlers::fingerprints::AcceptReceivedCardRequest;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_error_to_response, node_or_return};
use actix_web::{web, HttpResponse, Responder};

pub async fn list_received_cards(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);
    match fp_handlers::list_received_cards(node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

pub async fn accept_received_card(
    path: web::Path<String>,
    body: Option<web::Json<AcceptReceivedCardRequest>>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);
    let message_id = path.into_inner();
    let req = body.map(|b| b.into_inner()).unwrap_or_default();
    match fp_handlers::accept_received_card(node, message_id, req).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

pub async fn dismiss_received_card(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);
    let message_id = path.into_inner();
    match fp_handlers::dismiss_received_card(node, message_id).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}
