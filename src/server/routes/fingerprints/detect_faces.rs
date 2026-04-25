//! Actix route handler for pure face detection. Thin wrapper over
//! `crate::handlers::fingerprints::detect_faces::detect_faces`.
//!
//! Gated behind the `face-detection` cargo feature to match the
//! handler. Builds without the feature do not register this route.

#![cfg(feature = "face-detection")]

use crate::handlers::fingerprints as fp_handlers;
use crate::handlers::fingerprints::DetectFacesRequest;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_result_to_response, node_or_return};
use actix_web::{web, HttpResponse, Responder};

/// POST /api/fingerprints/detect-faces
///
/// Run the local ONNX face detector over a single base64-encoded
/// image and return the embeddings, bounding boxes, and confidence
/// scores for every detected face. Pure compute — no database
/// writes, no discovery side effects.
pub async fn detect_faces(
    body: web::Json<DetectFacesRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);
    let request = body.into_inner();

    handler_result_to_response(fp_handlers::detect_faces(node, request).await)
}
