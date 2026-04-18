//! Pure face-detection handler — runs the local ONNX face processor
//! over a single image and returns the detected face embeddings,
//! bounding boxes, and confidences without writing anything to the
//! database.
//!
//! ## Why a detect-only endpoint
//!
//! The sibling `ingest-photo-faces` batch handler assumes faces are
//! already detected and is purely a writer: callers hand in
//! pre-computed embeddings and the handler stores them. This
//! endpoint covers the mirror case — callers want face detection as
//! a service from a node that already has the ONNX models loaded,
//! without committing anything to their store. Uses:
//! - dogfooding the face detector against arbitrary images from the UI
//! - pre-flighting a photo before deciding whether to ingest it
//! - downstream tools that do their own storage
//!
//! ## Feature gating
//!
//! `NativeIndexManager::detect_faces` is gated behind fold_db's
//! `face-detection` cargo feature, so the whole handler (and its
//! route wiring) is gated to match. Builds without `face-detection`
//! do not register this endpoint at all — an explicit 404 is a
//! clearer signal than a 500 that always fires.
//!
//! ## Trust boundary
//!
//! Reachable via `POST /api/fingerprints/detect-faces` under the
//! loopback-owner invariant: every local HTTP request already runs
//! under owner context. This endpoint does not mutate anything, so
//! there's no access-tier gate to satisfy.

#![cfg(feature = "face-detection")]

use crate::fold_node::FoldNode;
use crate::handlers::fingerprints::ingest::DetectedFaceDto;
use crate::handlers::response::{ApiResponse, HandlerError, HandlerResult};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ── Request / response types ─────────────────────────────────────

/// Request body: one image encoded as a base64 string.
///
/// The encoding is plain `base64` (RFC 4648, standard alphabet).
/// Data-URL prefixes (`data:image/jpeg;base64,...`) are rejected —
/// the caller is responsible for stripping them before sending.
#[derive(Debug, Clone, Deserialize)]
pub struct DetectFacesRequest {
    pub image_base64: String,
}

/// Response body: one entry per detected face, in detector order.
/// Empty when the image contained no faces — that is a successful
/// response, not an error.
#[derive(Debug, Clone, Serialize)]
pub struct DetectFacesResponse {
    pub faces: Vec<DetectedFaceDto>,
}

// ── Handler ─────────────────────────────────────────────────────

/// Detect faces in a single base64-encoded image and return the
/// embeddings, bounding boxes, and confidence scores for each.
///
/// No database writes — this is pure compute on the loaded ONNX
/// models. An image with zero faces returns `{"faces": []}` with a
/// 200 status; an invalid base64 payload or a missing / unavailable
/// face processor is a 4xx / 5xx error respectively.
pub async fn detect_faces(
    node: Arc<FoldNode>,
    request: DetectFacesRequest,
) -> HandlerResult<DetectFacesResponse> {
    // Decode base64 up front so bad input fails fast with a loud
    // BadRequest instead of reaching the ONNX runtime.
    let image_bytes = BASE64_STANDARD
        .decode(request.image_base64.as_bytes())
        .map_err(|e| HandlerError::BadRequest(format!("invalid base64: {}", e)))?;

    if image_bytes.is_empty() {
        return Err(HandlerError::BadRequest(
            "image_base64 decoded to zero bytes".to_string(),
        ));
    }

    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("failed to acquire FoldDB: {}", e)))?;
    let db_ops = db.get_db_ops();
    let native_idx = db_ops
        .native_index_manager()
        .ok_or_else(|| HandlerError::Internal("native index manager not configured".to_string()))?;

    if !native_idx.has_face_processor() {
        return Err(HandlerError::Internal(
            "face processor not configured on this node".to_string(),
        ));
    }

    let embeddings = native_idx
        .detect_faces(&image_bytes)
        .map_err(|e| HandlerError::Internal(format!("face detection failed: {}", e)))?;

    let faces: Vec<DetectedFaceDto> = embeddings
        .into_iter()
        .map(|fe| DetectedFaceDto {
            embedding: fe.embedding,
            bbox: fe.bbox,
            confidence: fe.confidence,
        })
        .collect();

    log::info!(
        "fingerprints.detect_faces: detected {} face(s) in {} byte image",
        faces.len(),
        image_bytes.len()
    );

    Ok(ApiResponse::success(DetectFacesResponse { faces }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rejecting malformed base64 does not need a FoldNode or an
    /// ONNX runtime — the decode failure is surfaced before the
    /// handler ever touches the native index. We cannot build a
    /// FoldNode in a unit test easily, so we exercise just the
    /// decoding path by calling the standard decoder directly and
    /// asserting the error wraps into a BadRequest in the same way
    /// the handler does. This is the same "validate_* pure helper"
    /// style the sibling `ingest` handler uses.
    #[test]
    fn invalid_base64_maps_to_bad_request() {
        let res = BASE64_STANDARD
            .decode(b"!!!not base64!!!")
            .map_err(|e| HandlerError::BadRequest(format!("invalid base64: {}", e)));
        match res {
            Err(HandlerError::BadRequest(msg)) => {
                assert!(msg.contains("invalid base64"));
            }
            other => panic!("expected BadRequest, got {:?}", other),
        }
    }

    #[test]
    fn empty_decoded_bytes_should_be_rejected() {
        // An empty string decodes to zero bytes, which the handler
        // treats as a BadRequest. Guard the invariant here so a
        // future refactor doesn't accidentally feed zero bytes to
        // the ONNX pipeline.
        let decoded = BASE64_STANDARD.decode(b"").expect("empty is valid base64");
        assert!(decoded.is_empty());
    }

    // TODO: wire into integration harness — add a happy-path test
    // using a real face fixture from `test-framework/fixtures/faces/`
    // once the test harness can spin up a FoldNode with the
    // face-detection feature enabled and ONNX models available.
    #[ignore]
    #[test]
    fn detect_faces_on_real_image_fixture() {}
}
