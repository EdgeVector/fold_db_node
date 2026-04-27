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
use crate::handlers::response::{require_non_empty, ApiResponse, HandlerResult};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ── Request / response types ─────────────────────────────────────

/// Shape of a single detected face. Used on the way in (as the
/// caller's upload shape for `ingest-photo-faces`) and on the way
/// out (as the pure-compute response shape for `detect-faces`),
/// so it derives both `Deserialize` and `Serialize`.
#[derive(Debug, Clone, Deserialize, Serialize)]
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
    /// Caller signals that this record is expected to contain
    /// identity content (e.g. the upstream filter flagged it as a
    /// human-taken photo, not scenery). When `true` and every
    /// applicable extractor ran empty with no IngestionError
    /// written, a meta-level `ZeroExtractorYield` IngestionError is
    /// emitted so the silent-gap case is surfaced in the Failed
    /// panel. Absent/false means "no claim" — the zero-yield check
    /// is skipped and the record is treated as legitimately empty.
    /// See TODO-6 in the workspace backlog.
    #[serde(default)]
    pub expected_to_yield: bool,
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
    require_non_empty(
        &request.source_schema,
        "source_schema must be a non-empty string",
    )?;

    let now_iso8601 = Utc::now().to_rfc3339();
    let total_photos = request.photos.len();

    let mut per_photo: Vec<PhotoIngestResult> = Vec::with_capacity(total_photos);
    let mut successful_photos = 0usize;
    let mut total_faces = 0usize;
    let mut total_records_written = 0usize;

    tracing::info!(
        "fingerprints.ingest: starting batch ingest of {} photos under schema '{}'",
        total_photos,
        request.source_schema
    );

    for photo in request.photos {
        let source_key = photo.source_key.clone();
        let expected_to_yield = photo.expected_to_yield;
        let faces: Vec<DetectedFace> = photo.faces.into_iter().map(DetectedFace::from).collect();

        // Validate before handing to the planner. Empty embeddings
        // and malformed floats get caught here and surface as a
        // loud per-photo IngestionError row, instead of being
        // silently stored as useless "face embedding (0 dims)"
        // fingerprints — which was one of the friction points in
        // the Phase 1 UI walkthrough.
        if let Err((error_class, error_msg)) = validate_face_inputs(&faces) {
            record_photo_failure(
                node.clone(),
                &request.source_schema,
                &source_key,
                error_class,
                &error_msg,
                &mut per_photo,
            )
            .await;
            continue;
        }

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
                // TODO-6: surface the silent-gap case. The writer
                // succeeded (so no per-extractor IngestionError was
                // emitted) and the extractor saw zero faces; if the
                // caller flagged this record as expected to yield,
                // emit the meta-level ZeroExtractorYield row so the
                // Failed panel shows it.
                if expected_to_yield && ran_empty {
                    crate::fingerprints::ingestion_error_writer::emit_zero_yield_meta_error(
                        node.clone(),
                        &request.source_schema,
                        &source_key,
                        "face_detect ran with zero fingerprints despite expected_to_yield=true",
                    )
                    .await;
                }
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
                record_photo_failure(
                    node.clone(),
                    &request.source_schema,
                    &source_key,
                    "WriterError",
                    &msg,
                    &mut per_photo,
                )
                .await;
            }
        }
    }

    tracing::info!(
        "fingerprints.ingest: batch complete: {}/{} successful, {} faces, {} records written",
        successful_photos,
        total_photos,
        total_faces,
        total_records_written,
    );

    // Fire-and-forget post-ingest sweep — auto-creates tentative
    // Personas for any dense cluster that emerged or grew. Debounced
    // in auto_propose so a large migration doesn't trigger N sweeps.
    if total_records_written > 0 {
        let node_bg = node.clone();
        tokio::spawn(async move {
            crate::fingerprints::auto_propose::run_sweep_and_create_personas(node_bg).await;
        });
    }

    Ok(ApiResponse::success(IngestPhotoFacesResponse {
        total_photos,
        successful_photos,
        total_faces,
        total_records_written,
        per_photo,
    }))
}

/// Input validation for a single photo's face list. Rejects the
/// failure modes that would otherwise land as silent garbage
/// fingerprints in the store:
///
/// - Any face whose embedding vector is empty.
/// - Any face whose embedding contains NaN or infinite values —
///   those would poison the similarity graph.
///
/// Returns `(error_class, human-readable message)` on the first
/// violation, or `Ok(())` when every face is well-formed.
fn validate_face_inputs(faces: &[DetectedFace]) -> Result<(), (&'static str, String)> {
    for (idx, face) in faces.iter().enumerate() {
        if face.embedding.is_empty() {
            return Err((
                "EmptyFaceEmbedding",
                format!("face[{}] has an empty embedding vector", idx),
            ));
        }
        if let Some((bad_idx, bad_val)) = face
            .embedding
            .iter()
            .enumerate()
            .find(|(_, v)| !v.is_finite())
        {
            return Err((
                "MalformedFaceEmbedding",
                format!(
                    "face[{}].embedding[{}] = {} is not a finite number",
                    idx, bad_idx, bad_val
                ),
            ));
        }
    }
    Ok(())
}

/// Record a per-photo failure: push a `PhotoIngestResult { ok: false }`
/// row so the batch caller sees the failure in the response, AND
/// emit a loud `IngestionError` row so the Failed panel in the UI
/// can surface the failure for the user to retry or dismiss. Both
/// sides are independently best-effort — the response row goes in
/// memory, the IngestionError is persisted.
async fn record_photo_failure(
    node: Arc<FoldNode>,
    source_schema: &str,
    source_key: &str,
    error_class: &str,
    error_msg: &str,
    per_photo: &mut Vec<PhotoIngestResult>,
) {
    tracing::warn!(
        "fingerprints.ingest: photo '{}' on schema '{}' failed ({}): {}",
        source_key,
        source_schema,
        error_class,
        error_msg
    );

    crate::fingerprints::ingestion_error_writer::write_ingestion_error(
        node,
        crate::fingerprints::ingestion_error_writer::IngestionErrorRecord {
            source_schema,
            source_key,
            extractor: "face_detect",
            error_class,
            error_msg,
        },
    )
    .await;

    per_photo.push(PhotoIngestResult {
        source_key: source_key.to_string(),
        ok: false,
        face_count: 0,
        records_written: 0,
        ran_empty: false,
        error: Some(format!("{}: {}", error_class, error_msg)),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn face(emb: Vec<f32>) -> DetectedFace {
        DetectedFace {
            embedding: emb,
            bbox: [0.0, 0.0, 0.0, 0.0],
            confidence: 0.9,
        }
    }

    #[test]
    fn validate_face_inputs_accepts_well_formed_embedding() {
        assert!(validate_face_inputs(&[face(vec![0.1, 0.2, 0.3])]).is_ok());
    }

    #[test]
    fn validate_face_inputs_rejects_empty_embedding() {
        let err = validate_face_inputs(&[face(vec![])]).unwrap_err();
        assert_eq!(err.0, "EmptyFaceEmbedding");
        assert!(err.1.contains("face[0]"));
    }

    #[test]
    fn validate_face_inputs_rejects_nan() {
        let err = validate_face_inputs(&[face(vec![0.1, f32::NAN, 0.3])]).unwrap_err();
        assert_eq!(err.0, "MalformedFaceEmbedding");
        assert!(err.1.contains("face[0].embedding[1]"));
    }

    #[test]
    fn validate_face_inputs_rejects_infinity() {
        let err = validate_face_inputs(&[face(vec![0.1, f32::INFINITY])]).unwrap_err();
        assert_eq!(err.0, "MalformedFaceEmbedding");
        assert!(err.1.contains("face[0].embedding[1]"));
    }

    #[test]
    fn validate_face_inputs_reports_the_second_face_when_the_first_is_fine() {
        let err = validate_face_inputs(&[face(vec![0.1, 0.2]), face(vec![])]).unwrap_err();
        assert_eq!(err.0, "EmptyFaceEmbedding");
        assert!(err.1.contains("face[1]"));
    }

    #[test]
    fn validate_face_inputs_accepts_empty_face_list() {
        // An empty list means "extractor ran, found no faces" — that
        // is the ran_empty branch and must NOT be a failure.
        assert!(validate_face_inputs(&[]).is_ok());
    }

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

    #[test]
    fn expected_to_yield_defaults_false_when_absent() {
        // Omitting the field must still deserialize — existing callers
        // (e.g. migrate_photos) predate TODO-6 and do not send it.
        let raw = json!({
            "source_key": "IMG_1",
            "faces": []
        });
        let dto: PhotoFacesDto = serde_json::from_value(raw).expect("deserialize");
        assert!(!dto.expected_to_yield);
    }

    #[test]
    fn expected_to_yield_round_trips_true() {
        let raw = json!({
            "source_key": "IMG_1",
            "faces": [],
            "expected_to_yield": true
        });
        let dto: PhotoFacesDto = serde_json::from_value(raw).expect("deserialize");
        assert!(dto.expected_to_yield);
    }
}
