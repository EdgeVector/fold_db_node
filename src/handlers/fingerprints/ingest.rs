//! Photo ingestion handler — wraps
//! [`crate::fingerprints::ingest_photo::ingest_photo_faces`] in a
//! batch-friendly HTTP shape so callers can push the faces detected
//! on many photos in a single request.
//!
//! ## Why a batch endpoint
//!
//! Migration scripts (the "re-process every existing photo"
//! use case) typically build up the full list of detected faces
//! upfront — either from a pre-computed JSON file or by running an
//! ONNX face detector over the image store in a separate script —
//! and then want to POST the whole set to the node in a single
//! round trip. Per-photo HTTP calls would multiply latency by the
//! number of photos for no gain: each record write is already
//! independent inside the writer.
//!
//! ## Idempotency
//!
//! Every Fingerprint / Edge record is content-addressed, and every
//! Mention / ExtractionStatus record is keyed deterministically on
//! the `(source_schema, source_key, "face_detect")` triple (see
//! [`crate::fingerprints::ingest_photo::deterministic_mention_id`]).
//! Re-running the same batch produces the same keys and overwrites
//! the existing records in place, so callers can safely retry on
//! failure.
//!
//! ## Trust boundary
//!
//! This handler is reachable via `POST /api/fingerprints/ingest-photo-faces`.
//! Per the loopback-owner invariant documented in `CLAUDE.md`, every
//! local HTTP request already runs under owner context, so no
//! additional auth check is needed here — the trust tier gate on
//! the Fingerprint/Mention/Edge mutations is satisfied by the owner
//! short-circuit in `build_access_context`.

use crate::fingerprints::extractors::face::DetectedFace;
use crate::fingerprints::ingest_photo::{ingest_photo_faces, IngestionOutcome};
use crate::fold_node::FoldNode;
use crate::handlers::response::{ApiResponse, HandlerError, HandlerResult};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ── Request / response types ─────────────────────────────────────

/// Shape of a single detected face as sent by the caller.
#[derive(Debug, Clone, Deserialize)]
pub struct DetectedFaceDto {
    /// Raw face embedding — typically 512 f32 values from ArcFace,
    /// but this handler imposes no dimension requirement. Malformed
    /// embeddings are caught by the Fingerprint schema's field type
    /// validation at the writer.
    pub embedding: Vec<f32>,
    /// Bounding box `[x, y, w, h]` in image pixels. Serialized but
    /// not yet used for downstream logic; reserved for future UI
    /// features that highlight faces on the photo.
    pub bbox: [f32; 4],
    /// Detector confidence in `[0, 1]`.
    pub confidence: f32,
}

impl From<DetectedFaceDto> for DetectedFace {
    fn from(dto: DetectedFaceDto) -> Self {
        DetectedFace {
            embedding: dto.embedding,
            bbox: dto.bbox,
            confidence: dto.confidence,
        }
    }
}

/// One photo's worth of face-detection output inside a batch
/// request.
#[derive(Debug, Clone, Deserialize)]
pub struct PhotoFacesDto {
    /// Primary key of the Photo record on the source schema, e.g.
    /// `"IMG_1234"`.
    pub source_key: String,
    /// Faces detected on this photo. Empty means "extractor ran
    /// but found nothing" — the node still writes an ExtractionStatus
    /// record so the UI can distinguish "not yet processed" from
    /// "processed and empty."
    pub faces: Vec<DetectedFaceDto>,
}

/// Full batch ingest request body.
#[derive(Debug, Clone, Deserialize)]
pub struct IngestPhotoFacesRequest {
    /// Descriptive name of the schema the photos live on, e.g.
    /// `"Photos"`. Every entry in `photos` targets this same schema.
    /// Cross-schema batches are not supported — callers can send
    /// multiple requests if they need to migrate from more than one
    /// source schema.
    pub source_schema: String,
    /// Per-photo face sets. The order is preserved in the response
    /// so callers can correlate results by index if they want to.
    pub photos: Vec<PhotoFacesDto>,
}

/// Per-photo result summary returned in the response.
#[derive(Debug, Clone, Serialize)]
pub struct PhotoIngestResult {
    pub source_key: String,
    /// `true` when the photo was processed successfully. `false`
    /// when the writer returned an error. On failure, `error`
    /// carries a short message and `records_written` / `face_count`
    /// are zero.
    pub ok: bool,
    pub face_count: usize,
    pub records_written: usize,
    /// True when the extractor ran but found no faces. Mutually
    /// exclusive with `ok == false`.
    pub ran_empty: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Full batch ingest response body.
#[derive(Debug, Clone, Serialize)]
pub struct IngestPhotoFacesResponse {
    pub total_photos: usize,
    pub successful_photos: usize,
    pub total_faces: usize,
    pub total_records_written: usize,
    pub per_photo: Vec<PhotoIngestResult>,
}

// ── Handler ─────────────────────────────────────────────────────

/// Batch-ingest face-detection results for a set of photos.
///
/// On a per-photo error, the handler logs the failure, records it
/// in the response, and keeps going — callers get partial success
/// rather than the whole batch aborting on the first bad row. This
/// matches the existing ingestion handlers' behavior and is
/// important for migrations over large photo corpora where a
/// single corrupt record should not block the rest.
pub async fn ingest_photo_faces_batch(
    node: Arc<FoldNode>,
    request: IngestPhotoFacesRequest,
) -> HandlerResult<IngestPhotoFacesResponse> {
    if request.source_schema.trim().is_empty() {
        return Err(HandlerError::BadRequest(
            "source_schema must be a non-empty string".to_string(),
        ));
    }

    let now_iso8601 = Utc::now().to_rfc3339();
    let total_photos = request.photos.len();

    let mut per_photo: Vec<PhotoIngestResult> = Vec::with_capacity(total_photos);
    let mut successful_photos = 0usize;
    let mut total_faces = 0usize;
    let mut total_records_written = 0usize;

    log::info!(
        "fingerprints.ingest: starting batch ingest of {} photos under schema '{}'",
        total_photos,
        request.source_schema
    );

    for photo in request.photos {
        let source_key = photo.source_key.clone();
        let faces: Vec<DetectedFace> = photo.faces.into_iter().map(DetectedFace::from).collect();

        match ingest_photo_faces(
            node.clone(),
            &request.source_schema,
            &source_key,
            &faces,
            &now_iso8601,
        )
        .await
        {
            Ok(IngestionOutcome {
                records_written,
                face_count,
                ran_empty,
            }) => {
                successful_photos += 1;
                total_faces += face_count;
                total_records_written += records_written;
                per_photo.push(PhotoIngestResult {
                    source_key,
                    ok: true,
                    face_count,
                    records_written,
                    ran_empty,
                    error: None,
                });
            }
            Err(e) => {
                let msg = format!("{}", e);
                log::warn!(
                    "fingerprints.ingest: photo '{}' on schema '{}' failed: {}",
                    source_key,
                    request.source_schema,
                    msg
                );
                per_photo.push(PhotoIngestResult {
                    source_key,
                    ok: false,
                    face_count: 0,
                    records_written: 0,
                    ran_empty: false,
                    error: Some(msg),
                });
            }
        }
    }

    log::info!(
        "fingerprints.ingest: batch complete: {}/{} successful, {} faces, {} records written",
        successful_photos,
        total_photos,
        total_faces,
        total_records_written,
    );

    Ok(ApiResponse::success(IngestPhotoFacesResponse {
        total_photos,
        successful_photos,
        total_faces,
        total_records_written,
        per_photo,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_deserializes_from_json() {
        let raw = json!({
            "source_schema": "Photos",
            "photos": [
                {
                    "source_key": "IMG_1",
                    "faces": [
                        {
                            "embedding": [0.1, 0.2, 0.3],
                            "bbox": [10.0, 20.0, 30.0, 40.0],
                            "confidence": 0.95,
                        }
                    ]
                },
                {
                    "source_key": "IMG_2",
                    "faces": []
                }
            ]
        });
        let req: IngestPhotoFacesRequest = serde_json::from_value(raw).expect("deserialize");
        assert_eq!(req.source_schema, "Photos");
        assert_eq!(req.photos.len(), 2);
        assert_eq!(req.photos[0].source_key, "IMG_1");
        assert_eq!(req.photos[0].faces.len(), 1);
        assert_eq!(req.photos[0].faces[0].embedding, vec![0.1, 0.2, 0.3]);
        assert_eq!(req.photos[1].faces.len(), 0);
    }

    #[test]
    fn response_serializes_compactly() {
        let resp = IngestPhotoFacesResponse {
            total_photos: 2,
            successful_photos: 1,
            total_faces: 3,
            total_records_written: 12,
            per_photo: vec![
                PhotoIngestResult {
                    source_key: "IMG_1".into(),
                    ok: true,
                    face_count: 3,
                    records_written: 12,
                    ran_empty: false,
                    error: None,
                },
                PhotoIngestResult {
                    source_key: "IMG_2".into(),
                    ok: false,
                    face_count: 0,
                    records_written: 0,
                    ran_empty: false,
                    error: Some("canonical_names not initialized".into()),
                },
            ],
        };
        let json = serde_json::to_value(&resp).expect("serialize");
        assert_eq!(json["total_photos"], 2);
        assert_eq!(json["successful_photos"], 1);
        // Failed photo's error field is present, successful photo's is absent.
        assert!(json["per_photo"][0]["error"].is_null());
        assert_eq!(
            json["per_photo"][1]["error"],
            "canonical_names not initialized"
        );
    }

    #[test]
    fn detected_face_dto_maps_to_detected_face() {
        let dto = DetectedFaceDto {
            embedding: vec![1.0, 2.0],
            bbox: [0.0, 1.0, 2.0, 3.0],
            confidence: 0.5,
        };
        let face = DetectedFace::from(dto);
        assert_eq!(face.embedding, vec![1.0, 2.0]);
        assert_eq!(face.bbox, [0.0, 1.0, 2.0, 3.0]);
        assert_eq!(face.confidence, 0.5);
    }
}
