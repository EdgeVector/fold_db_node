//! Actix route handler for photo face ingestion. Thin wrapper over
//! `crate::handlers::fingerprints::ingest::ingest_photo_faces_batch`.

use crate::handlers::fingerprints as fp_handlers;
use crate::handlers::fingerprints::IngestPhotoFacesRequest;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_error_to_response, node_or_return};
use actix_web::{web, HttpResponse, Responder};

/// POST /api/fingerprints/ingest-photo-faces
///
/// Batch-ingest pre-computed face detection results for a set of
/// photos. Typical caller is the photo migration script, which runs
/// ONNX face detection over the user's image corpus once and then
/// hands the results to this endpoint in one or more batches.
///
/// Per-photo errors do not abort the batch; callers get per-row
/// status back in the `per_photo` field of the response.
pub async fn ingest_photo_faces(
    body: web::Json<IngestPhotoFacesRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);
    let request = body.into_inner();

    match fp_handlers::ingest_photo_faces_batch(node, request).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}
