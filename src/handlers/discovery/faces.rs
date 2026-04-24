//! Face search handlers, extracted from the discovery module.

use super::{DiscoveryNetworkSearchResponse, MAX_TOP_K};
use crate::discovery::publisher::DiscoveryPublisher;
use crate::fold_node::node::FoldNode;
use crate::handlers::response::{
    get_db_guard, ApiResponse, HandlerError, HandlerResult, IntoHandlerError,
};
use serde::{Deserialize, Serialize};

// === Face Discovery ===

/// Request to search for similar faces on the discovery network.
#[derive(Debug, Clone, Deserialize)]
pub struct FaceSearchRequest {
    pub source_schema: String,
    pub source_key: String,
    pub face_index: Option<usize>,
    pub top_k: Option<usize>,
}

/// A single face entry returned by the list_faces handler.
///
/// `bbox` and `confidence` are populated for face entries written after
/// fold_db's bbox plumb-through landed (see fold_db PR #535). Older entries
/// return `None` for both — the React UI should render those as "—" rather
/// than failing, so existing photos still appear in the list.
#[derive(Debug, Clone, Serialize)]
pub struct FaceEntry {
    pub face_index: usize,
    /// Normalized bounding box `[x1, y1, x2, y2]` in `[0, 1]` of the source
    /// image. None for legacy entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bbox: Option<[f32; 4]>,
    /// Detector confidence in `[0, 1]`. None for legacy entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
}

/// Response for listing faces on a record.
#[derive(Debug, Clone, Serialize)]
pub struct ListFacesResponse {
    pub faces: Vec<FaceEntry>,
}

/// List all face embeddings stored for a specific record.
/// Returns face indices, normalized bbox, and detector confidence
/// (the full embedding vectors stay server-side).
pub async fn list_faces(
    node: &FoldNode,
    schema: &str,
    key: &str,
) -> HandlerResult<ListFacesResponse> {
    let db = get_db_guard(node)?;

    let db_ops = db.get_db_ops();
    let native_index_mgr = db_ops
        .native_index_manager()
        .ok_or_else(|| HandlerError::Internal("Native index not available".to_string()))?;

    let key_value = fold_db::schema::types::key_value::KeyValue::new(Some(key.to_string()), None);
    let faces = native_index_mgr.list_faces(schema, &key_value);

    let entries: Vec<FaceEntry> = faces
        .into_iter()
        .map(|f| FaceEntry {
            face_index: f.fragment_idx,
            bbox: f.bbox,
            confidence: f.confidence,
        })
        .collect();

    Ok(ApiResponse::success(ListFacesResponse { faces: entries }))
}

/// Search the discovery network using a face embedding from a local record.
pub async fn face_search(
    req: &FaceSearchRequest,
    node: &FoldNode,
    discovery_url: &str,
    auth_token: &str,
    master_key: &[u8],
) -> HandlerResult<DiscoveryNetworkSearchResponse> {
    let face_index = req.face_index.unwrap_or(0);

    let db = get_db_guard(node)?;

    let db_ops = db.get_db_ops();
    let native_index_mgr = db_ops
        .native_index_manager()
        .ok_or_else(|| HandlerError::Internal("Native index not available".to_string()))?;

    let key_value =
        fold_db::schema::types::key_value::KeyValue::new(Some(req.source_key.clone()), None);
    let faces = native_index_mgr.list_faces(&req.source_schema, &key_value);

    let face = faces
        .into_iter()
        .find(|f| f.fragment_idx == face_index)
        .ok_or_else(|| {
            HandlerError::NotFound(format!(
                "No face at index {} for schema='{}' key='{}'",
                face_index, req.source_schema, req.source_key
            ))
        })?;
    let embedding = face.embedding;

    let publisher = DiscoveryPublisher::new(
        master_key.to_vec(),
        discovery_url.to_string(),
        auth_token.to_string(),
    );

    let top_k = req.top_k.unwrap_or(20).min(MAX_TOP_K);

    let results = publisher
        .search_with_threshold(embedding, top_k, None, None, None, "face".to_string())
        .await
        .handler_err("face search on discovery network")?;

    Ok(ApiResponse::success(DiscoveryNetworkSearchResponse {
        results,
    }))
}
