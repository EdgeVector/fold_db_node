//! Shared Discovery Handlers
//!
//! Framework-agnostic handlers for discovery network operations.

use crate::discovery::config::{self, DiscoveryOptIn};
use crate::discovery::connection::{
    self, ConnectionPayload, DataSharePayload, IdentityCardPayload, LocalConnectionRequest,
    LocalSentRequest, ReferralQueryPayload, SharedRecord, SharedRecordKey,
};
use crate::discovery::interests::{self, InterestProfile};
use crate::discovery::publisher::{self as publisher, DiscoveryPublisher};
use crate::discovery::types::*;
pub use crate::discovery::types::{
    MomentHashReceiveRequest, MomentOptInRequest, MomentOptOutRequest, PhotoMetadata,
};
use crate::fold_node::node::FoldNode;
use crate::fold_node::OperationProcessor;
use crate::handlers::response::{
    ApiResponse, HandlerError, HandlerResult, IntoHandlerError, IntoTypedHandlerError,
};
use crate::trust::contact_book::{Contact, ContactBook, TrustDirection};
use crate::trust::identity_card::IdentityCard;
use crate::trust::sharing_roles::SharingRoleConfig;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[cfg(feature = "ts-bindings")]
use ts_rs::TS;

pub mod calendar;
pub mod faces;
pub mod inbound;
pub mod notifications;
pub mod photo_moments;
pub(crate) mod util;

pub use inbound::poll_and_decrypt_requests;
use util::{
    collect_our_pseudonyms, release_connect_sentinel, try_acquire_connect_sentinel,
    SentinelAcquire, CONNECT_IN_FLIGHT_TTL_SECS,
};

pub use calendar::{
    calendar_sharing_opt_in, calendar_sharing_opt_out, calendar_sharing_status, get_shared_events,
    store_peer_events, sync_calendar_events, CalendarEventInput, CalendarSharingStatusResponse,
    SharedEventsResponse, StorePeerEventsRequest, SyncCalendarEventsRequest,
    SyncCalendarEventsResponse,
};
pub use faces::{face_search, list_faces, FaceEntry, FaceSearchRequest, ListFacesResponse};
pub use notifications::{dismiss_notification, list_notifications, notification_count};
pub use photo_moments::{
    moment_detect, moment_list, moment_opt_in, moment_opt_in_list, moment_opt_out,
    moment_receive_hashes, moment_scan, MomentDetectResponse, MomentOptInListResponse,
    MomentScanResponse, SharedMomentsResponse,
};

/// Maximum number of results per search query.
pub(super) const MAX_TOP_K: usize = 100;
/// Maximum offset for paginated results.
const MAX_OFFSET: usize = 10_000;
/// Maximum number of photos in a single moment scan request.
pub(super) const MAX_PHOTO_BATCH: usize = 1_000;
/// Maximum number of calendar events in a single sync or peer-store request.
pub(super) const MAX_CALENDAR_BATCH: usize = 1_000;

// === Request types ===

#[derive(Debug, Clone, Deserialize)]
pub struct OptInRequest {
    pub schema_name: String,
    pub category: String,
    pub include_preview: Option<bool>,
    pub preview_max_chars: Option<usize>,
    pub preview_excluded_fields: Option<Vec<String>>,
    pub field_privacy: Option<
        std::collections::HashMap<String, crate::discovery::field_privacy::FieldPrivacyClass>,
    >,
    #[serde(default)]
    pub publish_faces: bool,
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
    pub offset: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConnectRequest {
    pub target_pseudonym: uuid::Uuid,
    pub message: String,
    /// Role the requester wants to assign the acceptor (default: "acquaintance")
    pub preferred_role: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RespondToRequestPayload {
    pub request_id: String,
    /// "accept" or "decline"
    pub action: String,
    pub message: Option<String>,
    /// Sharing role to assign on accept (defaults to "acquaintance")
    pub role: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToggleInterestRequest {
    pub category: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CheckNetworkRequest {
    pub request_id: String,
}

// === Data Sharing types ===

#[derive(Debug, Clone, Deserialize)]
pub struct DataShareRequest {
    /// Public key of the contact to share with (from contact book)
    pub recipient_public_key: String,
    /// Records to share (batch)
    pub records: Vec<DataShareRecordRequest>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DataShareRecordRequest {
    pub schema_name: String,
    /// The range key to look up the record
    pub record_key: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DataShareResponse {
    pub shared: usize,
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

/// Response for the browse-categories endpoint (re-exported from types).
pub use crate::discovery::types::BrowseCategoriesResponse;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(
    feature = "ts-bindings",
    ts(export, export_to = "src/fold_node/static-react/src/types/")
)]
pub struct ConnectionRequestsResponse {
    pub requests: Vec<LocalConnectionRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(
    feature = "ts-bindings",
    ts(export, export_to = "src/fold_node/static-react/src/types/")
)]
pub struct SentRequestsResponse {
    pub requests: Vec<LocalSentRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(
    feature = "ts-bindings",
    ts(export, export_to = "src/fold_node/static-react/src/types/")
)]
pub struct RespondToRequestResponse {
    pub request: LocalConnectionRequest,
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
pub(super) fn get_metadata_store(
    db: &fold_db::fold_db_core::FoldDB,
) -> std::sync::Arc<dyn fold_db::storage::traits::KvStore> {
    db.get_db_ops().raw_metadata_store()
}

/// List all discovery opt-in configs.
pub async fn list_opt_ins(node: &FoldNode) -> HandlerResult<DiscoveryOptInListResponse> {
    let db = node
        .get_fold_db()
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

    opt_in_config.publish_faces = req.publish_faces;

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
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    // Remove local config
    config::remove_opt_in(&*store, &req.schema_name)
        .await
        .handler_err("remove discovery opt-in")?;

    // Enumerate pseudonyms to delete from the discovery lambda.
    //
    // Primary path: read the `discovery:uploaded:{schema}:*` tracking
    // table, which is the authoritative record of what we uploaded,
    // complete even if the user has since deleted source embeddings
    // locally. This prevents zombie embeddings surviving opt-out.
    //
    // Fallback path: if the tracking table is empty for this schema —
    // e.g. a pre-existing user who published before this tracking
    // table existed — rederive from live `emb:` entries. This preserves
    // pre-existing behaviour for those users but does NOT reach
    // fragments whose source data has already been deleted locally.
    // Once they republish with this version installed, future opt-outs
    // use the tracking table and the leak is closed going forward.
    let discovery_publisher = DiscoveryPublisher::new(
        master_key.to_vec(),
        discovery_url.to_string(),
        auth_token.to_string(),
    );

    let tracked = publisher::list_uploaded_pseudonyms(&*store, Some(&req.schema_name))
        .await
        .map_err(HandlerError::Internal)?;

    let pseudonyms: Vec<uuid::Uuid> = if tracked.is_empty() {
        discovery_publisher
            .derive_schema_pseudonyms(&*store, &req.schema_name)
            .await
            .handler_err("derive pseudonyms for opt-out (fallback)")?
    } else {
        tracked
            .into_iter()
            .map(|(_schema, pseudo)| pseudo)
            .collect()
    };

    if !pseudonyms.is_empty() {
        // Send the delete to the lambda first. On failure, leave the
        // tracking table intact so a retry can finish the job — no
        // silent drops.
        discovery_publisher
            .unpublish_pseudonyms(pseudonyms)
            .await
            .handler_err("unpublish from discovery service")?;
        // Only clear local tracking after the lambda has confirmed
        // deletion. No-op when we came in via the fallback path.
        publisher::clear_uploaded(&*store, Some(&req.schema_name))
            .await
            .map_err(HandlerError::Internal)?;
    }

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
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;

    let db_ops = db.get_db_ops();
    let metadata_store = db_ops.raw_metadata_store();
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

    let disabled_categories = match interests::load_interest_profile(&*metadata_store).await {
        Ok(Some(profile)) => profile
            .categories
            .iter()
            .filter(|c| !c.enabled)
            .map(|c| c.name.clone())
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };

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
            .publish_schema(opt_in_config, &*embedding_store, &disabled_categories)
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

    let top_k = req.top_k.unwrap_or(20).min(MAX_TOP_K);
    let offset = req.offset.map(|o| o.min(MAX_OFFSET));

    // Use a lower similarity threshold (0.3) for text search. The Lambda default is 0.7
    // which is too high for text query embeddings against fragment embeddings — query text
    // and stored fragments often share topic but not exact wording.
    let results = publisher
        .search_with_threshold(
            query_embedding,
            top_k,
            req.category_filter.clone(),
            offset,
            Some(0.3),
            "text".to_string(),
        )
        .await
        .handler_err("search discovery network")?;

    Ok(ApiResponse::success(DiscoveryNetworkSearchResponse {
        results,
    }))
}

/// Send an E2E encrypted connection request to a pseudonym owner.
///
/// 1. Looks up the target pseudonym's published X25519 public key
/// 2. Picks a sender pseudonym from our published embeddings
/// 3. Encrypts the intro message with the target's public key
/// 4. Posts the encrypted blob to the bulletin board
/// 5. Saves the sent request locally for tracking
pub async fn connect(
    req: &ConnectRequest,
    node: &FoldNode,
    discovery_url: &str,
    auth_token: &str,
    master_key: &[u8],
) -> HandlerResult<()> {
    // Acquire a per-target sentinel in the Sled store BEFORE running the guard
    // checks. This closes the race where two concurrent POSTs for the same
    // target pseudonym (CLI + UI, two UI tabs, ...) both pass the contact-book
    // and sent-request guards and each post a duplicate connection request to
    // the bulletin board. KvStore has no put-if-absent / compare-and-swap
    // primitive, so we do get → check TTL → put. Narrower than true CAS but
    // sufficient: concurrency here is within a single fold_db node process.
    let target_str = req.target_pseudonym.to_string();
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {e}")))?;
    let store = get_metadata_store(&db);

    let now_ts = chrono::Utc::now().timestamp();
    match try_acquire_connect_sentinel(&*store, &target_str, now_ts, CONNECT_IN_FLIGHT_TTL_SECS)
        .await?
    {
        SentinelAcquire::InFlight => {
            return Err(HandlerError::BadRequest(
                "Connection request already in flight for this peer".to_string(),
            ));
        }
        SentinelAcquire::Acquired => {}
    }

    // Run the actual connect flow. ALWAYS release the sentinel on every exit
    // path (success or error). Using `let result = ...` then release, instead
    // of a Drop guard, because the release is async.
    let result = connect_inner(
        req,
        node,
        discovery_url,
        auth_token,
        master_key,
        &target_str,
    )
    .await;
    release_connect_sentinel(&*store, &target_str).await;
    result
}

async fn connect_inner(
    req: &ConnectRequest,
    node: &FoldNode,
    discovery_url: &str,
    auth_token: &str,
    master_key: &[u8],
    target_str: &str,
) -> HandlerResult<()> {
    // Check if already connected to this pseudonym
    let op = OperationProcessor::new(std::sync::Arc::new(node.clone()));
    let book_path = op
        .contact_book_path()
        .map_err(|e| HandlerError::Internal(format!("Failed to resolve contacts path: {e}")))?;
    let contact_book = ContactBook::load_from(&book_path).unwrap_or_default();
    if contact_book.active_contacts().iter().any(|c| {
        c.pseudonym.as_deref() == Some(target_str)
            || c.messaging_pseudonym.as_deref() == Some(target_str)
    }) {
        return Err(HandlerError::BadRequest(
            "Already connected to this peer".to_string(),
        ));
    }

    // Check if there's already a pending sent request to this pseudonym
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {e}")))?;
    let store = get_metadata_store(&db);
    let sent_requests = connection::list_sent_requests(&*store)
        .await
        .handler_err("list sent requests")?;
    if sent_requests
        .iter()
        .any(|r| r.target_pseudonym == target_str && r.status == "pending")
    {
        return Err(HandlerError::BadRequest(
            "Connection request already pending for this peer".to_string(),
        ));
    }

    let publisher = DiscoveryPublisher::new(
        master_key.to_vec(),
        discovery_url.to_string(),
        auth_token.to_string(),
    );

    // 1. Look up target's public key
    let target_pk_b64 = publisher
        .get_public_key(&req.target_pseudonym)
        .await
        .handler_err("look up target public key")?
        .ok_or_else(|| {
            HandlerError::NotFound(
                "Target pseudonym has no published public key. They may not have published yet."
                    .to_string(),
            )
        })?;

    let target_pk_bytes = B64.decode(&target_pk_b64).map_err(|e| {
        HandlerError::Internal(format!("Invalid target public key encoding: {}", e))
    })?;
    if target_pk_bytes.len() != 32 {
        return Err(HandlerError::Internal(
            "Target public key must be 32 bytes".to_string(),
        ));
    }
    let mut target_pk = [0u8; 32];
    target_pk.copy_from_slice(&target_pk_bytes);

    // 2. Pick a sender pseudonym — use first published pseudonym we have
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);
    let configs = config::list_opt_ins(&*store)
        .await
        .handler_err("list opt-ins")?;

    // Derive a stable sender pseudonym from our first opt-in config
    let sender_pseudonym = if let Some(first_config) = configs.first() {
        let hash = crate::discovery::pseudonym::content_hash(&first_config.schema_name);
        crate::discovery::pseudonym::derive_pseudonym(master_key, &hash)
    } else {
        // No configs — derive from master key alone
        let hash = crate::discovery::pseudonym::content_hash("connection-sender");
        crate::discovery::pseudonym::derive_pseudonym(master_key, &hash)
    };

    // 3. Build and encrypt the connection payload
    let sender_pk_b64 = connection::get_pseudonym_public_key_b64(master_key, &sender_pseudonym);

    // Collect our contacts' Ed25519 public keys for mutual-contact detection
    let network_keys: Vec<String> = contact_book
        .active_contacts()
        .iter()
        .map(|c| c.public_key.clone())
        .collect();

    // Generate the stable round-trip request_id up-front. Put it in the
    // outgoing payload AND in our LocalSentRequest so that when the acceptor
    // echoes it back in their "accept" we can unambiguously match the row
    // (important when the acceptor derives a single reply pseudonym).
    let stable_request_id = uuid::Uuid::new_v4().to_string();

    let our_identity_pseudonym =
        crate::discovery::pseudonym::derive_identity_pseudonym(master_key).to_string();

    let payload = ConnectionPayload {
        message_type: "request".to_string(),
        message: req.message.clone(),
        sender_public_key: sender_pk_b64.clone(),
        sender_pseudonym: sender_pseudonym.to_string(),
        reply_public_key: sender_pk_b64,
        identity_card: None,
        preferred_role: req.preferred_role.clone(),
        network_keys: if network_keys.is_empty() {
            None
        } else {
            Some(network_keys)
        },
        request_id: Some(stable_request_id.clone()),
        identity_pseudonym: Some(our_identity_pseudonym),
    };

    let encrypted = connection::encrypt_connection_message(&target_pk, &payload)
        .map_err(|e| HandlerError::Internal(format!("Encryption failed: {}", e)))?;

    let encrypted_b64 = B64.encode(&encrypted);

    // 4. Post to bulletin board
    publisher
        .connect(req.target_pseudonym, encrypted_b64, Some(sender_pseudonym))
        .await
        .handler_err("send connection request")?;

    // 5. Save sent request locally
    let sent = LocalSentRequest {
        request_id: stable_request_id,
        target_pseudonym: req.target_pseudonym.to_string(),
        sender_pseudonym: sender_pseudonym.to_string(),
        message: req.message.clone(),
        status: "pending".to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        preferred_role: req.preferred_role.clone(),
    };
    connection::save_sent_request(&*store, &sent)
        .await
        .handler_err("save sent request")?;

    Ok(ApiResponse::success(()))
}

/// List all pseudonyms this node currently publishes. Used by the E2E test
/// framework for cleanup.
pub async fn my_pseudonyms(node: &FoldNode, master_key: &[u8]) -> HandlerResult<serde_json::Value> {
    let pseudonyms = collect_our_pseudonyms(node, master_key).await?;
    Ok(ApiResponse::success(serde_json::json!({
        "pseudonyms": pseudonyms.iter().map(|p| p.to_string()).collect::<Vec<_>>(),
        "count": pseudonyms.len(),
    })))
}

/// Clear all discovery opt-ins. Used by the E2E test framework for cleanup.
pub async fn opt_out_all(node: &FoldNode) -> HandlerResult<serde_json::Value> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("{e}")))?;
    let store = get_metadata_store(&db);
    let configs = config::list_opt_ins(&*store)
        .await
        .handler_err("list opt-ins")?;
    let mut opted_out = 0usize;
    for cfg in &configs {
        if config::remove_opt_in(&*store, &cfg.schema_name)
            .await
            .is_ok()
        {
            opted_out += 1;
        }
    }
    Ok(ApiResponse::success(
        serde_json::json!({"opted_out": opted_out}),
    ))
}

/// Respond to a connection request (accept or decline).
pub async fn respond_to_request(
    req: &RespondToRequestPayload,
    node: &FoldNode,
    discovery_url: &str,
    auth_token: &str,
    master_key: &[u8],
) -> HandlerResult<RespondToRequestResponse> {
    if req.action != "accept" && req.action != "decline" {
        return Err(HandlerError::BadRequest(
            "action must be 'accept' or 'decline'".to_string(),
        ));
    }

    // If accepting, require identity card and validate role upfront
    let identity_card = if req.action == "accept" {
        let db = node
            .get_fold_db()
            .map_err(|e| HandlerError::Internal(format!("FoldDB not available: {e}")))?;
        let card = IdentityCard::load(&db)
            .await
            .map_err(|e| HandlerError::Internal(format!("Failed to load identity card: {e}")))?
            .ok_or_else(|| {
                HandlerError::BadRequest(
                    "Cannot accept connection: identity card not set up. Please set your display name first.".to_string(),
                )
            })?;
        Some(card)
    } else {
        None
    };

    let role_name = req.role.as_deref().unwrap_or("acquaintance");
    let op = OperationProcessor::new(std::sync::Arc::new(node.clone()));
    let roles_path = op
        .sharing_roles_path()
        .map_err(|e| HandlerError::Internal(format!("Failed to resolve roles path: {e}")))?;
    let config = SharingRoleConfig::load_from(&roles_path)
        .map_err(|e| HandlerError::Internal(format!("Failed to load roles: {e}")))?;

    if req.action == "accept" {
        config
            .get_role(role_name)
            .ok_or_else(|| HandlerError::BadRequest(format!("Unknown role: {role_name}")))?;
    }

    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    // Update local status
    let updated = connection::update_request_status(&*store, &req.request_id, &req.action)
        .await
        .handler_err("update request status")?;

    // If accepting: grant trust, create contact, send encrypted response
    if req.action == "accept" {
        let role = config.get_role(role_name).unwrap(); // validated above

        // Grant trust for the sender's public key
        op.grant_trust_for_domain(&updated.sender_public_key, &role.domain, role.tier)
            .await
            .map_err(|e: fold_db::schema::SchemaError| {
                HandlerError::Internal(format!("Failed to grant trust: {e}"))
            })?;

        // Create contact (direction = Incoming because they initiated the request)
        let contact = Contact::from_discovery(
            updated.sender_public_key.clone(),
            format!(
                "Discovery contact ({})",
                &updated.sender_pseudonym[..8.min(updated.sender_pseudonym.len())]
            ),
            None,
            TrustDirection::Incoming,
            Some(updated.sender_pseudonym.clone()),
            Some(updated.reply_public_key.clone()),
            updated.sender_identity_pseudonym.clone(),
            role.domain.clone(),
            role_name.to_string(),
        );
        let book_path = op
            .contact_book_path()
            .map_err(|e| HandlerError::Internal(format!("Failed to resolve contacts path: {e}")))?;
        let mut book = ContactBook::load_from(&book_path)
            .map_err(|e| HandlerError::Internal(format!("Failed to load contacts: {e}")))?;
        book.upsert_contact(contact);
        book.save_to(&book_path)
            .map_err(|e| HandlerError::Internal(format!("Failed to save contacts: {e}")))?;

        // Build and send encrypted acceptance message with identity card
        let reply_pk_bytes = B64
            .decode(&updated.reply_public_key)
            .map_err(|e| HandlerError::Internal(format!("Invalid reply public key: {}", e)))?;
        if reply_pk_bytes.len() != 32 {
            return Err(HandlerError::Internal(
                "Reply public key must be 32 bytes".to_string(),
            ));
        }
        let mut reply_pk = [0u8; 32];
        reply_pk.copy_from_slice(&reply_pk_bytes);

        let our_pseudonym: uuid::Uuid = updated
            .target_pseudonym
            .parse()
            .map_err(|_| HandlerError::Internal("Invalid target pseudonym UUID".to_string()))?;
        let our_pk_b64 = connection::get_pseudonym_public_key_b64(master_key, &our_pseudonym);

        let card = identity_card.unwrap(); // validated above
        let response_payload = ConnectionPayload {
            message_type: "accept".to_string(),
            message: req
                .message
                .clone()
                .unwrap_or_else(|| "Connection accepted".to_string()),
            sender_public_key: our_pk_b64.clone(),
            sender_pseudonym: updated.target_pseudonym.clone(),
            reply_public_key: our_pk_b64,
            identity_card: Some(IdentityCardPayload {
                display_name: card.display_name,
                contact_hint: card.contact_hint,
                node_public_key: node.get_node_public_key().to_string(),
            }),
            preferred_role: None, // accept messages don't carry a role preference
            network_keys: None,   // not needed in accept messages
            // Echo the requester's stable request_id so they can match the
            // accept to the exact LocalSentRequest row (see G3).
            request_id: updated.sender_request_id.clone(),
            // Carry our own stable identity pseudonym so the requester
            // can persist it on their contact row for us and use it as
            // the primary match key in future referral queries.
            identity_pseudonym: Some(
                crate::discovery::pseudonym::derive_identity_pseudonym(master_key).to_string(),
            ),
        };

        let encrypted = connection::encrypt_connection_message(&reply_pk, &response_payload)
            .map_err(|e| HandlerError::Internal(format!("Encryption failed: {}", e)))?;
        let encrypted_b64 = B64.encode(&encrypted);

        let sender_pseudonym: uuid::Uuid = updated
            .sender_pseudonym
            .parse()
            .map_err(|_| HandlerError::Internal("Invalid sender pseudonym UUID".to_string()))?;

        let publisher = DiscoveryPublisher::new(
            master_key.to_vec(),
            discovery_url.to_string(),
            auth_token.to_string(),
        );
        // Fire-and-forget: trust and contact are already created above.
        // If the messaging send fails (timeout, Lambda cold start), the
        // requester won't get the acceptance message but the trust relationship
        // is established. They can re-poll later.
        if let Err(e) = publisher
            .connect(sender_pseudonym, encrypted_b64, Some(our_pseudonym))
            .await
        {
            log::warn!(
                "Failed to send acceptance message (trust still created): {}",
                e
            );
        }
    }

    Ok(ApiResponse::success(RespondToRequestResponse {
        request: updated,
    }))
}

/// List locally stored sent connection requests.
pub async fn list_sent_requests(node: &FoldNode) -> HandlerResult<SentRequestsResponse> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    let requests = connection::list_sent_requests(&*store)
        .await
        .handler_err("list sent requests")?;

    Ok(ApiResponse::success(SentRequestsResponse { requests }))
}

/// Legacy: Poll for incoming connection requests (uses DynamoDB).
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

/// Browse categories available on the discovery network.
pub async fn browse_categories(
    discovery_url: &str,
    auth_token: &str,
    master_key: &[u8],
) -> HandlerResult<BrowseCategoriesResponse> {
    let publisher = DiscoveryPublisher::new(
        master_key.to_vec(),
        discovery_url.to_string(),
        auth_token.to_string(),
    );

    let categories = publisher
        .browse_categories()
        .await
        .handler_err("browse discovery categories")?;

    Ok(ApiResponse::success(BrowseCategoriesResponse {
        categories,
    }))
}

// === Interest Detection Handlers ===

/// Get the current interest profile.
pub async fn get_interests(node: &FoldNode) -> HandlerResult<InterestProfile> {
    let db = node
        .get_fold_db()
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
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;

    let db_ops = db.get_db_ops();
    let metadata_store = db_ops.raw_metadata_store();

    // Get all schemas and extract their field_interest_categories
    let schemas: Vec<_> = db
        .schema_manager()
        .get_schemas()
        .typed_handler_err()?
        .into_values()
        .collect();

    // Drop the DB lock before doing the work
    drop(db);

    let profile = interests::detect_interests_from_schemas(&schemas, &*metadata_store)
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
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;

    let db_ops = db.get_db_ops();
    let metadata_store = db_ops.raw_metadata_store();

    let native_index_mgr = db_ops
        .native_index_manager()
        .ok_or_else(|| HandlerError::Internal("Native index not available".to_string()))?;

    let embedder = native_index_mgr.embedder().clone();

    // Load the user's interest profile
    let profile = interests::load_interest_profile(&*metadata_store)
        .await
        .handler_err("load interest profile")?;

    let mut enabled_categories: Vec<String> = match profile {
        Some(ref p) => p
            .categories
            .iter()
            .filter(|c| c.enabled)
            .map(|c| c.name.clone())
            .collect(),
        None => Vec::new(),
    };

    // Fallback: if no interest profile exists, use the discovery opt-in categories.
    // Users who opted schemas into discovery should see similar profiles without
    // needing to run a separate interest-detection step.
    if enabled_categories.is_empty() {
        let configs = crate::discovery::config::list_opt_ins(&*metadata_store)
            .await
            .map_err(|e| HandlerError::Internal(format!("load opt-in configs: {}", e)))?;
        let mut seen = std::collections::HashSet::new();
        for c in &configs {
            if seen.insert(c.category.clone()) {
                enabled_categories.push(c.category.clone());
            }
        }
    }

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
            .search_with_threshold(
                centroid,
                20,
                Some(cat_name.clone()),
                None,
                Some(0.15),
                "text".to_string(),
            )
            .await
        {
            Ok(r) => r,
            Err(e) => {
                log::warn!(
                    "Similar profiles search failed for category '{}': {}",
                    cat_name,
                    e
                );
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
            let match_percentage = (shared_categories.len() as f32 / user_cat_count as f32) * 100.0;
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

// === Calendar Sharing Handlers ===
// ===== Data Sharing =====

/// Send records to a contact via the encrypted bulletin board.
///
/// Loads each record's schema definition and field values, optionally includes
/// file data (base64-encoded), encrypts the batch with the contact's messaging
/// public key, and posts to the bulletin board.
pub async fn send_data_share(
    req: &DataShareRequest,
    node: &FoldNode,
    discovery_url: &str,
    auth_token: &str,
    master_key: &[u8],
) -> HandlerResult<DataShareResponse> {
    // 1. Look up recipient in contact book
    let op = OperationProcessor::new(std::sync::Arc::new(node.clone()));
    let book_path = op
        .contact_book_path()
        .map_err(|e| HandlerError::Internal(format!("Failed to resolve contacts path: {e}")))?;
    let book = ContactBook::load_from(&book_path)
        .map_err(|e| HandlerError::Internal(format!("Failed to load contacts: {e}")))?;

    let contact = book.get(&req.recipient_public_key).ok_or_else(|| {
        HandlerError::NotFound(format!(
            "Contact not found for public key: {}",
            &req.recipient_public_key
        ))
    })?;

    if contact.revoked {
        return Err(HandlerError::BadRequest(
            "Cannot share data with a revoked contact".to_string(),
        ));
    }

    let messaging_pk_b64 = contact.messaging_public_key.as_ref().ok_or_else(|| {
        HandlerError::BadRequest(
            "Contact has no messaging public key. They may not have been connected via discovery."
                .to_string(),
        )
    })?;

    let messaging_pseudonym = contact.messaging_pseudonym.as_ref().ok_or_else(|| {
        HandlerError::BadRequest(
            "Contact has no messaging pseudonym. Cannot send bulletin board messages.".to_string(),
        )
    })?;

    let target_pseudonym: uuid::Uuid = messaging_pseudonym.parse().map_err(|_| {
        HandlerError::Internal("Invalid messaging pseudonym UUID in contact".to_string())
    })?;

    let messaging_pk_bytes = B64.decode(messaging_pk_b64).map_err(|e| {
        HandlerError::Internal(format!("Invalid messaging public key encoding: {}", e))
    })?;
    if messaging_pk_bytes.len() != 32 {
        return Err(HandlerError::Internal(
            "Messaging public key must be 32 bytes".to_string(),
        ));
    }
    let mut target_pk = [0u8; 32];
    target_pk.copy_from_slice(&messaging_pk_bytes);

    // 2. Get sender identity
    let sender_public_key = node.get_node_public_key().to_string();
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let sender_display_name = IdentityCard::load(&db)
        .await
        .ok()
        .flatten()
        .map(|c| c.display_name)
        .unwrap_or_else(|| "Unknown".to_string());

    // 3. Load each record

    let mut shared_records = Vec::with_capacity(req.records.len());

    // Sender is the record owner sharing their own data — owner context.
    let owner_ctx = fold_db::access::AccessContext::owner(sender_public_key.clone());

    for record_req in &req.records {
        // Load schema definition
        let schema_def = match db
            .schema_manager()
            .get_schema_metadata(&record_req.schema_name)
        {
            Ok(Some(schema)) => serde_json::to_value(&schema).ok(),
            _ => None,
        };

        // Query the record
        let query = fold_db::schema::types::operations::Query::new(
            record_req.schema_name.clone(),
            vec![], // all fields
        );
        let result_map = db
            .query_executor()
            .query_with_access(query, &owner_ctx, None)
            .await;
        let records_map = match result_map {
            Ok(rm) => fold_db::fold_db_core::query::records_from_field_map(&rm),
            Err(e) => {
                log::warn!(
                    "Failed to query records for schema '{}': {}",
                    record_req.schema_name,
                    e
                );
                continue;
            }
        };

        // Find the matching record by key
        let matching_record = records_map.iter().find(|(key, _)| {
            key.range
                .as_deref()
                .map(|r| r == record_req.record_key)
                .unwrap_or(false)
                || key
                    .hash
                    .as_deref()
                    .map(|h| h == record_req.record_key)
                    .unwrap_or(false)
        });

        let (key, record) = match matching_record {
            Some((k, r)) => (k, r),
            None => {
                log::warn!(
                    "Record not found for key '{}' in schema '{}'",
                    record_req.record_key,
                    record_req.schema_name
                );
                continue;
            }
        };

        // Convert fields to HashMap<String, Value>
        let fields: HashMap<String, serde_json::Value> = record
            .fields
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        // Check for file data (look for file_hash or source_file_name in fields)
        let file_hash = fields
            .get("file_hash")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let file_name = fields
            .get("source_file_name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let file_data_base64 = if let Some(ref hash) = file_hash {
            // Try to read the file from upload storage
            let upload_path = std::env::var("FOLDDB_UPLOAD_PATH")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("data/uploads"));
            let upload_storage = fold_db::storage::UploadStorage::local(upload_path);
            match upload_storage.read_file(hash, None).await {
                Ok(bytes) => Some(B64.encode(&bytes)),
                Err(e) => {
                    log::debug!("Could not read file '{}' for sharing: {}", hash, e);
                    None
                }
            }
        } else {
            None
        };

        shared_records.push(SharedRecord {
            schema_name: record_req.schema_name.clone(),
            schema_definition: schema_def,
            fields,
            key: SharedRecordKey {
                hash: key.hash.clone(),
                range: key.range.clone(),
            },
            file_data_base64,
            file_name,
        });
    }

    // Release DB lock before network call
    drop(db);

    let shared_count = shared_records.len();

    if shared_records.is_empty() {
        return Err(HandlerError::BadRequest(
            "No records found to share".to_string(),
        ));
    }

    // 4. Build the data share payload
    let payload = DataSharePayload {
        message_type: "data_share".to_string(),
        sender_public_key,
        sender_display_name,
        records: shared_records,
    };

    // 5. Encrypt with recipient's messaging public key
    let encrypted = connection::encrypt_message(&target_pk, &payload)
        .map_err(|e| HandlerError::Internal(format!("Encryption failed: {}", e)))?;

    let encrypted_b64 = B64.encode(&encrypted);

    // 6. Post to bulletin board
    let sender_pseudonym = {
        let hash = crate::discovery::pseudonym::content_hash("connection-sender");
        crate::discovery::pseudonym::derive_pseudonym(master_key, &hash)
    };

    let publisher = DiscoveryPublisher::new(
        master_key.to_vec(),
        discovery_url.to_string(),
        auth_token.to_string(),
    );

    publisher
        .connect(target_pseudonym, encrypted_b64, Some(sender_pseudonym))
        .await
        .handler_err("send data share message")?;

    log::info!(
        "Shared {} records with contact (pseudonym {})",
        shared_count,
        target_pseudonym
    );

    Ok(ApiResponse::success(DataShareResponse {
        shared: shared_count,
    }))
}

// ===== Referral query handlers =====

/// Initiate a referral query: ask trusted contacts if they know the sender
/// of a pending connection request.
pub async fn initiate_referral_query(
    req: &CheckNetworkRequest,
    node: &FoldNode,
    discovery_url: &str,
    auth_token: &str,
    master_key: &[u8],
) -> HandlerResult<serde_json::Value> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {e}")))?;
    let store = get_metadata_store(&db);

    // Find the connection request by scanning prefix
    let entries = store
        .scan_prefix(b"discovery:conn_req:")
        .await
        .handler_err("scan connection requests")?;

    let mut found_key: Option<Vec<u8>> = None;
    let mut found_req: Option<LocalConnectionRequest> = None;
    for (key, value) in &entries {
        if let Ok(local_req) = serde_json::from_slice::<LocalConnectionRequest>(value) {
            if local_req.request_id == req.request_id {
                found_key = Some(key.clone());
                found_req = Some(local_req);
                break;
            }
        }
    }

    let (sled_key, mut local_req) = match (found_key, found_req) {
        (Some(k), Some(r)) => (k, r),
        _ => {
            return Err(HandlerError::NotFound(format!(
                "Connection request {} not found",
                req.request_id
            )));
        }
    };

    if local_req.status != "pending" {
        return Err(HandlerError::BadRequest(
            "Can only check network for pending requests".to_string(),
        ));
    }

    if local_req.referral_query_id.is_some() {
        return Err(HandlerError::BadRequest(
            "Referral query already sent for this request".to_string(),
        ));
    }

    // Load contact book
    let op = OperationProcessor::new(std::sync::Arc::new(node.clone()));
    let book_path = op
        .contact_book_path()
        .map_err(|e| HandlerError::Internal(format!("Failed to resolve contacts path: {e}")))?;
    let contact_book = ContactBook::load_from(&book_path).unwrap_or_default();

    // Filter to contacts with both messaging pseudonym and public key
    let eligible: Vec<Contact> = contact_book
        .active_contacts()
        .into_iter()
        .filter(|c| c.messaging_pseudonym.is_some() && c.messaging_public_key.is_some())
        .cloned()
        .collect();

    if eligible.is_empty() {
        return Ok(ApiResponse::success(serde_json::json!({
            "query_id": null,
            "contacts_queried": 0
        })));
    }

    let query_id = uuid::Uuid::new_v4().to_string();

    // Derive our connection-sender pseudonym
    let sender_hash = crate::discovery::pseudonym::content_hash("connection-sender");
    let our_pseudonym = crate::discovery::pseudonym::derive_pseudonym(master_key, &sender_hash);
    let our_pk_b64 = connection::get_pseudonym_public_key_b64(master_key, &our_pseudonym);

    let publisher = DiscoveryPublisher::new(
        master_key.to_vec(),
        discovery_url.to_string(),
        auth_token.to_string(),
    );

    let mut queried_count: u32 = 0;

    for contact in &eligible {
        let messaging_pseudonym_str = contact.messaging_pseudonym.as_ref().unwrap();
        let messaging_pk_b64 = contact.messaging_public_key.as_ref().unwrap();

        let target_pseudonym: uuid::Uuid = match messaging_pseudonym_str.parse() {
            Ok(u) => u,
            Err(e) => {
                log::warn!(
                    "Invalid messaging pseudonym UUID for contact {}: {}",
                    contact.display_name,
                    e
                );
                continue;
            }
        };

        let pk_bytes = match B64.decode(messaging_pk_b64) {
            Ok(b) if b.len() == 32 => {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&b);
                arr
            }
            Ok(_) => {
                log::warn!(
                    "Messaging public key wrong length for contact {}",
                    contact.display_name
                );
                continue;
            }
            Err(e) => {
                log::warn!(
                    "Invalid messaging public key for contact {}: {}",
                    contact.display_name,
                    e
                );
                continue;
            }
        };

        let payload = ReferralQueryPayload {
            message_type: "referral_query".to_string(),
            query_id: query_id.clone(),
            subject_pseudonym: local_req.sender_pseudonym.clone(),
            subject_public_key: local_req.sender_public_key.clone(),
            sender_pseudonym: our_pseudonym.to_string(),
            reply_public_key: our_pk_b64.clone(),
            // Primary match key: the subject's stable identity pseudonym
            // carried on the original incoming `ConnectionPayload`. When
            // `None` (legacy sender), handlers fall back to the
            // pseudonym/public-key match.
            subject_identity_pseudonym: local_req.sender_identity_pseudonym.clone(),
        };

        let encrypted = match connection::encrypt_message(&pk_bytes, &payload) {
            Ok(e) => e,
            Err(e) => {
                log::warn!(
                    "Failed to encrypt referral query for {}: {}",
                    contact.display_name,
                    e
                );
                continue;
            }
        };

        let encrypted_b64 = B64.encode(&encrypted);

        if let Err(e) = publisher
            .connect(target_pseudonym, encrypted_b64, Some(our_pseudonym))
            .await
        {
            log::warn!(
                "Failed to send referral query to {}: {}",
                contact.display_name,
                e
            );
            continue;
        }

        queried_count += 1;
    }

    // Update the connection request with referral info
    local_req.referral_query_id = Some(query_id.clone());
    local_req.referral_contacts_queried = queried_count;

    let updated = serde_json::to_vec(&local_req)
        .map_err(|e| HandlerError::Internal(format!("Failed to serialize request: {e}")))?;
    store
        .put(&sled_key, updated)
        .await
        .handler_err("save updated connection request")?;

    Ok(ApiResponse::success(serde_json::json!({
        "query_id": query_id,
        "contacts_queried": queried_count
    })))
}
