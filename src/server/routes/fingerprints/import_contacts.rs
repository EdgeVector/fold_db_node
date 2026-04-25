//! Actix route for contact book import.

use crate::handlers::fingerprints as fp_handlers;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_result_to_response, node_or_return};
use actix_web::{web, Responder};

/// POST /api/fingerprints/import-contacts — import the trust contact
/// book into the fingerprint graph. No body needed; reads contacts
/// from disk. Idempotent.
pub async fn import_contacts(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);
    handler_result_to_response(fp_handlers::import_contacts(node).await)
}
