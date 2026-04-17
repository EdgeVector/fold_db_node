//! Actix route for `POST /api/fingerprints/my-identity-card/reissue`.
//! Thin wrapper over `handlers::fingerprints::reissue_identity_card`.

use crate::handlers::fingerprints as fp_handlers;
use crate::handlers::fingerprints::ReissueIdentityCardRequest;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_error_to_response, node_or_return};
use actix_web::{web, HttpResponse, Responder};

pub async fn reissue_identity_card(
    state: web::Data<AppState>,
    body: web::Json<ReissueIdentityCardRequest>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);
    match fp_handlers::reissue_identity_card(node, body.into_inner()).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}
