//! Shared Discovery Handlers
//!
//! Framework-agnostic handlers for discovery network operations.

use crate::discovery::config::{self, DiscoveryOptIn};
use crate::discovery::interests::{self, InterestProfile};
use crate::discovery::publisher::DiscoveryPublisher;
use crate::discovery::types::*;
use crate::fold_node::node::FoldNode;
use crate::handlers::response::{ApiResponse, HandlerError, HandlerResult, IntoHandlerError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[cfg(feature = "ts-bindings")]
use ts_rs::TS;

// === Request types ===

#[derive(Debug, Clone, Deserialize)]
pub struct OptInRequest {
    pub schema_name: String,
    pub category: String,
    pub include_preview: Option<bool>,
    pub preview_max_chars: Option<usize>,
    pub preview_excluded_fields: Option<Vec<String>>,
    pub field_privacy: Option<
        std::collections::HashMap<
            String,
            fold_db::db_operations::native_index::anonymity::FieldPrivacyClass,
        >,
    >,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OptOutRequest {
    pub schema_name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    pub top_k: Option<usize>,
    pub category_filter: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConnectRequest {
    pub target_pseudonym: uuid::Uuid,
    pub encrypted_blob: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToggleInterestRequest {
    pub category: String,
    pub enabled: bool,
}

// === Response types ===

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(
    feature = "ts-bindings",
    ts(export, export_to = "src/fold_node/static-react/src/types/")
)]
pub struct DiscoveryOptInListResponse {
    pub configs: Vec<DiscoveryOptIn>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(
    feature = "ts-bindings",
    ts(export, export_to = "src/fold_node/static-react/src/types/")
)]
pub struct DiscoveryPublishResponse {
    pub accepted: usize,
    pub quarantined: usize,
    pub total: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(
    feature = "ts-bindings",
    ts(export, export_to = "src/fold_node/static-react/src/types/")
)]
pub struct DiscoveryNetworkSearchResponse {
    pub results: Vec<DiscoverySearchResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(
    feature = "ts-bindings",
    ts(export, export_to = "src/fold_node/static-react/src/types/")
)]
pub struct DiscoveryConnectionsResponse {
    pub requests: Vec<IncomingConnectionRequest>,
}

/// A single anonymized profile aggregated from discovery search results.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(
    feature = "ts-bindings",
    ts(export, export_to = "src/fold_node/static-react/src/types/")
)]
pub struct SimilarProfile {
    pub pseudonym: uuid::Uuid,
    pub match_percentage: f32,
    pub shared_categories: Vec<String>,
    pub top_similarity: f32,
}

/// Response for the similar-profiles endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(
    feature = "ts-bindings",
    ts(export, export_to = "src/fold_node/static-react/src/types/")
)]
pub struct SimilarProfilesResponse {
    pub profiles: Vec<SimilarProfile>,
    pub user_categories_used: usize,
}

// === Handlers ===

/// Get the metadata KV store from a FoldDB guard.
fn get_metadata_store(
    db: &tokio::sync::OwnedMutexGuard<fold_db::fold_db_core::FoldDB>,
) -> std::sync::Arc<dyn fold_db::storage::traits::KvStore> {
    db.get_db_ops().metadata_store().inner().clone()
}

/// List all discovery opt-in configs.
pub async fn list_opt_ins(node: &FoldNode) -> HandlerResult<DiscoveryOptInListResponse> {
    let db = node
        .get_fold_db()
        .await
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    let configs = config::list_opt_ins(&*store)
        .await
        .handler_err("list discovery opt-ins")?;

    Ok(ApiResponse::success(DiscoveryOptInListResponse { configs }))
}

/// Opt-in a schema for discovery publication.
pub async fn opt_in(
    req: &OptInRequest,
    node: &FoldNode,
) -> HandlerResult<DiscoveryOptInListResponse> {
    let db = node
        .get_fold_db()
        .await
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    let mut opt_in_config = DiscoveryOptIn::new(req.schema_name.clone(), req.category.clone());

    if req.include_preview.unwrap_or(false) {
        opt_in_config = opt_in_config.with_preview(
            req.preview_max_chars.unwrap_or(100),
            req.preview_excluded_fields.clone().unwrap_or_default(),
        );
    }

    if let Some(ref field_privacy) = req.field_privacy {
        opt_in_config = opt_in_config.with_field_privacy(field_privacy.clone());
    }

    config::save_opt_in(&*store, &opt_in_config)
        .await
        .handler_err("save discovery opt-in")?;

    // Return updated list
    let configs = config::list_opt_ins(&*store)
        .await
        .handler_err("list discovery opt-ins")?;

    Ok(ApiResponse::success(DiscoveryOptInListResponse { configs }))
}

/// Opt-out a schema from discovery publication.
pub async fn opt_out(
    req: &OptOutRequest,
    node: &FoldNode,
    discovery_url: &str,
    auth_token: &str,
    master_key: &[u8],
) -> HandlerResult<DiscoveryOptInListResponse> {
    let db = node
        .get_fold_db()
        .await
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    // Remove local config
    config::remove_opt_in(&*store, &req.schema_name)
        .await
        .handler_err("remove discovery opt-in")?;

    // Tell the discovery service to remove published vectors
    let publisher = DiscoveryPublisher::new(
        master_key.to_vec(),
        discovery_url.to_string(),
        auth_token.to_string(),
    );
    publisher
        .unpublish_schema(&req.schema_name)
        .await
        .handler_err("unpublish from discovery service")?;

    let configs = config::list_opt_ins(&*store)
        .await
        .handler_err("list discovery opt-ins")?;

    Ok(ApiResponse::success(DiscoveryOptInListResponse { configs }))
}

/// Publish embeddings for all opted-in schemas.
pub async fn publish(
    node: &FoldNode,
    discovery_url: &str,
    auth_token: &str,
    master_key: &[u8],
) -> HandlerResult<DiscoveryPublishResponse> {
    let db = node
        .get_fold_db()
        .await
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;

    let db_ops = db.get_db_ops();
    let metadata_store = db_ops.metadata_store().inner().clone();
    let configs = config::list_opt_ins(&*metadata_store)
        .await
        .handler_err("list discovery opt-ins")?;

    if configs.is_empty() {
        return Ok(ApiResponse::success(DiscoveryPublishResponse {
            accepted: 0,
            quarantined: 0,
            total: 0,
            skipped: 0,
        }));
    }

    let native_index_mgr = db_ops
        .native_index_manager()
        .ok_or_else(|| HandlerError::Internal("Native index not available".to_string()))?;
    let embedding_store = native_index_mgr.store().clone();

    let publisher = DiscoveryPublisher::new(
        master_key.to_vec(),
        discovery_url.to_string(),
        auth_token.to_string(),
    );

    let mut total_accepted = 0;
    let mut total_quarantined = 0;
    let mut total_total = 0;
    let mut total_skipped = 0;

    for opt_in_config in &configs {
        match publisher
            .publish_schema(opt_in_config, &*embedding_store)
            .await
        {
            Ok(result) => {
                total_accepted += result.accepted;
                total_quarantined += result.quarantined;
                total_total += result.total;
                total_skipped += result.skipped;
            }
            Err(e) => {
                log::error!(
                    "Failed to publish schema '{}': {}",
                    opt_in_config.schema_name,
                    e
                );
                return Err(HandlerError::Internal(format!(
                    "Failed to publish schema '{}': {}",
                    opt_in_config.schema_name, e
                )));
            }
        }
    }

    Ok(ApiResponse::success(DiscoveryPublishResponse {
        accepted: total_accepted,
        quarantined: total_quarantined,
        total: total_total,
        skipped: total_skipped,
    }))
}

/// Search the discovery network.
pub async fn search(
    req: &SearchRequest,
    node: &FoldNode,
    discovery_url: &str,
    auth_token: &str,
    master_key: &[u8],
) -> HandlerResult<DiscoveryNetworkSearchResponse> {
    // Generate embedding from query text
    let db = node
        .get_fold_db()
        .await
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;

    let db_ops = db.get_db_ops();
    let native_index_mgr = db_ops
        .native_index_manager()
        .ok_or_else(|| HandlerError::Internal("Native index not available".to_string()))?;

    let query_embedding = native_index_mgr
        .embed_text(&req.query)
        .handler_err("generate query embedding")?;

    let publisher = DiscoveryPublisher::new(
        master_key.to_vec(),
        discovery_url.to_string(),
        auth_token.to_string(),
    );

    let results = publisher
        .search(
            query_embedding,
            req.top_k.unwrap_or(20),
            req.category_filter.clone(),
        )
        .await
        .handler_err("search discovery network")?;

    Ok(ApiResponse::success(DiscoveryNetworkSearchResponse {
        results,
    }))
}

/// Send a connection request to a pseudonym owner.
pub async fn connect(
    req: &ConnectRequest,
    discovery_url: &str,
    auth_token: &str,
    master_key: &[u8],
) -> HandlerResult<()> {
    let publisher = DiscoveryPublisher::new(
        master_key.to_vec(),
        discovery_url.to_string(),
        auth_token.to_string(),
    );

    publisher
        .connect(req.target_pseudonym, req.encrypted_blob.clone())
        .await
        .handler_err("send connection request")?;

    Ok(ApiResponse::success(()))
}

/// Poll for incoming connection requests.
pub async fn poll_requests(
    discovery_url: &str,
    auth_token: &str,
    master_key: &[u8],
) -> HandlerResult<DiscoveryConnectionsResponse> {
    let publisher = DiscoveryPublisher::new(
        master_key.to_vec(),
        discovery_url.to_string(),
        auth_token.to_string(),
    );

    let requests = publisher
        .poll_requests()
        .await
        .handler_err("poll connection requests")?;

    Ok(ApiResponse::success(DiscoveryConnectionsResponse {
        requests,
    }))
}

// === Interest Detection Handlers ===

/// Get the current interest profile.
pub async fn get_interests(node: &FoldNode) -> HandlerResult<InterestProfile> {
    let db = node
        .get_fold_db()
        .await
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    let profile = interests::load_interest_profile(&*store)
        .await
        .handler_err("load interest profile")?;

    match profile {
        Some(p) => Ok(ApiResponse::success(p)),
        None => Ok(ApiResponse::success(InterestProfile {
            categories: Vec::new(),
            total_embeddings_scanned: 0,
            unmatched_count: 0,
            detected_at: chrono::Utc::now(),
            seed_version: 0,
        })),
    }
}

/// Toggle an interest category's enabled flag.
pub async fn toggle_interest(
    req: &ToggleInterestRequest,
    node: &FoldNode,
) -> HandlerResult<InterestProfile> {
    let db = node
        .get_fold_db()
        .await
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    let profile = interests::toggle_interest_category(&*store, &req.category, req.enabled)
        .await
        .handler_err("toggle interest category")?;

    Ok(ApiResponse::success(profile))
}

/// Manually trigger interest detection.
pub async fn detect_interests(node: &FoldNode) -> HandlerResult<InterestProfile> {
    let db = node
        .get_fold_db()
        .await
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;

    let db_ops = db.get_db_ops();
    let metadata_store = db_ops.metadata_store().inner().clone();

    let native_index_mgr = db_ops
        .native_index_manager()
        .ok_or_else(|| HandlerError::Internal("Native index not available".to_string()))?;

    let embedding_store = native_index_mgr.store().clone();
    let embedder = native_index_mgr.embedder().clone();

    // Drop the DB lock before doing the heavy work
    drop(db);

    let profile = interests::detect_interests(&*embedding_store, &*metadata_store, &*embedder)
        .await
        .handler_err("detect interests")?;

    Ok(ApiResponse::success(profile))
}

/// Find similar profiles on the discovery network based on the user's interest fingerprint.
///
/// For each enabled interest category, uses the category centroid embedding to search
/// the discovery network, then aggregates results by pseudonym into profiles showing
/// match percentage (fraction of user's categories that overlap) and top similarity.
pub async fn similar_profiles(
    node: &FoldNode,
    discovery_url: &str,
    auth_token: &str,
    master_key: &[u8],
) -> HandlerResult<SimilarProfilesResponse> {
    let db = node
        .get_fold_db()
        .await
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;

    let db_ops = db.get_db_ops();
    let metadata_store = db_ops.metadata_store().inner().clone();

    let native_index_mgr = db_ops
        .native_index_manager()
        .ok_or_else(|| HandlerError::Internal("Native index not available".to_string()))?;

    let embedder = native_index_mgr.embedder().clone();

    // Load the user's interest profile
    let profile = interests::load_interest_profile(&*metadata_store)
        .await
        .handler_err("load interest profile")?;

    let enabled_categories: Vec<String> = match profile {
        Some(ref p) => p
            .categories
            .iter()
            .filter(|c| c.enabled)
            .map(|c| c.name.clone())
            .collect(),
        None => Vec::new(),
    };

    if enabled_categories.is_empty() {
        return Ok(ApiResponse::success(SimilarProfilesResponse {
            profiles: Vec::new(),
            user_categories_used: 0,
        }));
    }

    // Load centroids for each enabled category
    let centroids = interests::get_centroids(&*metadata_store, &*embedder)
        .await
        .handler_err("load interest centroids")?;

    // Drop the DB lock before network calls
    drop(db);

    let publisher = DiscoveryPublisher::new(
        master_key.to_vec(),
        discovery_url.to_string(),
        auth_token.to_string(),
    );

    // Search per enabled category using the centroid embedding, collect all results
    // pseudonym -> (set of categories, max similarity)
    let mut profile_map: HashMap<uuid::Uuid, (Vec<String>, f32)> = HashMap::new();
    let user_cat_count = enabled_categories.len();

    for cat_name in &enabled_categories {
        let centroid = match centroids.iter().find(|(name, _)| name == cat_name) {
            Some((_, emb)) => emb.clone(),
            None => continue,
        };

        let results = match publisher
            .search(centroid, 20, Some(cat_name.clone()))
            .await
        {
            Ok(r) => r,
            Err(e) => {
                log::warn!("Similar profiles search failed for category '{}': {}", cat_name, e);
                continue;
            }
        };

        for result in results {
            let entry = profile_map
                .entry(result.pseudonym)
                .or_insert_with(|| (Vec::new(), 0.0));
            if !entry.0.contains(cat_name) {
                entry.0.push(cat_name.clone());
            }
            if result.similarity > entry.1 {
                entry.1 = result.similarity;
            }
        }
    }

    // Convert to SimilarProfile list, sorted by match_percentage desc then top_similarity desc
    let mut profiles: Vec<SimilarProfile> = profile_map
        .into_iter()
        .map(|(pseudonym, (shared_categories, top_similarity))| {
            let match_percentage =
                (shared_categories.len() as f32 / user_cat_count as f32) * 100.0;
            SimilarProfile {
                pseudonym,
                match_percentage,
                shared_categories,
                top_similarity,
            }
        })
        .collect();

    profiles.sort_by(|a, b| {
        b.match_percentage
            .partial_cmp(&a.match_percentage)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                b.top_similarity
                    .partial_cmp(&a.top_similarity)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });

    // Cap at 20 profiles
    profiles.truncate(20);

    Ok(ApiResponse::success(SimilarProfilesResponse {
        profiles,
        user_categories_used: user_cat_count,
    }))
}
