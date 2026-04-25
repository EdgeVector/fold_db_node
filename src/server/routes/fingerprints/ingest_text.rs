//! Actix route for text-signal ingestion. Thin wrapper over
//! `crate::handlers::fingerprints::ingest_text`.

use crate::handlers::fingerprints as fp_handlers;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_result_to_response, node_or_return};
use actix_web::{web, Responder};

/// POST /api/fingerprints/ingest-text-signals — batch-ingest
/// email/phone signals from text records (Notes, Messages, etc.).
pub async fn ingest_text_signals(
    body: web::Json<fp_handlers::IngestTextSignalsRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);
    handler_result_to_response(
        fp_handlers::ingest_text_signals_batch(node, body.into_inner()).await,
    )
}
