//! Shared Discovery Handlers
//!
//! Framework-agnostic handlers for discovery network operations.

use crate::discovery::async_query::{
    self, QueryRequestPayload, QueryResponsePayload, SchemaInfo, SchemaListRequestPayload,
    SchemaListResponsePayload,
};
use crate::discovery::config::{self, DiscoveryOptIn};
use crate::discovery::connection::{
    self, ConnectionPayload, DataSharePayload, IdentityCardPayload, LocalConnectionRequest,
    LocalSentRequest, MutualContact, ReferralQueryPayload, ReferralResponsePayload, SharedRecord,
    SharedRecordKey, Vouch,
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
pub mod notifications;
pub mod photo_moments;

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

// Sled key prefix for per-target "connect in flight" sentinels.
//
// Used by the connect handler to serialize concurrent POSTs to the
// `/api/discovery/connect` endpoint targeting the same pseudonym (CLI + UI,
// two UI tabs, etc.).
const CONNECT_IN_FLIGHT_PREFIX: &str = "discovery:connect_in_flight:";

// TTL on the in-flight sentinel. If the guarded flow dies mid-way (crash,
// panic, dropped future) without releasing, the next attempt after this many
// seconds will treat the sentinel as stale and overwrite it. Long enough to
// cover a network round trip, short enough that a legitimate retry isn't
// blocked.
const CONNECT_IN_FLIGHT_TTL_SECS: i64 = 60;

/// Outcome of attempting to acquire the per-target connect sentinel.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SentinelAcquire {
    /// Sentinel acquired (either fresh or stale-overwritten). Caller owns it
    /// and must release on every exit path.
    Acquired,
    /// Another fresh sentinel already exists — caller must reject the request.
    InFlight,
}

/// Try to acquire the per-target in-flight sentinel in the Sled store.
/// See `connect` for rationale. Pure helper, unit-testable.
pub(crate) async fn try_acquire_connect_sentinel(
    store: &dyn fold_db::storage::traits::KvStore,
    target_pseudonym: &str,
    now_ts: i64,
    ttl_secs: i64,
) -> Result<SentinelAcquire, HandlerError> {
    let key = format!("{}{}", CONNECT_IN_FLIGHT_PREFIX, target_pseudonym);
    if let Some(existing) = store
        .get(key.as_bytes())
        .await
        .map_err(|e| HandlerError::Internal(format!("sentinel read: {e}")))?
    {
        let existing_ts = std::str::from_utf8(&existing)
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
            .ok_or_else(|| {
                HandlerError::Internal("corrupt connect_in_flight sentinel value".to_string())
            })?;
        if now_ts - existing_ts < ttl_secs {
            return Ok(SentinelAcquire::InFlight);
        }
        // Stale: previous attempt died mid-flight. Fall through and overwrite.
    }
    store
        .put(key.as_bytes(), now_ts.to_string().into_bytes())
        .await
        .map_err(|e| HandlerError::Internal(format!("sentinel write: {e}")))?;
    Ok(SentinelAcquire::Acquired)
}

/// Release the per-target in-flight sentinel. Best-effort — a release failure
/// is logged but does not mask the caller's primary result, because the
/// sentinel self-expires after `CONNECT_IN_FLIGHT_TTL_SECS`.
pub(crate) async fn release_connect_sentinel(
    store: &dyn fold_db::storage::traits::KvStore,
    target_pseudonym: &str,
) {
    let key = format!("{}{}", CONNECT_IN_FLIGHT_PREFIX, target_pseudonym);
    if let Err(e) = store.delete(key.as_bytes()).await {
        log::warn!(
            "Failed to release connect_in_flight sentinel for {}: {}",
            target_pseudonym,
            e
        );
    }
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
    let op = OperationProcessor::new(node.clone());
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

/// Collect all pseudonyms this node publishes. Used by the request poller and by
/// the `my_pseudonyms` handler (test-framework cleanup). The derivation must stay
/// in sync with `publisher.rs`.
pub(crate) async fn collect_our_pseudonyms(
    node: &FoldNode,
    master_key: &[u8],
) -> Result<Vec<uuid::Uuid>, HandlerError> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let db_ops = db.get_db_ops();
    let store = get_metadata_store(&db);

    let configs = config::list_opt_ins(&*store)
        .await
        .handler_err("list opt-ins")?;

    let mut pseudonyms = Vec::new();

    // Add our connection-sender pseudonym (fallback used by connect handler when no opt-ins)
    let hash = crate::discovery::pseudonym::content_hash("connection-sender");
    pseudonyms.push(crate::discovery::pseudonym::derive_pseudonym(
        master_key, &hash,
    ));

    // Add our schema-name-derived sender pseudonyms. The connect handler uses
    // derive_pseudonym(master_key, content_hash(first_opt_in.schema_name)) as
    // sender_pseudonym. When someone replies or shares data to that pseudonym,
    // we need to poll for it. Without this, data shares never reach their target.
    for cfg in &configs {
        let schema_hash = crate::discovery::pseudonym::content_hash(&cfg.schema_name);
        pseudonyms.push(crate::discovery::pseudonym::derive_pseudonym(
            master_key,
            &schema_hash,
        ));
    }

    // Add pseudonyms derived from actual published embeddings (same as publisher.rs)
    let native_index_mgr = db_ops.native_index_manager();
    if let Some(nim) = native_index_mgr {
        let embedding_store = nim.store().clone();
        for cfg in &configs {
            let prefix = format!("emb:{}:", cfg.schema_name);
            if let Ok(raw_entries) = embedding_store.scan_prefix(prefix.as_bytes()).await {
                for (_key, value) in &raw_entries {
                    if let Ok(stored) = serde_json::from_slice::<serde_json::Value>(value) {
                        if let Some(emb_arr) = stored.get("embedding").and_then(|e| e.as_array()) {
                            let embedding_bytes: Vec<u8> = emb_arr
                                .iter()
                                .filter_map(|v| v.as_f64().map(|f| f as f32))
                                .flat_map(|f| f.to_le_bytes())
                                .collect();
                            let content_hash =
                                crate::discovery::pseudonym::content_hash_bytes(&embedding_bytes);
                            pseudonyms.push(crate::discovery::pseudonym::derive_pseudonym(
                                master_key,
                                &content_hash,
                            ));
                        }
                    }
                }
            }
        }
    }

    pseudonyms.sort();
    pseudonyms.dedup();
    // NOTE: previously truncated to 1000 here with a "URL length limit" comment,
    // but that was misleading — the poll request only sends pseudonyms to the
    // server when `our_pseudonyms.len() <= 100` (otherwise it passes None and
    // filters client-side). The truncate silently dropped decrypt keys beyond
    // the 1000th, causing addressed messages to be missed. No cap needed:
    // pseudonyms are 16 bytes each, and the server-filter branch already guards
    // URL length.
    log::debug!(
        "our_pseudonyms[0..5]: {:?}",
        pseudonyms
            .iter()
            .take(5)
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
    );
    Ok(pseudonyms)
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

// === Dedup marker prune (G2) =====================================================
//
// Every processed bulletin-board message leaves a `msg_processed:{id}` marker so
// we don't re-dispatch it on subsequent polls. Without a bound the key-space grows
// forever. We prune entries older than `DEDUP_RETENTION_SECS` every
// `PRUNE_EVERY_N_POLLS` invocations of `poll_and_decrypt_requests`.
//
// The marker value is an 8-byte little-endian u64 seconds timestamp. The older
// marker format was `b"1"`; malformed/short values are treated as stale (age = 0
// at deploy, then immediately older than retention on subsequent prunes) and are
// deleted on the next prune pass.

const MSG_PROCESSED_PREFIX: &str = "msg_processed:";
/// Retain dedup markers for 7 days. Bulletin-board messages in DynamoDB have a
/// shorter TTL anyway; a reappearing 7-day-old message is safe to re-dispatch.
const DEDUP_RETENTION_SECS: u64 = 7 * 24 * 60 * 60;
/// Run the prune scan every N-th call. A simple in-memory atomic counter avoids
/// a separate background task.
const PRUNE_EVERY_N_POLLS: u64 = 50;

static PRUNE_POLL_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn encode_marker_timestamp(secs: u64) -> Vec<u8> {
    secs.to_le_bytes().to_vec()
}

/// Decode a dedup marker value into a wall-clock seconds timestamp.
/// Returns `None` for legacy/malformed markers (pre-G2 wrote `b"1"`).
fn decode_marker_timestamp(value: &[u8]) -> Option<u64> {
    if value.len() != 8 {
        return None;
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(value);
    Some(u64::from_le_bytes(buf))
}

/// Delete dedup markers older than `DEDUP_RETENTION_SECS`. Legacy markers
/// (non-8-byte values written before this fix) are also deleted — they're
/// known to be at most ≤7 days old at deploy time and are safe to drop.
///
/// Uses the `KvStore::scan_prefix` method which loads all matching entries
/// at once (no streaming API exists on the trait). The marker key-space is
/// bounded by recent bulletin-board traffic, so a full load is fine.
pub(crate) async fn prune_msg_processed_markers(
    store: &dyn fold_db::storage::traits::KvStore,
    now: u64,
    retention_secs: u64,
) -> Result<usize, String> {
    let entries = store
        .scan_prefix(MSG_PROCESSED_PREFIX.as_bytes())
        .await
        .map_err(|e| format!("scan_prefix failed: {e}"))?;

    let mut deleted = 0usize;
    for (key, value) in entries {
        let stale = match decode_marker_timestamp(&value) {
            Some(ts) => now.saturating_sub(ts) > retention_secs,
            // Legacy/malformed marker — drop it.
            None => true,
        };
        if stale {
            store
                .delete(&key)
                .await
                .map_err(|e| format!("delete failed: {e}"))?;
            deleted += 1;
        }
    }
    Ok(deleted)
}

/// Poll the bulletin board, decrypt messages for our pseudonyms, and store locally.
pub async fn poll_and_decrypt_requests(
    node: &FoldNode,
    discovery_url: &str,
    auth_token: &str,
    master_key: &[u8],
) -> HandlerResult<ConnectionRequestsResponse> {
    let publisher = DiscoveryPublisher::new(
        master_key.to_vec(),
        discovery_url.to_string(),
        auth_token.to_string(),
    );

    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    // Get our published pseudonyms via the shared helper — same derivation as the
    // publisher uses when uploading: derive(master_key, SHA256(embedding_bytes)).
    let our_pseudonyms: Vec<uuid::Uuid> = collect_our_pseudonyms(node, master_key).await?;

    if our_pseudonyms.is_empty() {
        return Ok(ApiResponse::success(ConnectionRequestsResponse {
            requests: Vec::new(),
        }));
    }

    // Poll messages in chunks of ≤100 pseudonyms. The messaging service requires a
    // pseudonym filter (returns empty if None), so we must always send a filter.
    // For large pseudonym sets, split into multiple polls and merge results.
    const POLL_CHUNK_SIZE: usize = 100;
    let mut messages = Vec::new();
    for chunk in our_pseudonyms.chunks(POLL_CHUNK_SIZE) {
        let chunk_messages = publisher
            .poll_messages(None, Some(chunk))
            .await
            .handler_err("poll messages")?;
        messages.extend(chunk_messages);
    }
    log::info!(
        "Polled {} pseudonyms in {} chunks, got {} messages",
        our_pseudonyms.len(),
        our_pseudonyms.len().div_ceil(POLL_CHUNK_SIZE),
        messages.len()
    );

    // Try to decrypt and dispatch each message. The loop enforces the invariant
    // that a message is marked "processed" in the dedup store ONLY after the
    // dispatch handler reports either successful handling (`Handled`) or a
    // permanent parse/unknown failure (`Skipped`). Transient errors (store
    // hiccups, failed persistence) propagate as `Err` and leave the dedup marker
    // absent, so the next poll retries the same message.
    //
    // CLAUDE.md "no silent failures": we no longer swallow dispatch errors with
    // `log::warn!; continue;`. Errors are collected and logged at the end, and
    // the dedup store write itself is checked (no `let _ = ...`).
    let mut dispatch_errors: Vec<(String, HandlerError)> = Vec::new();
    for msg in &messages {
        let target: uuid::Uuid = match msg.target_pseudonym.parse() {
            Ok(u) => u,
            // Not addressable — cannot possibly be for us. Skip without dedup
            // marker (cheap to re-skip next poll; avoids polluting the store).
            Err(_) => continue,
        };

        // Derive the secret key for this pseudonym
        let (secret, _) = connection::derive_pseudonym_keypair(master_key, &target);

        let encrypted_bytes = match B64.decode(&msg.encrypted_blob) {
            Ok(b) => b,
            Err(_) => continue,
        };

        let raw = match connection::decrypt_message_raw(&secret, &encrypted_bytes) {
            Ok(v) => v,
            Err(e) => {
                log::debug!(
                    "Failed to decrypt message {} for target {}: {}",
                    msg.message_id,
                    target,
                    e
                );
                // Not for us, corrupted, or from a different sender key. We
                // intentionally do NOT mark this as processed: decrypt cost is
                // low and the bulletin board expires messages on its own.
                continue;
            }
        };

        // De-duplication: check if we already processed this message.
        // Any present value counts as "processed"; the stored value is a
        // wall-clock timestamp (little-endian u64 seconds) used only by the
        // periodic prune of stale markers.
        let dedup_key = format!("{}{}", MSG_PROCESSED_PREFIX, msg.message_id);
        match store.get(dedup_key.as_bytes()).await {
            Ok(Some(_)) => continue,
            Ok(None) => {}
            Err(e) => {
                // Store read failure — treat as transient, do not dispatch.
                dispatch_errors.push((
                    msg.message_id.clone(),
                    HandlerError::Internal(format!("dedup read failed: {e}")),
                ));
                continue;
            }
        }

        // Dispatch. The helper returns Handled/Skipped to indicate the message
        // should be marked processed, or Err to indicate a transient failure
        // that must be retried on the next poll.
        let outcome =
            dispatch_decrypted_message(node, &*store, master_key, &publisher, msg, raw).await;

        match outcome {
            Ok(DispatchOutcome::Handled) | Ok(DispatchOutcome::Skipped { .. }) => {
                if let Ok(DispatchOutcome::Skipped { ref reason }) = outcome {
                    log::warn!(
                        "Permanently skipping message {}: {}",
                        msg.message_id,
                        reason
                    );
                }
                // Only mark as processed after the handler reports a terminal
                // outcome. Propagate errors from the dedup write — a silent
                // failure here would cause the message to re-dispatch forever.
                // Store the write timestamp so the periodic prune can expire
                // stale markers (see G2 prune module above).
                if let Err(e) = store
                    .put(dedup_key.as_bytes(), encode_marker_timestamp(now_secs()))
                    .await
                {
                    dispatch_errors.push((
                        msg.message_id.clone(),
                        HandlerError::Internal(format!("dedup write failed: {e}")),
                    ));
                }
            }
            Err(e) => {
                log::error!(
                    "Transient dispatch failure for message {} (will retry next poll): {}",
                    msg.message_id,
                    e
                );
                dispatch_errors.push((msg.message_id.clone(), e));
            }
        }
    }

    if !dispatch_errors.is_empty() {
        log::error!(
            "poll_and_decrypt_requests: {} message(s) failed dispatch and will be retried on the next poll",
            dispatch_errors.len()
        );
    }

    // Prune stale dedup markers every N-th poll. Done after the dispatch loop
    // so we never delete a marker we're about to check in the same iteration.
    let prune_count = PRUNE_POLL_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if prune_count.is_multiple_of(PRUNE_EVERY_N_POLLS) {
        match prune_msg_processed_markers(&*store, now_secs(), DEDUP_RETENTION_SECS).await {
            Ok(deleted) => {
                if deleted > 0 {
                    log::info!("Pruned {} stale msg_processed markers", deleted);
                }
            }
            Err(e) => log::error!("Failed to prune msg_processed markers: {}", e),
        }
    }

    // Return all locally stored received requests
    let requests = connection::list_received_requests(&*store)
        .await
        .handler_err("list received requests")?;

    Ok(ApiResponse::success(ConnectionRequestsResponse {
        requests,
    }))
}

/// Outcome of dispatching a single decrypted bulletin-board message.
///
/// Both `Handled` and `Skipped` cause the caller to write the dedup marker.
/// `Err(HandlerError)` (returned separately) means the dispatch should be
/// retried on the next poll and the dedup marker must NOT be written.
#[derive(Debug)]
enum DispatchOutcome {
    /// Dispatch succeeded.
    Handled,
    /// Message is permanently unprocessable (bad/unknown payload). The caller
    /// writes the dedup marker so we don't re-dispatch garbage on every poll.
    Skipped { reason: String },
}

/// Dispatch a single decrypted message to the appropriate type-specific
/// handler. This replaces the prior "log warn and continue" pattern that
/// silently lost data when a transient error occurred between marking a
/// message processed and actually persisting it.
///
/// Classification:
/// - Transient (returns `Err`, caller retries): `save_received_request`,
///   `update_sent_request_status`, `process_accepted_connection`,
///   `process_data_share`. These can fail on a transient Sled hiccup or a
///   missing-but-recoverable prerequisite, and must be retried.
/// - Permanent (returns `Ok(Skipped)`, caller marks processed): payload
///   deserialization failure, unknown `message_type`. Retrying forever would
///   spam logs without changing the outcome.
/// - Best-effort async helpers (`handle_incoming_query`, `..._query_response`,
///   `..._schema_list_*`, `..._referral_*`): these already log their internal
///   failures and have no return signal; they're treated as `Ok(Handled)` once
///   the payload parses. Refactoring them to return `Result` is out of scope
///   for this fix and would explode the diff.
#[allow(clippy::too_many_lines)]
async fn dispatch_decrypted_message(
    node: &FoldNode,
    store: &dyn fold_db::storage::traits::KvStore,
    master_key: &[u8],
    publisher: &DiscoveryPublisher,
    msg: &crate::discovery::types::EncryptedMessage,
    raw: serde_json::Value,
) -> Result<DispatchOutcome, HandlerError> {
    let message_type = raw
        .get("message_type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    match message_type.as_str() {
        "request" => {
            let payload: ConnectionPayload = match serde_json::from_value(raw) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(DispatchOutcome::Skipped {
                        reason: format!("parse connection request: {e}"),
                    });
                }
            };

            // Mutual contact detection via network intersection
            let mutual_contacts = if let Some(ref keys) = payload.network_keys {
                let op = OperationProcessor::new(node.clone());
                let our_book = op
                    .contact_book_path()
                    .ok()
                    .map(|p| ContactBook::load_from(&p).unwrap_or_default())
                    .unwrap_or_default();
                let our_keys: std::collections::HashSet<&str> = our_book
                    .active_contacts()
                    .iter()
                    .map(|c| c.public_key.as_str())
                    .collect();
                keys.iter()
                    .filter(|k| our_keys.contains(k.as_str()))
                    .filter_map(|k| {
                        our_book.get(k).map(|c| MutualContact {
                            display_name: c.display_name.clone(),
                            public_key: c.public_key.clone(),
                        })
                    })
                    .collect()
            } else {
                vec![]
            };

            let request_id = format!("msg-{}", msg.message_id);
            let local_req = LocalConnectionRequest {
                request_id,
                message_id: msg.message_id.clone(),
                target_pseudonym: msg.target_pseudonym.clone(),
                sender_pseudonym: payload.sender_pseudonym.clone(),
                sender_public_key: payload.sender_public_key.clone(),
                reply_public_key: payload.reply_public_key.clone(),
                message: payload.message.clone(),
                status: "pending".to_string(),
                created_at: msg.created_at.clone(),
                responded_at: None,
                vouches: Vec::new(),
                referral_query_id: None,
                referral_contacts_queried: 0,
                mutual_contacts,
                // Stash the requester's stable id so that when we later build
                // an accept message we can echo it back (G3 — stable match).
                sender_request_id: payload.request_id.clone(),
                // Stash the requester's stable identity pseudonym so we
                // can persist it on the contact row at accept-time and
                // use it as a referral-match key.
                sender_identity_pseudonym: payload.identity_pseudonym.clone(),
            };
            connection::save_received_request(store, &local_req)
                .await
                .map_err(|e| HandlerError::Internal(format!("save received request: {e}")))?;
            Ok(DispatchOutcome::Handled)
        }
        "accept" => {
            let payload: ConnectionPayload = match serde_json::from_value(raw) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(DispatchOutcome::Skipped {
                        reason: format!("parse connection accept: {e}"),
                    });
                }
            };
            let sent_request = connection::update_sent_request_status(
                store,
                &payload.sender_pseudonym,
                payload.request_id.as_deref(),
                "accepted",
            )
            .await
            .map_err(|e| HandlerError::Internal(format!("update sent request: {e}")))?;

            // Use the preferred_role from the original sent request, falling
            // back to "acquaintance" if unset or if the sent request wasn't found.
            let role = sent_request
                .as_ref()
                .and_then(|r| r.preferred_role.as_deref())
                .unwrap_or("acquaintance");

            // Auto-create trust relationship from accepted connection
            if payload.identity_card.is_some() {
                process_accepted_connection(node, &payload, role)
                    .await
                    .map_err(|e| {
                        HandlerError::Internal(format!("process accepted connection: {e}"))
                    })?;
            }
            Ok(DispatchOutcome::Handled)
        }
        "decline" => {
            let payload: ConnectionPayload = match serde_json::from_value(raw) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(DispatchOutcome::Skipped {
                        reason: format!("parse connection decline: {e}"),
                    });
                }
            };
            connection::update_sent_request_status(
                store,
                &payload.sender_pseudonym,
                payload.request_id.as_deref(),
                "declined",
            )
            .await
            .map_err(|e| HandlerError::Internal(format!("update sent request: {e}")))?;
            Ok(DispatchOutcome::Handled)
        }
        "query_request" => {
            let payload: QueryRequestPayload = match serde_json::from_value(raw) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(DispatchOutcome::Skipped {
                        reason: format!("parse query request: {e}"),
                    });
                }
            };
            handle_incoming_query(node, &payload, master_key, publisher).await
        }
        "query_response" => {
            let payload: QueryResponsePayload = match serde_json::from_value(raw) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(DispatchOutcome::Skipped {
                        reason: format!("parse query response: {e}"),
                    });
                }
            };
            handle_incoming_query_response(store, &payload)
                .await
                .map_err(|e| HandlerError::Internal(format!("handle query response: {e}")))?;
            Ok(DispatchOutcome::Handled)
        }
        "schema_list_request" => {
            let payload: SchemaListRequestPayload = match serde_json::from_value(raw) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(DispatchOutcome::Skipped {
                        reason: format!("parse schema list request: {e}"),
                    });
                }
            };
            handle_incoming_schema_list_request(node, &payload, master_key, publisher).await
        }
        "schema_list_response" => {
            let payload: SchemaListResponsePayload = match serde_json::from_value(raw) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(DispatchOutcome::Skipped {
                        reason: format!("parse schema list response: {e}"),
                    });
                }
            };
            handle_incoming_schema_list_response(store, &payload)
                .await
                .map_err(|e| HandlerError::Internal(format!("handle schema list response: {e}")))?;
            Ok(DispatchOutcome::Handled)
        }
        "data_share" => {
            let payload: DataSharePayload = match serde_json::from_value(raw) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(DispatchOutcome::Skipped {
                        reason: format!("parse data share: {e}"),
                    });
                }
            };

            // Part A: authorization gate — sender must be a known, non-revoked
            // contact. Unknown or revoked senders could otherwise inject
            // arbitrary mutations onto this node just by knowing our messaging
            // pseudonym + pubkey. See fix doc: trust-boundary hole.
            let op = OperationProcessor::new(node.clone());
            let book_path = op
                .contact_book_path()
                .map_err(|e| HandlerError::Internal(format!("contact book path: {e}")))?;
            let contact_book = ContactBook::load_from(&book_path).unwrap_or_default();
            match authorize_data_share_sender(&contact_book, &payload) {
                DataShareAuthz::Authorized => {}
                DataShareAuthz::UnknownSender => {
                    let pk_preview: String = payload.sender_public_key.chars().take(8).collect();
                    log::warn!(
                        "Rejecting data_share (msg {}): unknown sender pubkey {}",
                        msg.message_id,
                        pk_preview
                    );
                    return Ok(DispatchOutcome::Skipped {
                        reason: "data_share from unknown sender".to_string(),
                    });
                }
                DataShareAuthz::RevokedSender => {
                    let pk_preview: String = payload.sender_public_key.chars().take(8).collect();
                    log::warn!(
                        "Rejecting data_share (msg {}): revoked sender pubkey {}",
                        msg.message_id,
                        pk_preview
                    );
                    return Ok(DispatchOutcome::Skipped {
                        reason: "data_share from revoked sender".to_string(),
                    });
                }
            }

            // Part B: schema gate — schema service owns schema creation. Do
            // not allow a shared payload to install or auto-approve schemas.
            let db = node
                .get_fold_db()
                .map_err(|e| HandlerError::Internal(format!("Failed to get db: {e}")))?;
            let schema_states = db
                .schema_manager()
                .get_schema_states()
                .map_err(|e| HandlerError::Internal(format!("get schema states: {e}")))?;
            match validate_data_share_schemas(&schema_states, &payload) {
                DataShareSchemaCheck::AllApproved => {}
                DataShareSchemaCheck::UnknownOrUnapproved { schema_name } => {
                    log::warn!(
                        "Rejecting data_share (msg {}): schema '{}' not installed/approved on this node",
                        msg.message_id,
                        schema_name
                    );
                    return Ok(DispatchOutcome::Skipped {
                        reason: format!("data_share references unknown schema '{schema_name}'"),
                    });
                }
            }

            process_data_share(node, &payload).await?;
            log::info!(
                "Received {} records from {}",
                payload.records.len(),
                payload.sender_display_name
            );
            Ok(DispatchOutcome::Handled)
        }
        "referral_query" => {
            let payload: ReferralQueryPayload = match serde_json::from_value(raw) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(DispatchOutcome::Skipped {
                        reason: format!("parse referral query: {e}"),
                    });
                }
            };
            handle_incoming_referral_query(node, &payload, master_key, publisher).await
        }
        "referral_response" => {
            let payload: ReferralResponsePayload = match serde_json::from_value(raw) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(DispatchOutcome::Skipped {
                        reason: format!("parse referral response: {e}"),
                    });
                }
            };
            handle_incoming_referral_response(node, store, &payload).await
        }
        other => Ok(DispatchOutcome::Skipped {
            reason: format!("unknown message_type '{other}'"),
        }),
    }
}

// ===== Async query auto-processing helpers =====

/// Handle an incoming query request: execute the query and send results back.
/// Returns `Skipped` for permanent parse failures (bad reply pk, bad sender
/// pseudonym UUID — retrying won't recover). Returns `Err` for transient
/// network or encryption failures so the caller retries (FU-8).
async fn handle_incoming_query(
    node: &FoldNode,
    payload: &QueryRequestPayload,
    master_key: &[u8],
    publisher: &DiscoveryPublisher,
) -> Result<DispatchOutcome, HandlerError> {
    use crate::fold_node::OperationProcessor;
    use fold_db::schema::types::operations::Query;

    log::info!(
        "Processing incoming query request {} for schema '{}'",
        payload.request_id,
        payload.schema_name
    );

    let op = OperationProcessor::new(node.clone());
    let query = Query::new(payload.schema_name.clone(), payload.fields.clone());

    // Query execution errors are reported back to the caller in the response
    // payload — this is part of the wire protocol, not a silent failure.
    let (success, results, error) = match op
        .execute_query_json_with_access(query, &payload.sender_public_key)
        .await
    {
        Ok(results) => (true, Some(results), None),
        Err(e) => (false, None, Some(format!("Query failed: {}", e))),
    };

    let hash = crate::discovery::pseudonym::content_hash("connection-sender");
    let our_pseudonym = crate::discovery::pseudonym::derive_pseudonym(master_key, &hash);
    let our_reply_pk = connection::get_pseudonym_public_key_b64(master_key, &our_pseudonym);

    let response = QueryResponsePayload {
        message_type: "query_response".to_string(),
        request_id: payload.request_id.clone(),
        success,
        results,
        error,
        sender_pseudonym: our_pseudonym.to_string(),
        reply_public_key: our_reply_pk,
    };

    // Parse-permanent: reply public key malformed on the wire. Not recoverable.
    let reply_pk_bytes = match B64.decode(&payload.reply_public_key) {
        Ok(b) if b.len() == 32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&b);
            arr
        }
        _ => {
            return Ok(DispatchOutcome::Skipped {
                reason: format!(
                    "invalid reply public key in query request {}",
                    payload.request_id
                ),
            });
        }
    };

    // Encryption failure is a transient crypto error — propagate for retry.
    let encrypted = connection::encrypt_message(&reply_pk_bytes, &response)
        .map_err(|e| HandlerError::Internal(format!("encrypt query response: {e}")))?;

    // Parse-permanent: sender pseudonym malformed UUID. Not recoverable.
    let sender_pseudonym: uuid::Uuid = match payload.sender_pseudonym.parse() {
        Ok(u) => u,
        Err(e) => {
            return Ok(DispatchOutcome::Skipped {
                reason: format!("invalid sender pseudonym UUID: {e}"),
            });
        }
    };

    let encrypted_b64 = B64.encode(&encrypted);
    publisher
        .connect(sender_pseudonym, encrypted_b64, Some(our_pseudonym))
        .await
        .map_err(|e| HandlerError::Internal(format!("send query response: {e}")))?;
    Ok(DispatchOutcome::Handled)
}

/// Handle an incoming query response: update local async query with results.
/// Returns `Err` on transient store failure so the dispatcher retries
/// (FU-8: was previously swallowing the error silently).
async fn handle_incoming_query_response(
    store: &dyn fold_db::storage::traits::KvStore,
    payload: &QueryResponsePayload,
) -> Result<(), HandlerError> {
    log::info!("Received query response for request {}", payload.request_id);

    let results = if payload.success {
        match payload.results.as_ref() {
            Some(r) => Some(
                serde_json::to_value(r)
                    .map_err(|e| HandlerError::Internal(format!("serialize query results: {e}")))?,
            ),
            None => None,
        }
    } else {
        None
    };

    async_query::update_async_query_result(
        store,
        &payload.request_id,
        results,
        payload.error.clone(),
    )
    .await
    .map_err(|e| HandlerError::Internal(format!("update async query result: {e}")))?;
    Ok(())
}

/// Handle an incoming schema list request: list schemas and send back.
/// Returns `Skipped` for wire-malformed parse-permanent errors; returns `Err`
/// for transient db/network failures so the caller retries (FU-8).
async fn handle_incoming_schema_list_request(
    node: &FoldNode,
    payload: &SchemaListRequestPayload,
    master_key: &[u8],
    publisher: &DiscoveryPublisher,
) -> Result<DispatchOutcome, HandlerError> {
    use crate::fold_node::OperationProcessor;

    log::info!(
        "Processing incoming schema list request {}",
        payload.request_id
    );

    let op = OperationProcessor::new(node.clone());
    let db = op
        .get_db_public()
        .map_err(|e| HandlerError::Internal(format!("get db for schema list: {e}")))?;

    let all_schemas = db
        .schema_manager()
        .get_schemas()
        .map_err(|e| HandlerError::Internal(format!("get schemas: {e}")))?;
    let schemas: Vec<SchemaInfo> = all_schemas
        .values()
        .map(|s| SchemaInfo {
            name: s.name.clone(),
            descriptive_name: s.descriptive_name.clone(),
        })
        .collect();

    let hash = crate::discovery::pseudonym::content_hash("connection-sender");
    let our_pseudonym = crate::discovery::pseudonym::derive_pseudonym(master_key, &hash);
    let our_reply_pk = connection::get_pseudonym_public_key_b64(master_key, &our_pseudonym);

    let response = SchemaListResponsePayload {
        message_type: "schema_list_response".to_string(),
        request_id: payload.request_id.clone(),
        schemas,
        sender_pseudonym: our_pseudonym.to_string(),
        reply_public_key: our_reply_pk,
    };

    let reply_pk_bytes = match B64.decode(&payload.reply_public_key) {
        Ok(b) if b.len() == 32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&b);
            arr
        }
        _ => {
            return Ok(DispatchOutcome::Skipped {
                reason: "invalid reply public key in schema list request".to_string(),
            });
        }
    };

    let encrypted = connection::encrypt_message(&reply_pk_bytes, &response)
        .map_err(|e| HandlerError::Internal(format!("encrypt schema list response: {e}")))?;

    let sender_pseudonym: uuid::Uuid = match payload.sender_pseudonym.parse() {
        Ok(u) => u,
        Err(e) => {
            return Ok(DispatchOutcome::Skipped {
                reason: format!("invalid sender pseudonym UUID in schema list request: {e}"),
            });
        }
    };

    let encrypted_b64 = B64.encode(&encrypted);
    publisher
        .connect(sender_pseudonym, encrypted_b64, Some(our_pseudonym))
        .await
        .map_err(|e| HandlerError::Internal(format!("send schema list response: {e}")))?;
    Ok(DispatchOutcome::Handled)
}

/// Handle an incoming schema list response: update local async query.
/// Returns `Err` on transient store failure so the dispatcher retries
/// (FU-8: was previously swallowing the error silently).
async fn handle_incoming_schema_list_response(
    store: &dyn fold_db::storage::traits::KvStore,
    payload: &SchemaListResponsePayload,
) -> Result<(), HandlerError> {
    log::info!(
        "Received schema list response for request {}",
        payload.request_id
    );

    let results = serde_json::to_value(&payload.schemas)
        .map_err(|e| HandlerError::Internal(format!("serialize schema list: {e}")))?;
    async_query::update_async_query_result(store, &payload.request_id, Some(results), None)
        .await
        .map_err(|e| HandlerError::Internal(format!("update schema list result: {e}")))?;
    Ok(())
}

/// Process an accepted connection: create trust relationship and contact on our side.
/// Called when the polling loop receives an "accept" message with an identity card.
async fn process_accepted_connection(
    node: &FoldNode,
    acceptance: &ConnectionPayload,
    default_role: &str,
) -> Result<(), HandlerError> {
    let identity = acceptance
        .identity_card
        .as_ref()
        .ok_or_else(|| HandlerError::Internal("No identity card in acceptance".to_string()))?;

    let op = OperationProcessor::new(node.clone());
    let roles_path = op
        .sharing_roles_path()
        .map_err(|e| HandlerError::Internal(format!("Failed to resolve roles path: {e}")))?;
    let config = SharingRoleConfig::load_from(&roles_path)
        .map_err(|e| HandlerError::Internal(format!("Failed to load roles: {e}")))?;
    let role = config
        .get_role(default_role)
        .ok_or_else(|| HandlerError::Internal(format!("Unknown role: {default_role}")))?;

    op.grant_trust_for_domain(&identity.node_public_key, &role.domain, role.tier)
        .await
        .map_err(|e: fold_db::schema::SchemaError| {
            HandlerError::Internal(format!("Failed to grant trust: {e}"))
        })?;

    let contact = Contact::from_discovery(
        identity.node_public_key.clone(),
        identity.display_name.clone(),
        identity.contact_hint.clone(),
        TrustDirection::Outgoing,
        Some(acceptance.sender_pseudonym.clone()),
        Some(acceptance.reply_public_key.clone()),
        acceptance.identity_pseudonym.clone(),
        role.domain.clone(),
        default_role.to_string(),
    );
    let book_path = op
        .contact_book_path()
        .map_err(|e| HandlerError::Internal(format!("Failed to resolve contacts path: {e}")))?;
    let mut book = ContactBook::load_from(&book_path)
        .map_err(|e| HandlerError::Internal(format!("Failed to load contacts: {e}")))?;
    book.upsert_contact(contact);
    book.save_to(&book_path)
        .map_err(|e| HandlerError::Internal(format!("Failed to save contacts: {e}")))?;

    Ok(())
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
        let card = IdentityCard::load()
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
    let op = OperationProcessor::new(node.clone());
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

/// List locally stored received connection requests.
pub async fn list_connection_requests(
    node: &FoldNode,
) -> HandlerResult<ConnectionRequestsResponse> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    let requests = connection::list_received_requests(&*store)
        .await
        .handler_err("list received requests")?;

    Ok(ApiResponse::success(ConnectionRequestsResponse {
        requests,
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
    let op = OperationProcessor::new(node.clone());
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
    let sender_display_name = IdentityCard::load()
        .ok()
        .flatten()
        .map(|c| c.display_name)
        .unwrap_or_else(|| "Unknown".to_string());

    // 3. Load each record
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;

    let mut shared_records = Vec::with_capacity(req.records.len());

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
        let result_map = db.query_executor().query(query).await;
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
    let op = OperationProcessor::new(node.clone());
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

/// Pure match helper: does contact `c` correspond to the referral-query
/// subject described by `payload`?
///
/// Matching strategy:
/// 1. **Primary** — stable identity pseudonym. Only fires when *both*
///    sides carry one. Two nodes that both know the same subject derive
///    the same identity pseudonym for them, so this is the reliable key.
/// 2. **Legacy fallback** — rotating pseudonym / public-key fields.
///    Used for contacts that were created before identity pseudonyms
///    existed, or when the referral query came from a legacy sender.
fn referral_contact_matches(c: &Contact, payload: &ReferralQueryPayload) -> bool {
    let identity_match = match (
        c.identity_pseudonym.as_deref(),
        payload.subject_identity_pseudonym.as_deref(),
    ) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    };
    identity_match
        || c.pseudonym.as_deref() == Some(&payload.subject_pseudonym)
        || c.messaging_pseudonym.as_deref() == Some(&payload.subject_pseudonym)
        || c.messaging_public_key.as_deref() == Some(&payload.subject_public_key)
}

/// Pure match helper: does contact `c` correspond to the voucher who sent
/// referral `response`?
///
/// Same primary/fallback strategy as [`referral_contact_matches`].
fn referral_voucher_matches(c: &Contact, response: &ReferralResponsePayload) -> bool {
    let identity_match = match (
        c.identity_pseudonym.as_deref(),
        response.voucher_identity_pseudonym.as_deref(),
    ) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    };
    identity_match
        || c.messaging_pseudonym.as_deref() == Some(&response.sender_pseudonym)
        || c.pseudonym.as_deref() == Some(&response.sender_pseudonym)
}

/// Handle an incoming referral query: check if we know the subject and respond if so.
/// Returns `Skipped` for parse-permanent errors; `Err` for transient network
/// or crypto failures so the caller retries (FU-8).
async fn handle_incoming_referral_query(
    node: &FoldNode,
    payload: &ReferralQueryPayload,
    master_key: &[u8],
    publisher: &DiscoveryPublisher,
) -> Result<DispatchOutcome, HandlerError> {
    let op = OperationProcessor::new(node.clone());
    let book_path = op
        .contact_book_path()
        .map_err(|e| HandlerError::Internal(format!("contact book path: {e}")))?;
    let contact_book = ContactBook::load_from(&book_path).unwrap_or_default();

    // Check if we know the subject. See `referral_contact_matches` for the
    // primary/legacy-fallback match strategy.
    let active = contact_book.active_contacts();
    let matched_contact = active.iter().find(|c| referral_contact_matches(c, payload));

    let contact = match matched_contact {
        Some(c) => (*c).clone(),
        None => {
            // Silence = no. This is the protocol: non-matching referral queries
            // produce no response. Mark as handled (nothing to retry).
            return Ok(DispatchOutcome::Handled);
        }
    };

    // Derive our connection-sender pseudonym + X25519 key
    let sender_hash = crate::discovery::pseudonym::content_hash("connection-sender");
    let our_pseudonym = crate::discovery::pseudonym::derive_pseudonym(master_key, &sender_hash);
    let our_pk_b64 = connection::get_pseudonym_public_key_b64(master_key, &our_pseudonym);

    let response = ReferralResponsePayload {
        message_type: "referral_response".to_string(),
        query_id: payload.query_id.clone(),
        known_as: contact.display_name.clone(),
        sender_pseudonym: our_pseudonym.to_string(),
        reply_public_key: our_pk_b64,
        // Our own stable identity pseudonym so the query originator can
        // resolve us to a contact row and render our display name
        // instead of "Unknown contact".
        voucher_identity_pseudonym: Some(
            crate::discovery::pseudonym::derive_identity_pseudonym(master_key).to_string(),
        ),
    };

    let reply_pk_bytes = match B64.decode(&payload.reply_public_key) {
        Ok(b) if b.len() == 32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&b);
            arr
        }
        Ok(_) => {
            return Ok(DispatchOutcome::Skipped {
                reason: "referral query reply key wrong length".to_string(),
            });
        }
        Err(e) => {
            return Ok(DispatchOutcome::Skipped {
                reason: format!("invalid referral query reply key: {e}"),
            });
        }
    };

    let encrypted = connection::encrypt_message(&reply_pk_bytes, &response)
        .map_err(|e| HandlerError::Internal(format!("encrypt referral response: {e}")))?;

    let encrypted_b64 = B64.encode(&encrypted);

    let sender_uuid: uuid::Uuid = match payload.sender_pseudonym.parse() {
        Ok(u) => u,
        Err(e) => {
            return Ok(DispatchOutcome::Skipped {
                reason: format!("invalid sender pseudonym UUID in referral query: {e}"),
            });
        }
    };

    publisher
        .connect(sender_uuid, encrypted_b64, Some(our_pseudonym))
        .await
        .map_err(|e| HandlerError::Internal(format!("send referral response: {e}")))?;
    log::info!(
        "Sent referral response for query {} (known as {})",
        payload.query_id,
        contact.display_name
    );
    Ok(DispatchOutcome::Handled)
}

/// Handle an incoming referral response: append the vouch to the connection request.
/// Returns `Ok(DispatchOutcome::Skipped)` when the referenced connection request
/// no longer exists (permanent — retrying won't help). Propagates transient
/// store errors as `Err` so the dispatcher retries, instead of silently
/// dropping the vouch (FU-8).
async fn handle_incoming_referral_response(
    node: &FoldNode,
    store: &dyn fold_db::storage::traits::KvStore,
    payload: &ReferralResponsePayload,
) -> Result<DispatchOutcome, HandlerError> {
    // Scan for the connection request matching this query_id. A scan failure
    // here is transient — propagate so the caller retries.
    let entries = store
        .scan_prefix(b"discovery:conn_req:")
        .await
        .map_err(|e| HandlerError::Internal(format!("scan connection requests: {e}")))?;

    let mut found_key: Option<Vec<u8>> = None;
    let mut found_req: Option<LocalConnectionRequest> = None;
    for (key, value) in &entries {
        // Deserialization of an existing row: if one row is corrupt, log and
        // skip just that row — matching connection.rs's existing pattern. Do
        // NOT drop the whole message because of one bad neighbor.
        match serde_json::from_slice::<LocalConnectionRequest>(value) {
            Ok(local_req) => {
                if local_req.referral_query_id.as_deref() == Some(&payload.query_id) {
                    found_key = Some(key.clone());
                    found_req = Some(local_req);
                    break;
                }
            }
            Err(e) => {
                log::warn!("Corrupt connection request row during referral scan: {e}");
            }
        }
    }

    let (sled_key, mut local_req) = match (found_key, found_req) {
        (Some(k), Some(r)) => (k, r),
        _ => {
            log::warn!("Referral response for unknown query {}", payload.query_id);
            return Ok(DispatchOutcome::Skipped {
                reason: format!("referral response for unknown query {}", payload.query_id),
            });
        }
    };

    // Look up voucher identity from contact book. Contact-book load failure
    // is treated as "no voucher name"; this is deliberate because the vouch
    // itself is the important part and we already have the sender pseudonym
    // on the payload. We do NOT hide the error from the caller — we just
    // render an explicit "Unknown contact" label.
    let voucher_display_name = {
        let op = OperationProcessor::new(node.clone());
        match op.contact_book_path() {
            Ok(book_path) => {
                let contact_book = ContactBook::load_from(&book_path).unwrap_or_default();
                contact_book
                    .active_contacts()
                    .iter()
                    .find(|c| referral_voucher_matches(c, payload))
                    .map(|c| c.display_name.clone())
                    .unwrap_or_else(|| "Unknown contact".to_string())
            }
            Err(e) => {
                log::warn!("Failed to resolve contact book path for voucher lookup: {e}");
                "Unknown contact".to_string()
            }
        }
    };

    local_req.vouches.push(Vouch {
        voucher_display_name,
        known_as: payload.known_as.clone(),
        received_at: chrono::Utc::now().to_rfc3339(),
    });

    let updated = serde_json::to_vec(&local_req).map_err(|e| {
        HandlerError::Internal(format!("serialize vouched connection request: {e}"))
    })?;

    store
        .put(&sled_key, updated)
        .await
        .map_err(|e| HandlerError::Internal(format!("save vouched connection request: {e}")))?;
    log::info!(
        "Added vouch for referral query {} (known as '{}')",
        payload.query_id,
        payload.known_as
    );
    Ok(DispatchOutcome::Handled)
}

/// Outcome of the data-share sender authorization check.
///
/// Pure-function result so the gate can be unit tested without standing up a
/// full `FoldNode`. See [`authorize_data_share_sender`].
#[derive(Debug, PartialEq, Eq)]
enum DataShareAuthz {
    /// Sender matches an active (non-revoked) contact.
    Authorized,
    /// Sender pubkey does not match any contact in the book.
    UnknownSender,
    /// Sender matches a contact whose trust has been revoked.
    RevokedSender,
}

/// Pure authorization helper: does `payload.sender_public_key` correspond to
/// a known, non-revoked contact in `contact_book`?
///
/// The primary match key is the Ed25519 pubkey (base64) because that is the
/// field already carried on every [`DataSharePayload`]. If the contact also
/// carries a stable `identity_pseudonym` (PR #418) and the payload ever grows
/// one, that would become the secondary key — for now only the pubkey match
/// is available and it is both necessary and sufficient.
fn authorize_data_share_sender(
    contact_book: &ContactBook,
    payload: &DataSharePayload,
) -> DataShareAuthz {
    match contact_book.get(&payload.sender_public_key) {
        None => DataShareAuthz::UnknownSender,
        Some(c) if c.revoked => DataShareAuthz::RevokedSender,
        Some(_) => DataShareAuthz::Authorized,
    }
}

/// Outcome of the data-share schema validation check.
#[derive(Debug, PartialEq, Eq)]
enum DataShareSchemaCheck {
    /// Every record references a schema that is already installed and
    /// approved on this node.
    AllApproved,
    /// At least one record references a schema the node has not installed
    /// and approved. The first offending schema name is returned so the
    /// caller can log it.
    UnknownOrUnapproved { schema_name: String },
}

/// Pure schema-validation helper: every record in `payload` must reference a
/// schema that is already present on this node AND in the `Approved` state.
///
/// Schema creation is owned by the schema service. Shared payloads must NOT
/// be able to install or auto-approve arbitrary schema definitions — doing
/// so would let any contact silently add a schema to the recipient's node.
fn validate_data_share_schemas(
    schema_states: &HashMap<String, fold_db::schema::SchemaState>,
    payload: &DataSharePayload,
) -> DataShareSchemaCheck {
    for record in &payload.records {
        match schema_states.get(&record.schema_name) {
            Some(fold_db::schema::SchemaState::Approved) => {}
            _ => {
                return DataShareSchemaCheck::UnknownOrUnapproved {
                    schema_name: record.schema_name.clone(),
                };
            }
        }
    }
    DataShareSchemaCheck::AllApproved
}

/// Process a received data share: write mutations and save any included
/// file data.
///
/// Caller MUST have already authorized the sender (via
/// [`authorize_data_share_sender`]) and validated that every referenced
/// schema is installed and approved (via [`validate_data_share_schemas`]).
/// This function assumes both gates have passed and does not re-check.
async fn process_data_share(
    node: &FoldNode,
    payload: &DataSharePayload,
) -> Result<(), HandlerError> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to get db: {e}")))?;

    for record in &payload.records {
        // Write the mutation with the sender's pub_key. The dispatch arm has
        // already verified the schema is installed and approved, so any
        // failure here is a transient Sled hiccup — propagate so the caller
        // retries on the next poll instead of silently dropping the record.
        let key = fold_db::schema::types::key_value::KeyValue::new(
            record.key.hash.clone(),
            record.key.range.clone(),
        );

        let mutation = fold_db::schema::types::Mutation::new(
            record.schema_name.clone(),
            record.fields.clone(),
            key.clone(),
            payload.sender_public_key.clone(),
            fold_db::schema::types::operations::MutationType::Create,
        );

        db.mutation_manager()
            .write_mutations_batch_async(vec![mutation])
            .await
            .map_err(|e| {
                HandlerError::Internal(format!(
                    "write shared record for schema '{}': {e}",
                    record.schema_name
                ))
            })?;

        // If file data is included, save it to upload storage. A bad base64
        // blob is a permanent payload error — bail out with Internal so the
        // full partial write stays out of dedup and the peer can resend a
        // fixed payload. A Sled/fs write failure is transient — also bail.
        if let Some(ref file_b64) = record.file_data_base64 {
            let file_bytes = B64.decode(file_b64).map_err(|e| {
                HandlerError::Internal(format!(
                    "decode shared file data for schema '{}': {e}",
                    record.schema_name
                ))
            })?;
            let file_name = record
                .file_name
                .as_deref()
                .or_else(|| record.fields.get("file_hash").and_then(|v| v.as_str()))
                .unwrap_or("shared_file");

            let upload_path = std::env::var("FOLDDB_UPLOAD_PATH")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("data/uploads"));
            let upload_storage = fold_db::storage::UploadStorage::local(upload_path);

            upload_storage
                .save_file(file_name, &file_bytes, None)
                .await
                .map_err(|e| {
                    HandlerError::Internal(format!("save shared file '{file_name}': {e}"))
                })?;

            // Run face detection on shared photos. The mutation + file write
            // already succeeded, but face indexing is part of how this record
            // becomes searchable on the receiver. Treat failures as TRANSIENT
            // and propagate `Internal` so the dispatch arm retries on the
            // next poll cycle (matches the DispatchOutcome retry semantics
            // from PR #398). Silently warning would leave the record on the
            // node but invisible to face search forever.
            #[cfg(feature = "face-detection")]
            {
                let db_ops = db.get_db_ops();
                if let Some(native_idx) = db_ops.native_index_manager() {
                    if native_idx.has_face_processor() {
                        let count = native_idx
                            .index_faces(&record.schema_name, &key, &file_bytes)
                            .await
                            .map_err(|e| {
                                HandlerError::Internal(format!(
                                    "face indexing failed for shared photo '{file_name}': {e}"
                                ))
                            })?;
                        if count > 0 {
                            log::info!(
                                "Detected {} face(s) in shared photo '{}'",
                                count,
                                file_name
                            );
                        }
                    }
                }
            }
        }
    }

    // Store a notification for the UI. Failure here is transient (Sled) and
    // leaves the records written but the UI unnotified — propagate so the
    // poll retry can try again.
    let notification = serde_json::json!({
        "type": "data_share_received",
        "sender_display_name": payload.sender_display_name,
        "sender_public_key": payload.sender_public_key,
        "records_received": payload.records.len(),
        "schema_names": payload.records.iter().map(|r| r.schema_name.clone()).collect::<Vec<_>>(),
        "received_at": chrono::Utc::now().to_rfc3339(),
    });

    let notif_key = format!(
        "notification:{}:{}",
        chrono::Utc::now().timestamp_millis(),
        uuid::Uuid::new_v4()
    );
    let store = get_metadata_store(&db);
    store
        .put(
            notif_key.as_bytes(),
            serde_json::to_vec(&notification).map_err(|e| {
                HandlerError::Internal(format!("serialize data share notification: {e}"))
            })?,
        )
        .await
        .map_err(|e| HandlerError::Internal(format!("store data share notification: {e}")))?;

    log::info!(
        "Received {} records from {} (schemas: {})",
        payload.records.len(),
        payload.sender_display_name,
        payload
            .records
            .iter()
            .map(|r| r.schema_name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    Ok(())
}

// =========================================================================
// Tests for the discovery message dispatch loop
// =========================================================================
//
// These tests verify the "no silent failures" invariant of the bulletin-board
// poll loop: dedup markers must be written ONLY after dispatch reports either
// successful handling or a permanent (parse/unknown) skip. Transient errors
// must leave the dedup marker absent so the next poll can retry.
#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use crate::discovery::types::EncryptedMessage;
    use fold_db::storage::error::StorageResult;
    use fold_db::storage::inmemory_backend::InMemoryKvStore;
    use fold_db::storage::traits::{ExecutionModel, FlushBehavior, KvStore};
    use std::sync::Arc;
    use tempfile::tempdir;

    /// Build a minimal FoldNode suitable for tests that only touch the
    /// metadata store via the dispatch helper.
    async fn make_test_node() -> (FoldNode, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
        let config = crate::fold_node::NodeConfig::new(dir.path().to_path_buf())
            .with_schema_service_url("test://mock")
            .with_identity(&keypair.public_key_base64(), &keypair.secret_key_base64());
        let node = FoldNode::new(config).await.unwrap();
        (node, dir)
    }

    fn make_publisher() -> DiscoveryPublisher {
        DiscoveryPublisher::new(
            vec![0u8; 32],
            "test://mock".to_string(),
            "test-token".to_string(),
        )
    }

    fn make_msg(message_id: &str) -> EncryptedMessage {
        EncryptedMessage {
            message_id: message_id.to_string(),
            encrypted_blob: String::new(),
            target_pseudonym: uuid::Uuid::new_v4().to_string(),
            sender_pseudonym: None,
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// A KvStore wrapper that fails `put` for keys starting with a configured
    /// prefix. Used to simulate transient storage failures for specific
    /// sub-operations without breaking the dedup marker write.
    struct FailingPutStore {
        inner: Arc<InMemoryKvStore>,
        fail_prefix: Vec<u8>,
    }

    #[async_trait::async_trait]
    impl KvStore for FailingPutStore {
        async fn get(&self, key: &[u8]) -> StorageResult<Option<Vec<u8>>> {
            self.inner.get(key).await
        }
        async fn put(&self, key: &[u8], value: Vec<u8>) -> StorageResult<()> {
            if key.starts_with(&self.fail_prefix) {
                return Err(fold_db::storage::error::StorageError::BackendError(
                    "injected transient failure".to_string(),
                ));
            }
            self.inner.put(key, value).await
        }
        async fn delete(&self, key: &[u8]) -> StorageResult<bool> {
            self.inner.delete(key).await
        }
        async fn exists(&self, key: &[u8]) -> StorageResult<bool> {
            self.inner.exists(key).await
        }
        async fn scan_prefix(&self, prefix: &[u8]) -> StorageResult<Vec<(Vec<u8>, Vec<u8>)>> {
            self.inner.scan_prefix(prefix).await
        }
        async fn batch_put(&self, items: Vec<(Vec<u8>, Vec<u8>)>) -> StorageResult<()> {
            self.inner.batch_put(items).await
        }
        async fn batch_delete(&self, keys: Vec<Vec<u8>>) -> StorageResult<()> {
            self.inner.batch_delete(keys).await
        }
        async fn flush(&self) -> StorageResult<()> {
            self.inner.flush().await
        }
        fn backend_name(&self) -> &'static str {
            "failing-put-test"
        }
        fn execution_model(&self) -> ExecutionModel {
            self.inner.execution_model()
        }
        fn flush_behavior(&self) -> FlushBehavior {
            self.inner.flush_behavior()
        }
    }

    /// Parse-permanent: unknown message_type → Skipped (dedup should be marked
    /// by the caller so we don't re-log garbage forever).
    #[tokio::test]
    async fn unknown_message_type_is_skipped_not_errored() {
        let (node, _dir) = make_test_node().await;
        let publisher = make_publisher();
        let store: Arc<dyn KvStore> = Arc::new(InMemoryKvStore::new());
        let raw = serde_json::json!({"message_type": "totally_made_up"});
        let msg = make_msg("m-unknown");

        let outcome = dispatch_decrypted_message(&node, &*store, &[0u8; 32], &publisher, &msg, raw)
            .await
            .expect("unknown type must not return Err (it's a permanent skip)");

        match outcome {
            DispatchOutcome::Skipped { reason } => {
                assert!(
                    reason.contains("totally_made_up"),
                    "reason should name the unknown type, got: {reason}"
                );
            }
            DispatchOutcome::Handled => panic!("expected Skipped, got Handled"),
        }
    }

    /// Parse-permanent: malformed payload for a known message_type → Skipped.
    #[tokio::test]
    async fn malformed_request_payload_is_skipped() {
        let (node, _dir) = make_test_node().await;
        let publisher = make_publisher();
        let store: Arc<dyn KvStore> = Arc::new(InMemoryKvStore::new());
        // "request" expects ConnectionPayload fields; give it only message_type.
        let raw = serde_json::json!({"message_type": "request"});
        let msg = make_msg("m-bad-request");

        let outcome = dispatch_decrypted_message(&node, &*store, &[0u8; 32], &publisher, &msg, raw)
            .await
            .expect("parse failure must not return Err");

        assert!(matches!(outcome, DispatchOutcome::Skipped { .. }));
    }

    /// Happy path: a well-formed "request" payload is persisted and the
    /// dispatch returns Handled. A second identical dispatch (simulating the
    /// outer loop's dedup check) would be skipped by the caller via the dedup
    /// key — we verify the save landed in the store.
    #[tokio::test]
    async fn valid_request_is_handled_and_persisted() {
        let (node, _dir) = make_test_node().await;
        let publisher = make_publisher();
        let store: Arc<dyn KvStore> = Arc::new(InMemoryKvStore::new());

        let raw = serde_json::json!({
            "message_type": "request",
            "message": "hi",
            "sender_public_key": "pk1",
            "sender_pseudonym": "ps1",
            "reply_public_key": "rpk1",
        });
        let msg = make_msg("m-good");

        let outcome = dispatch_decrypted_message(&node, &*store, &[0u8; 32], &publisher, &msg, raw)
            .await
            .expect("valid request should succeed");

        assert!(matches!(outcome, DispatchOutcome::Handled));

        let received = connection::list_received_requests(&*store)
            .await
            .expect("list");
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].message_id, "m-good");
        assert_eq!(received[0].sender_pseudonym, "ps1");
    }

    /// Transient: `save_received_request` fails → dispatch returns Err. The
    /// outer loop uses this as the signal to NOT mark the dedup key, so the
    /// next poll can retry. We verify the error is returned AND that no
    /// connection-request was persisted (the failing put wrapper refused it).
    #[tokio::test]
    async fn transient_save_failure_returns_err_and_does_not_persist() {
        let (node, _dir) = make_test_node().await;
        let publisher = make_publisher();
        let inner = Arc::new(InMemoryKvStore::new());
        let store: Arc<dyn KvStore> = Arc::new(FailingPutStore {
            inner: inner.clone(),
            fail_prefix: b"discovery:conn_req:".to_vec(),
        });

        let raw = serde_json::json!({
            "message_type": "request",
            "message": "hi",
            "sender_public_key": "pk1",
            "sender_pseudonym": "ps1",
            "reply_public_key": "rpk1",
        });
        let msg = make_msg("m-transient");

        let outcome =
            dispatch_decrypted_message(&node, &*store, &[0u8; 32], &publisher, &msg, raw).await;

        match outcome {
            Err(HandlerError::Internal(msg)) => {
                assert!(
                    msg.contains("save received request"),
                    "error should identify the failing step, got: {msg}"
                );
            }
            other => panic!("expected Err(Internal), got {other:?}"),
        }

        // Nothing was persisted, so a subsequent retry would try again.
        let entries = inner
            .scan_prefix(b"discovery:conn_req:")
            .await
            .expect("scan");
        assert!(entries.is_empty(), "nothing should have been persisted");
    }

    /// Transient path + simulated outer-loop dedup behavior: verify the
    /// contract that Err leaves dedup absent and the next attempt re-runs
    /// dispatch. Models the key invariant from the bugfix.
    #[tokio::test]
    async fn err_leaves_dedup_absent_handled_sets_it() {
        let (node, _dir) = make_test_node().await;
        let publisher = make_publisher();
        let inner = Arc::new(InMemoryKvStore::new());

        // First attempt: store fails on save → Err, no dedup set.
        let failing: Arc<dyn KvStore> = Arc::new(FailingPutStore {
            inner: inner.clone(),
            fail_prefix: b"discovery:conn_req:".to_vec(),
        });
        let raw1 = serde_json::json!({
            "message_type": "request",
            "message": "hi",
            "sender_public_key": "pk1",
            "sender_pseudonym": "ps1",
            "reply_public_key": "rpk1",
        });
        let msg = make_msg("m-retry");
        let dedup_key = format!("msg_processed:{}", msg.message_id);

        let outcome1 =
            dispatch_decrypted_message(&node, &*failing, &[0u8; 32], &publisher, &msg, raw1).await;
        assert!(outcome1.is_err());
        // Outer loop would NOT have written the dedup key on Err.
        let dedup_present = inner.get(dedup_key.as_bytes()).await.unwrap();
        assert!(
            dedup_present.is_none(),
            "dedup key must be absent after transient failure"
        );

        // Second attempt: store is now healthy → Handled, caller sets dedup.
        let healthy: Arc<dyn KvStore> = inner.clone();
        let raw2 = serde_json::json!({
            "message_type": "request",
            "message": "hi",
            "sender_public_key": "pk1",
            "sender_pseudonym": "ps1",
            "reply_public_key": "rpk1",
        });
        let outcome2 =
            dispatch_decrypted_message(&node, &*healthy, &[0u8; 32], &publisher, &msg, raw2)
                .await
                .expect("second attempt should succeed");
        assert!(matches!(outcome2, DispatchOutcome::Handled));

        // Simulate the outer loop writing the dedup marker after Handled.
        healthy
            .put(dedup_key.as_bytes(), b"1".to_vec())
            .await
            .unwrap();
        let dedup_present = inner.get(dedup_key.as_bytes()).await.unwrap();
        assert_eq!(dedup_present.as_deref(), Some(&b"1"[..]));
    }
}

#[cfg(test)]
mod prune_tests {
    use super::*;
    use fold_db::storage::inmemory_backend::InMemoryKvStore;
    use fold_db::storage::traits::KvStore;

    #[tokio::test]
    async fn prune_deletes_old_markers_and_keeps_fresh_ones() {
        let store = InMemoryKvStore::default();
        let now: u64 = 1_700_000_000;

        // Fresh marker (1 hour old)
        let fresh_key = format!("{}fresh", MSG_PROCESSED_PREFIX);
        store
            .put(fresh_key.as_bytes(), encode_marker_timestamp(now - 3_600))
            .await
            .unwrap();

        // Stale marker (8 days old)
        let stale_key = format!("{}stale", MSG_PROCESSED_PREFIX);
        store
            .put(
                stale_key.as_bytes(),
                encode_marker_timestamp(now - 8 * 24 * 60 * 60),
            )
            .await
            .unwrap();

        // Legacy marker (pre-G2 `b"1"`) — treated as malformed and deleted.
        let legacy_key = format!("{}legacy", MSG_PROCESSED_PREFIX);
        store
            .put(legacy_key.as_bytes(), b"1".to_vec())
            .await
            .unwrap();

        // Unrelated key — must not be touched by prefix scan.
        store.put(b"other:untouched", b"x".to_vec()).await.unwrap();

        let deleted = prune_msg_processed_markers(&store, now, DEDUP_RETENTION_SECS)
            .await
            .unwrap();
        assert_eq!(deleted, 2, "stale + legacy should be deleted");

        assert!(store.get(fresh_key.as_bytes()).await.unwrap().is_some());
        assert!(store.get(stale_key.as_bytes()).await.unwrap().is_none());
        assert!(store.get(legacy_key.as_bytes()).await.unwrap().is_none());
        assert!(store.get(b"other:untouched").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn prune_noop_when_all_fresh() {
        let store = InMemoryKvStore::default();
        let now: u64 = 1_700_000_000;
        for i in 0..5 {
            let k = format!("{}msg-{}", MSG_PROCESSED_PREFIX, i);
            store
                .put(k.as_bytes(), encode_marker_timestamp(now - 60))
                .await
                .unwrap();
        }
        let deleted = prune_msg_processed_markers(&store, now, DEDUP_RETENTION_SECS)
            .await
            .unwrap();
        assert_eq!(deleted, 0);
    }

    // ===== connect sentinel tests (FU-2) =====

    #[tokio::test]
    async fn sentinel_first_acquire_succeeds() {
        let store = InMemoryKvStore::default();
        let store_ref: &dyn KvStore = &store;
        let target = "11111111-1111-1111-1111-111111111111";
        let outcome = try_acquire_connect_sentinel(store_ref, target, 1_000_000, 60)
            .await
            .expect("first acquire must not error");
        assert_eq!(outcome, SentinelAcquire::Acquired);
        // Sentinel key present.
        let key = format!("{}{}", CONNECT_IN_FLIGHT_PREFIX, target);
        assert!(store_ref.get(key.as_bytes()).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn sentinel_second_acquire_within_ttl_rejects() {
        let store = InMemoryKvStore::default();
        let store_ref: &dyn KvStore = &store;
        let target = "22222222-2222-2222-2222-222222222222";
        let first = try_acquire_connect_sentinel(store_ref, target, 1_000_000, 60)
            .await
            .unwrap();
        assert_eq!(first, SentinelAcquire::Acquired);
        // Only 10 seconds have passed — still in flight.
        let second = try_acquire_connect_sentinel(store_ref, target, 1_000_010, 60)
            .await
            .unwrap();
        assert_eq!(second, SentinelAcquire::InFlight);
    }

    #[tokio::test]
    async fn sentinel_stale_is_overwritten() {
        let store = InMemoryKvStore::default();
        let store_ref: &dyn KvStore = &store;
        let target = "33333333-3333-3333-3333-333333333333";
        try_acquire_connect_sentinel(store_ref, target, 1_000_000, 60)
            .await
            .unwrap();
        // Clock has advanced 2 minutes. Previous sentinel is stale.
        let second = try_acquire_connect_sentinel(store_ref, target, 1_000_120, 60)
            .await
            .unwrap();
        assert_eq!(second, SentinelAcquire::Acquired);
        // The stored timestamp should be the new one.
        let key = format!("{}{}", CONNECT_IN_FLIGHT_PREFIX, target);
        let value = store_ref.get(key.as_bytes()).await.unwrap().unwrap();
        assert_eq!(std::str::from_utf8(&value).unwrap(), "1000120");
    }

    #[tokio::test]
    async fn sentinel_release_clears_key_so_next_acquire_succeeds() {
        let store = InMemoryKvStore::default();
        let store_ref: &dyn KvStore = &store;
        let target = "44444444-4444-4444-4444-444444444444";
        try_acquire_connect_sentinel(store_ref, target, 1_000_000, 60)
            .await
            .unwrap();
        release_connect_sentinel(store_ref, target).await;
        let key = format!("{}{}", CONNECT_IN_FLIGHT_PREFIX, target);
        assert!(store_ref.get(key.as_bytes()).await.unwrap().is_none());
        // And a fresh acquire at the same logical time now succeeds.
        let again = try_acquire_connect_sentinel(store_ref, target, 1_000_005, 60)
            .await
            .unwrap();
        assert_eq!(again, SentinelAcquire::Acquired);
    }

    #[tokio::test]
    async fn sentinel_corrupt_value_errors_loudly() {
        let store = InMemoryKvStore::default();
        let store_ref: &dyn KvStore = &store;
        let target = "55555555-5555-5555-5555-555555555555";
        let key = format!("{}{}", CONNECT_IN_FLIGHT_PREFIX, target);
        store_ref
            .put(key.as_bytes(), b"not-a-timestamp".to_vec())
            .await
            .unwrap();
        let err = try_acquire_connect_sentinel(store_ref, target, 1_000_000, 60)
            .await
            .expect_err("corrupt sentinel must surface as an error, not silently overwrite");
        match err {
            HandlerError::Internal(msg) => assert!(msg.contains("corrupt")),
            other => panic!("expected Internal error, got {other:?}"),
        }
    }
}

// ---- Referral match tests ---------------------------------------------------

/// Pure unit tests for [`referral_contact_matches`] / [`referral_voucher_matches`].
///
/// Exercises the stable-identity-pseudonym primary match and the legacy
/// rotating-pseudonym fallback. These pure helpers are the only logic we can
/// test without standing up a full `FoldNode` + publisher, which is precisely
/// why the matching was extracted into them.
#[cfg(test)]
mod referral_match_tests {
    use super::*;
    use chrono::Utc;
    use std::collections::HashMap;

    fn contact_with(
        identity: Option<&str>,
        pseudonym: Option<&str>,
        messaging_pseudonym: Option<&str>,
        messaging_public_key: Option<&str>,
    ) -> Contact {
        Contact {
            public_key: "pk".to_string(),
            display_name: "Bob".to_string(),
            contact_hint: None,
            direction: TrustDirection::Mutual,
            connected_at: Utc::now(),
            pseudonym: pseudonym.map(str::to_string),
            messaging_pseudonym: messaging_pseudonym.map(str::to_string),
            messaging_public_key: messaging_public_key.map(str::to_string),
            identity_pseudonym: identity.map(str::to_string),
            revoked: false,
            roles: HashMap::new(),
        }
    }

    fn query_with(
        subject_identity: Option<&str>,
        subject_pseudonym: &str,
        subject_pk: &str,
    ) -> ReferralQueryPayload {
        ReferralQueryPayload {
            message_type: "referral_query".to_string(),
            query_id: "qid".to_string(),
            subject_pseudonym: subject_pseudonym.to_string(),
            subject_public_key: subject_pk.to_string(),
            sender_pseudonym: "sender".to_string(),
            reply_public_key: "reply".to_string(),
            subject_identity_pseudonym: subject_identity.map(str::to_string),
        }
    }

    fn response_with(
        voucher_identity: Option<&str>,
        sender_pseudonym: &str,
    ) -> ReferralResponsePayload {
        ReferralResponsePayload {
            message_type: "referral_response".to_string(),
            query_id: "qid".to_string(),
            known_as: "Charlie".to_string(),
            sender_pseudonym: sender_pseudonym.to_string(),
            reply_public_key: "reply".to_string(),
            voucher_identity_pseudonym: voucher_identity.map(str::to_string),
        }
    }

    /// **The memory-flagged bug** — rotating pseudonyms differ between Alice
    /// and Bob, so every legacy match field is wrong. The identity pseudonym
    /// is the only key that agrees, and it alone must produce a match.
    #[test]
    fn matches_on_identity_pseudonym_when_all_legacy_fields_mismatch() {
        let bob_contact_row = contact_with(
            Some("IDENTITY-STABLE"),
            Some("bobs-view-of-charlie"),
            Some("bobs-view-of-charlie"),
            Some("bobs-view-pk"),
        );
        // Alice derived totally different rotating values for Charlie:
        let alice_query = query_with(
            Some("IDENTITY-STABLE"),
            "alices-view-of-charlie",
            "alices-view-pk",
        );
        assert!(
            referral_contact_matches(&bob_contact_row, &alice_query),
            "identity pseudonym match must succeed even when every legacy field disagrees"
        );
    }

    /// Legacy compat — an old contact row with no identity pseudonym, and an
    /// old-sender query with no identity pseudonym, must still match via the
    /// rotating pseudonym field. Guards the migration path.
    #[test]
    fn legacy_fallback_matches_on_messaging_pseudonym() {
        let legacy_contact = contact_with(None, None, Some("rotating-pseudo"), Some("old-pk"));
        let legacy_query = query_with(None, "rotating-pseudo", "some-other-pk");
        assert!(referral_contact_matches(&legacy_contact, &legacy_query));
    }

    /// Negative case — both sides have identity pseudonyms but they differ.
    /// The identity match fails and the legacy rotating fields don't agree
    /// either, so the overall match must be false. No silent catch-all.
    #[test]
    fn no_match_when_identity_pseudonyms_differ_and_legacy_fields_disagree() {
        let contact = contact_with(Some("id-a"), Some("rot-a"), Some("rot-a"), Some("pk-a"));
        let query = query_with(Some("id-b"), "rot-b", "pk-b");
        assert!(!referral_contact_matches(&contact, &query));
    }

    /// **Voucher-lookup symptom from the memory doc** — Bob derived
    /// `sender_pseudonym` from the stable `connection-sender` tag, but
    /// Alice's contact row for Bob was keyed off Bob's first opt-in config
    /// name, so the rotating fields disagree. The identity pseudonym is the
    /// one key they share, and it must resolve `voucher_display_name` to
    /// Bob's actual name instead of falling through to "Unknown contact".
    #[test]
    fn voucher_matches_on_identity_pseudonym() {
        let alices_row_for_bob = contact_with(
            Some("BOBS-IDENTITY"),
            Some("alices-view-of-bob"),
            Some("alices-view-of-bob"),
            Some("alices-view-pk"),
        );
        let bobs_response = response_with(Some("BOBS-IDENTITY"), "bobs-connection-sender-pseudo");
        assert!(referral_voucher_matches(
            &alices_row_for_bob,
            &bobs_response
        ));
    }

    #[test]
    fn voucher_legacy_fallback_matches_on_sender_pseudonym() {
        let legacy_row = contact_with(None, None, Some("rotating"), Some("pk"));
        let legacy_response = response_with(None, "rotating");
        assert!(referral_voucher_matches(&legacy_row, &legacy_response));
    }
}

// ---- data_share authorization gate tests -----------------------------------

/// Pure unit tests for the `data_share` trust-boundary gates:
/// [`authorize_data_share_sender`] and [`validate_data_share_schemas`].
///
/// These gates protect against a sender with no existing trust relationship
/// injecting mutations (or inventing schemas) on the recipient's node just
/// by knowing the recipient's messaging pseudonym + pubkey.
#[cfg(test)]
mod data_share_gate_tests {
    use super::*;
    use chrono::Utc;
    use std::collections::HashMap;

    fn active_contact(public_key: &str) -> Contact {
        Contact {
            public_key: public_key.to_string(),
            display_name: "Alice".to_string(),
            contact_hint: None,
            direction: TrustDirection::Mutual,
            connected_at: Utc::now(),
            pseudonym: None,
            messaging_pseudonym: None,
            messaging_public_key: None,
            identity_pseudonym: None,
            revoked: false,
            roles: HashMap::new(),
        }
    }

    fn revoked_contact(public_key: &str) -> Contact {
        let mut c = active_contact(public_key);
        c.revoked = true;
        c
    }

    fn payload_from(sender_pk: &str, schema_names: &[&str]) -> DataSharePayload {
        DataSharePayload {
            message_type: "data_share".to_string(),
            sender_public_key: sender_pk.to_string(),
            sender_display_name: "Alice".to_string(),
            records: schema_names
                .iter()
                .map(|name| SharedRecord {
                    schema_name: (*name).to_string(),
                    schema_definition: None,
                    fields: HashMap::new(),
                    key: SharedRecordKey {
                        hash: Some("h".to_string()),
                        range: None,
                    },
                    file_data_base64: None,
                    file_name: None,
                })
                .collect(),
        }
    }

    #[test]
    fn gate_rejects_unknown_sender() {
        let book = ContactBook::new();
        let payload = payload_from("pk_alice", &["Photography"]);
        assert_eq!(
            authorize_data_share_sender(&book, &payload),
            DataShareAuthz::UnknownSender
        );
    }

    #[test]
    fn gate_rejects_revoked_sender() {
        let mut book = ContactBook::new();
        book.upsert_contact(revoked_contact("pk_alice"));
        let payload = payload_from("pk_alice", &["Photography"]);
        assert_eq!(
            authorize_data_share_sender(&book, &payload),
            DataShareAuthz::RevokedSender
        );
    }

    #[test]
    fn gate_accepts_active_sender() {
        let mut book = ContactBook::new();
        book.upsert_contact(active_contact("pk_alice"));
        let payload = payload_from("pk_alice", &["Photography"]);
        assert_eq!(
            authorize_data_share_sender(&book, &payload),
            DataShareAuthz::Authorized
        );
    }

    #[test]
    fn schema_gate_rejects_unknown_schema() {
        let states: HashMap<String, fold_db::schema::SchemaState> = HashMap::new();
        let payload = payload_from("pk_alice", &["Photography"]);
        assert_eq!(
            validate_data_share_schemas(&states, &payload),
            DataShareSchemaCheck::UnknownOrUnapproved {
                schema_name: "Photography".to_string()
            }
        );
    }

    #[test]
    fn schema_gate_rejects_non_approved_schema() {
        let mut states: HashMap<String, fold_db::schema::SchemaState> = HashMap::new();
        // Present but only Available — not yet approved. Must be rejected to
        // preserve the invariant that users explicitly approve schemas.
        states.insert(
            "Photography".to_string(),
            fold_db::schema::SchemaState::Available,
        );
        let payload = payload_from("pk_alice", &["Photography"]);
        assert_eq!(
            validate_data_share_schemas(&states, &payload),
            DataShareSchemaCheck::UnknownOrUnapproved {
                schema_name: "Photography".to_string()
            }
        );
    }

    #[test]
    fn schema_gate_accepts_all_approved_schemas() {
        let mut states: HashMap<String, fold_db::schema::SchemaState> = HashMap::new();
        states.insert(
            "Photography".to_string(),
            fold_db::schema::SchemaState::Approved,
        );
        states.insert("Travel".to_string(), fold_db::schema::SchemaState::Approved);
        let payload = payload_from("pk_alice", &["Photography", "Travel"]);
        assert_eq!(
            validate_data_share_schemas(&states, &payload),
            DataShareSchemaCheck::AllApproved
        );
    }

    #[test]
    fn schema_gate_rejects_mixed_when_any_is_unknown() {
        let mut states: HashMap<String, fold_db::schema::SchemaState> = HashMap::new();
        states.insert(
            "Photography".to_string(),
            fold_db::schema::SchemaState::Approved,
        );
        // "Travel" is missing entirely — must reject even though the first
        // record is fine. Partial writes are not acceptable.
        let payload = payload_from("pk_alice", &["Photography", "Travel"]);
        assert_eq!(
            validate_data_share_schemas(&states, &payload),
            DataShareSchemaCheck::UnknownOrUnapproved {
                schema_name: "Travel".to_string()
            }
        );
    }
}
