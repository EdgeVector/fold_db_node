//! Actix route for `GET /api/fingerprints/my-identity-card`.
//! Thin wrapper over `handlers::fingerprints::my_identity_card`.

use crate::handlers::fingerprints as fp_handlers;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_error_to_response, node_or_return};
use actix_web::{web, HttpResponse, Responder};

pub async fn get_my_identity_card(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);
    match fp_handlers::get_my_identity_card(node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}
