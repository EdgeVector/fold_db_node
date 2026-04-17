//! Actix route for `GET /api/fingerprints/identities`.
//! Thin wrapper over `handlers::fingerprints::list_identities`.

use crate::handlers::fingerprints as fp_handlers;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_error_to_response, node_or_return};
use actix_web::{web, HttpResponse, Responder};

pub async fn list_identities(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);
    match fp_handlers::list_identities(node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}
