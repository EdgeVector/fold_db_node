//! Photo moment detection handlers, extracted from the discovery module.

use super::get_metadata_store;
use super::MAX_PHOTO_BATCH;
use crate::discovery::moments;
use crate::discovery::types::{
    MomentHashReceiveRequest, MomentOptInRequest, MomentOptOutRequest, PhotoMetadata,
};
use crate::fold_node::node::FoldNode;
use crate::handlers::response::{ApiResponse, HandlerError, HandlerResult, IntoHandlerError};
use serde::{Deserialize, Serialize};

#[cfg(feature = "ts-bindings")]
use ts_rs::TS;

// === Photo Moment Detection Handlers ===

/// Response for moment opt-in list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(
    feature = "ts-bindings",
    ts(export, export_to = "src/fold_node/static-react/src/types/")
)]
pub struct MomentOptInListResponse {
    pub opt_ins: Vec<moments::MomentOptIn>,
}

/// Response for shared moments list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(
    feature = "ts-bindings",
    ts(export, export_to = "src/fold_node/static-react/src/types/")
)]
pub struct SharedMomentsResponse {
    pub moments: Vec<moments::SharedMoment>,
}

/// Response for moment hash scan.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(
    feature = "ts-bindings",
    ts(export, export_to = "src/fold_node/static-react/src/types/")
)]
pub struct MomentScanResponse {
    pub photos_scanned: usize,
    pub hashes_generated: usize,
    pub peers_processed: usize,
}

/// Response for moment detection run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(
    feature = "ts-bindings",
    ts(export, export_to = "src/fold_node/static-react/src/types/")
)]
pub struct MomentDetectResponse {
    pub new_moments_found: usize,
    pub moments: Vec<moments::SharedMoment>,
}

/// Opt-in to photo moment sharing with a peer.
pub async fn moment_opt_in(
    req: &MomentOptInRequest,
    node: &FoldNode,
) -> HandlerResult<MomentOptInListResponse> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    let opt_in = moments::MomentOptIn {
        peer_pseudonym: req.peer_pseudonym.clone(),
        peer_display_name: req.peer_display_name.clone(),
        opted_in_at: chrono::Utc::now().to_rfc3339(),
    };

    moments::save_moment_opt_in(&*store, &opt_in)
        .await
        .handler_err("save moment opt-in")?;

    let opt_ins = moments::list_moment_opt_ins(&*store)
        .await
        .handler_err("list moment opt-ins")?;

    Ok(ApiResponse::success(MomentOptInListResponse { opt_ins }))
}

/// Opt-out of photo moment sharing with a peer.
pub async fn moment_opt_out(
    req: &MomentOptOutRequest,
    node: &FoldNode,
) -> HandlerResult<MomentOptInListResponse> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    moments::remove_moment_opt_in(&*store, &req.peer_pseudonym)
        .await
        .handler_err("remove moment opt-in")?;

    let opt_ins = moments::list_moment_opt_ins(&*store)
        .await
        .handler_err("list moment opt-ins")?;

    Ok(ApiResponse::success(MomentOptInListResponse { opt_ins }))
}

/// List all moment opt-ins.
pub async fn moment_opt_in_list(node: &FoldNode) -> HandlerResult<MomentOptInListResponse> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    let opt_ins = moments::list_moment_opt_ins(&*store)
        .await
        .handler_err("list moment opt-ins")?;

    Ok(ApiResponse::success(MomentOptInListResponse { opt_ins }))
}

/// Scan local photos and generate moment hashes for all opted-in peers.
pub async fn moment_scan(
    node: &FoldNode,
    master_key: &[u8],
    photo_metadata: &[PhotoMetadata],
) -> HandlerResult<MomentScanResponse> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    if photo_metadata.len() > MAX_PHOTO_BATCH {
        return Err(HandlerError::BadRequest(format!(
            "Too many photos in batch: {} (max {})",
            photo_metadata.len(),
            MAX_PHOTO_BATCH
        )));
    }

    let opt_ins = moments::list_moment_opt_ins(&*store)
        .await
        .handler_err("list moment opt-ins")?;

    if opt_ins.is_empty() {
        return Ok(ApiResponse::success(MomentScanResponse {
            photos_scanned: photo_metadata.len(),
            hashes_generated: 0,
            peers_processed: 0,
        }));
    }

    let our_pseudo_hash = crate::discovery::pseudonym::content_hash("moment-sharing");
    let our_pseudonym = crate::discovery::pseudonym::derive_pseudonym(master_key, &our_pseudo_hash);
    let our_pseudonym_str = our_pseudonym.to_string();

    let mut total_hashes = 0;

    for opt_in in &opt_ins {
        let shared_secret = moments::derive_peer_shared_secret(
            master_key,
            &our_pseudonym_str,
            &opt_in.peer_pseudonym,
        );

        let mut all_hashes = Vec::new();

        for photo in photo_metadata {
            let ts = photo
                .timestamp
                .parse::<chrono::DateTime<chrono::Utc>>()
                .map_err(|e| {
                    HandlerError::BadRequest(format!(
                        "Invalid timestamp '{}': {}",
                        photo.timestamp, e
                    ))
                })?;

            let hashes = moments::generate_moment_hashes(
                &ts,
                photo.latitude,
                photo.longitude,
                &photo.record_id,
                &shared_secret,
            );
            all_hashes.extend(hashes);
        }

        total_hashes += all_hashes.len();
        moments::save_our_moment_hashes(&*store, &opt_in.peer_pseudonym, &all_hashes)
            .await
            .handler_err("save moment hashes")?;
    }

    Ok(ApiResponse::success(MomentScanResponse {
        photos_scanned: photo_metadata.len(),
        hashes_generated: total_hashes,
        peers_processed: opt_ins.len(),
    }))
}

/// Receive moment hashes from a peer (via encrypted exchange).
pub async fn moment_receive_hashes(
    req: &MomentHashReceiveRequest,
    node: &FoldNode,
) -> HandlerResult<()> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    let has_opt_in = moments::has_moment_opt_in(&*store, &req.sender_pseudonym)
        .await
        .handler_err("check moment opt-in")?;

    if !has_opt_in {
        return Err(HandlerError::BadRequest(format!(
            "No moment opt-in for peer {}. Both peers must opt-in first.",
            req.sender_pseudonym
        )));
    }

    let exchange = moments::MomentHashExchange {
        sender_pseudonym: req.sender_pseudonym.clone(),
        hashes: req.hashes.clone(),
        exchanged_at: chrono::Utc::now().to_rfc3339(),
    };

    moments::save_peer_moment_hashes(&*store, &req.sender_pseudonym, &exchange)
        .await
        .handler_err("save peer moment hashes")?;

    Ok(ApiResponse::success(()))
}

/// Detect shared moments by comparing our hashes with received peer hashes.
pub async fn moment_detect(node: &FoldNode) -> HandlerResult<MomentDetectResponse> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    let opt_ins = moments::list_moment_opt_ins(&*store)
        .await
        .handler_err("list moment opt-ins")?;

    let mut all_new_moments = Vec::new();

    for opt_in in &opt_ins {
        let new_moments = moments::detect_shared_moments(
            &*store,
            &opt_in.peer_pseudonym,
            opt_in.peer_display_name.as_deref(),
        )
        .await
        .handler_err("detect shared moments")?;

        all_new_moments.extend(new_moments);
    }

    let count = all_new_moments.len();
    Ok(ApiResponse::success(MomentDetectResponse {
        new_moments_found: count,
        moments: all_new_moments,
    }))
}

/// List all detected shared moments.
pub async fn moment_list(node: &FoldNode) -> HandlerResult<SharedMomentsResponse> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    let shared_moments = moments::list_shared_moments(&*store)
        .await
        .handler_err("list shared moments")?;

    Ok(ApiResponse::success(SharedMomentsResponse {
        moments: shared_moments,
    }))
}
