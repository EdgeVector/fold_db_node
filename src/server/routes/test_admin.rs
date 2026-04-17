//! HTTP routes for test-admin endpoints (local multi-node testing only).
//!
//! All routes here require `FOLDDB_ENABLE_TEST_ADMIN=1` in the environment.
//! The handlers themselves re-check this per-request.

use crate::handlers::admin as admin_handlers;
use crate::server::http_server::AppState;
use crate::server::routes::discovery::get_discovery_config;
use crate::server::routes::{
    handler_error_to_response, handler_result_to_response, node_or_return,
};
use actix_web::{web, HttpResponse, Responder};

/// POST /api/test-admin/contacts — insert/update a contact directly, bypassing discovery.
pub async fn upsert_contact(
    body: web::Json<admin_handlers::UpsertContactRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(admin_handlers::upsert_contact(&body, &user_hash, &node).await)
}

/// GET /api/test-admin/my-messaging-keys — dump pseudonym + X25519 pubkey pairs
/// so a peer can import this node as a contact via upsert_contact.
pub async fn my_messaging_keys(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);

    let (_url, master_key) = match get_discovery_config(&state).await {
        Ok(c) => c,
        Err(response) => return response,
    };

    match admin_handlers::my_messaging_keys(&user_hash, &node, &master_key).await {
        Ok(resp) => HttpResponse::Ok().json(resp),
        Err(e) => handler_error_to_response(e),
    }
}
