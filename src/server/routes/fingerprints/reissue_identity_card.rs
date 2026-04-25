//! Actix route for `POST /api/fingerprints/my-identity-card/reissue`.
//! Thin wrapper over `handlers::fingerprints::reissue_identity_card`.

use crate::handlers::fingerprints as fp_handlers;
use crate::handlers::fingerprints::ReissueIdentityCardRequest;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_result_to_response, node_or_return};
use actix_web::{web, Responder};

pub async fn reissue_identity_card(
    state: web::Data<AppState>,
    body: web::Json<ReissueIdentityCardRequest>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);
    handler_result_to_response(fp_handlers::reissue_identity_card(node, body.into_inner()).await)
}
