//! Shared Discovery Handlers
//!
//! Framework-agnostic handlers for discovery network operations.

use crate::discovery::async_query::{
    self, QueryRequestPayload, QueryResponsePayload, SchemaInfo, SchemaListRequestPayload,
    SchemaListResponsePayload,
};
use crate::discovery::calendar_sharing::{self, EventFingerprint, PeerEventSet, SharedEvent};
use crate::discovery::config::{self, DiscoveryOptIn};
use crate::discovery::connection::{
    self, ConnectionPayload, DataSharePayload, IdentityCardPayload, LocalConnectionRequest,
    LocalSentRequest, MutualContact, ReferralQueryPayload, ReferralResponsePayload, SharedRecord,
    SharedRecordKey, Vouch,
};
use crate::discovery::interests::{self, InterestProfile};
use crate::discovery::moments;
use crate::discovery::publisher::DiscoveryPublisher;
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

/// Maximum number of results per search query.
const MAX_TOP_K: usize = 100;
/// Maximum offset for paginated results.
const MAX_OFFSET: usize = 10_000;
/// Maximum number of photos in a single moment scan request.
const MAX_PHOTO_BATCH: usize = 1_000;

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
fn get_metadata_store(
    db: &fold_db::fold_db_core::FoldDB,
) -> std::sync::Arc<dyn fold_db::storage::traits::KvStore> {
    db.get_db_ops().metadata_store().inner().clone()
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

    // Derive pseudonyms locally and send to discovery service for deletion.
    // No server-side pseudonym-to-user mapping — privacy by design.
    let publisher = DiscoveryPublisher::new(
        master_key.to_vec(),
        discovery_url.to_string(),
        auth_token.to_string(),
    );
    let pseudonyms = publisher
        .derive_schema_pseudonyms(&*store, &req.schema_name)
        .await
        .handler_err("derive pseudonyms for opt-out")?;
    if !pseudonyms.is_empty() {
        publisher
            .unpublish_pseudonyms(pseudonyms)
            .await
            .handler_err("unpublish from discovery service")?;
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

    let results = publisher
        .search(query_embedding, top_k, req.category_filter.clone(), offset)
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
    // Check if already connected to this pseudonym
    let op = OperationProcessor::new(node.clone());
    let book_path = op
        .contact_book_path()
        .map_err(|e| HandlerError::Internal(format!("Failed to resolve contacts path: {e}")))?;
    let contact_book = ContactBook::load_from(&book_path).unwrap_or_default();
    let target_str = req.target_pseudonym.to_string();
    if contact_book.active_contacts().iter().any(|c| {
        c.pseudonym.as_deref() == Some(target_str.as_str())
            || c.messaging_pseudonym.as_deref() == Some(target_str.as_str())
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
        request_id: uuid::Uuid::new_v4().to_string(),
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
    let db_ops = db.get_db_ops();
    let store = get_metadata_store(&db);

    // Get our published pseudonyms by scanning the native index — same derivation
    // as the publisher uses when uploading: derive(master_key, SHA256(embedding_bytes)).
    let configs = config::list_opt_ins(&*store)
        .await
        .handler_err("list opt-ins")?;

    let our_pseudonyms: Vec<uuid::Uuid> = {
        let mut pseudonyms = Vec::new();

        // Add our connection-sender pseudonym (used by the connect handler as sender_pseudonym)
        let hash = crate::discovery::pseudonym::content_hash("connection-sender");
        pseudonyms.push(crate::discovery::pseudonym::derive_pseudonym(
            master_key, &hash,
        ));

        // Add pseudonyms derived from actual published embeddings (same as publisher.rs)
        let native_index_mgr = db_ops.native_index_manager();
        if let Some(nim) = native_index_mgr {
            let embedding_store = nim.store().clone();
            for cfg in &configs {
                let prefix = format!("emb:{}:", cfg.schema_name);
                if let Ok(raw_entries) = embedding_store.scan_prefix(prefix.as_bytes()).await {
                    for (_key, value) in &raw_entries {
                        if let Ok(stored) = serde_json::from_slice::<serde_json::Value>(value) {
                            if let Some(emb_arr) =
                                stored.get("embedding").and_then(|e| e.as_array())
                            {
                                let embedding_bytes: Vec<u8> = emb_arr
                                    .iter()
                                    .filter_map(|v| v.as_f64().map(|f| f as f32))
                                    .flat_map(|f| f.to_le_bytes())
                                    .collect();
                                let content_hash = crate::discovery::pseudonym::content_hash_bytes(
                                    &embedding_bytes,
                                );
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
        // Cap pseudonyms to avoid excessively long URLs in the poll request.
        // Each UUID is 36 chars + comma separator. At 1000 pseudonyms that's ~37KB,
        // within typical URL limits for most HTTP servers.
        pseudonyms.truncate(1000);
        pseudonyms
    };

    if our_pseudonyms.is_empty() {
        return Ok(ApiResponse::success(ConnectionRequestsResponse {
            requests: Vec::new(),
        }));
    }

    // Poll messages: if we have a reasonable number of pseudonyms, filter server-side.
    // Otherwise poll all recent messages and filter client-side during decryption.
    let pseudonym_filter = if our_pseudonyms.len() <= 100 {
        Some(our_pseudonyms.as_slice())
    } else {
        log::info!(
            "Too many pseudonyms ({}) for URL filter, polling all recent messages",
            our_pseudonyms.len()
        );
        None
    };
    let messages = publisher
        .poll_messages(None, pseudonym_filter)
        .await
        .handler_err("poll messages")?;

    // Try to decrypt each message
    for msg in &messages {
        let target: uuid::Uuid = match msg.target_pseudonym.parse() {
            Ok(u) => u,
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
                continue; // Not for us or corrupted
            }
        };

        let message_type = raw
            .get("message_type")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // De-duplication: check if we already processed this message
        let dedup_key = format!("msg_processed:{}", msg.message_id);
        let existing = store.get(dedup_key.as_bytes()).await.ok().flatten();
        if existing.is_some() {
            continue;
        }

        // Mark as processed (store a small marker)
        let _ = store.put(dedup_key.as_bytes(), b"1".to_vec()).await;

        match message_type {
            "request" => {
                let payload: ConnectionPayload = match serde_json::from_value(raw) {
                    Ok(p) => p,
                    Err(e) => {
                        log::warn!("Failed to parse connection request: {}", e);
                        continue;
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
                    request_id: request_id.clone(),
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
                };
                if let Err(e) = connection::save_received_request(&*store, &local_req).await {
                    log::warn!("Failed to save received request: {}", e);
                }
            }
            "accept" => {
                let payload: ConnectionPayload = match serde_json::from_value(raw) {
                    Ok(p) => p,
                    Err(e) => {
                        log::warn!("Failed to parse connection accept: {}", e);
                        continue;
                    }
                };
                let sent_request = match connection::update_sent_request_status(
                    &*store,
                    &payload.sender_pseudonym,
                    "accepted",
                )
                .await
                {
                    Ok(req) => req,
                    Err(e) => {
                        log::warn!("Failed to update sent request: {}", e);
                        None
                    }
                };

                // Use the preferred_role from the original sent request, falling
                // back to "acquaintance" if unset or if the sent request wasn't found.
                let role = sent_request
                    .as_ref()
                    .and_then(|r| r.preferred_role.as_deref())
                    .unwrap_or("acquaintance");

                // Auto-create trust relationship from accepted connection
                if payload.identity_card.is_some() {
                    if let Err(e) = process_accepted_connection(node, &payload, role).await {
                        log::warn!(
                            "Failed to auto-create trust from accepted connection: {}",
                            e
                        );
                    }
                }
            }
            "decline" => {
                let payload: ConnectionPayload = match serde_json::from_value(raw) {
                    Ok(p) => p,
                    Err(e) => {
                        log::warn!("Failed to parse connection decline: {}", e);
                        continue;
                    }
                };
                if let Err(e) = connection::update_sent_request_status(
                    &*store,
                    &payload.sender_pseudonym,
                    "declined",
                )
                .await
                {
                    log::warn!("Failed to update sent request: {e}");
                }
            }
            "query_request" => {
                let payload: QueryRequestPayload = match serde_json::from_value(raw) {
                    Ok(p) => p,
                    Err(e) => {
                        log::warn!("Failed to parse query request: {}", e);
                        continue;
                    }
                };
                handle_incoming_query(node, &payload, master_key, &publisher).await;
            }
            "query_response" => {
                let payload: QueryResponsePayload = match serde_json::from_value(raw) {
                    Ok(p) => p,
                    Err(e) => {
                        log::warn!("Failed to parse query response: {}", e);
                        continue;
                    }
                };
                handle_incoming_query_response(&*store, &payload).await;
            }
            "schema_list_request" => {
                let payload: SchemaListRequestPayload = match serde_json::from_value(raw) {
                    Ok(p) => p,
                    Err(e) => {
                        log::warn!("Failed to parse schema list request: {}", e);
                        continue;
                    }
                };
                handle_incoming_schema_list_request(node, &payload, master_key, &publisher).await;
            }
            "schema_list_response" => {
                let payload: SchemaListResponsePayload = match serde_json::from_value(raw) {
                    Ok(p) => p,
                    Err(e) => {
                        log::warn!("Failed to parse schema list response: {}", e);
                        continue;
                    }
                };
                handle_incoming_schema_list_response(&*store, &payload).await;
            }
            "data_share" => {
                let payload: DataSharePayload = match serde_json::from_value(raw) {
                    Ok(p) => p,
                    Err(e) => {
                        log::warn!("Failed to parse data share: {}", e);
                        continue;
                    }
                };

                if let Err(e) = process_data_share(node, &payload).await {
                    log::warn!(
                        "Failed to process data share from {}: {}",
                        payload.sender_display_name,
                        e
                    );
                } else {
                    log::info!(
                        "Received {} records from {}",
                        payload.records.len(),
                        payload.sender_display_name
                    );
                }
            }
            "referral_query" => {
                if let Ok(payload) = serde_json::from_value::<ReferralQueryPayload>(raw.clone()) {
                    handle_incoming_referral_query(node, &payload, master_key, &publisher).await;
                } else {
                    log::warn!("Failed to parse referral_query payload");
                }
            }
            "referral_response" => {
                if let Ok(payload) = serde_json::from_value::<ReferralResponsePayload>(raw.clone())
                {
                    handle_incoming_referral_response(node, &*store, &payload).await;
                } else {
                    log::warn!("Failed to parse referral_response payload");
                }
            }
            _ => {
                log::warn!("Unknown message type: {}", message_type);
            }
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

// ===== Async query auto-processing helpers =====

/// Handle an incoming query request: execute the query and send results back.
async fn handle_incoming_query(
    node: &FoldNode,
    payload: &QueryRequestPayload,
    master_key: &[u8],
    publisher: &DiscoveryPublisher,
) {
    use crate::fold_node::OperationProcessor;
    use fold_db::schema::types::operations::Query;

    log::info!(
        "Processing incoming query request {} for schema '{}'",
        payload.request_id,
        payload.schema_name
    );

    let op = OperationProcessor::new(node.clone());
    let query = Query::new(payload.schema_name.clone(), payload.fields.clone());

    // Execute with access control using sender's Ed25519 key
    let (success, results, error) = match op
        .execute_query_json_with_access(query, &payload.sender_public_key)
        .await
    {
        Ok(results) => (true, Some(results), None),
        Err(e) => (false, None, Some(format!("Query failed: {}", e))),
    };

    // Derive our reply pseudonym + X25519 key
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

    // Encrypt with requester's reply public key and send back
    let reply_pk_bytes = match B64.decode(&payload.reply_public_key) {
        Ok(b) if b.len() == 32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&b);
            arr
        }
        _ => {
            log::warn!(
                "Invalid reply public key in query request {}",
                payload.request_id
            );
            return;
        }
    };

    let encrypted = match connection::encrypt_message(&reply_pk_bytes, &response) {
        Ok(e) => e,
        Err(e) => {
            log::warn!("Failed to encrypt query response: {}", e);
            return;
        }
    };

    let sender_pseudonym: uuid::Uuid = match payload.sender_pseudonym.parse() {
        Ok(u) => u,
        Err(_) => {
            log::warn!("Invalid sender pseudonym in query request");
            return;
        }
    };

    let encrypted_b64 = B64.encode(&encrypted);
    if let Err(e) = publisher
        .connect(sender_pseudonym, encrypted_b64, Some(our_pseudonym))
        .await
    {
        log::warn!("Failed to send query response: {}", e);
    }
}

/// Handle an incoming query response: update local async query with results.
async fn handle_incoming_query_response(
    store: &dyn fold_db::storage::traits::KvStore,
    payload: &QueryResponsePayload,
) {
    log::info!("Received query response for request {}", payload.request_id);

    let results = if payload.success {
        payload
            .results
            .as_ref()
            .map(|r| serde_json::to_value(r).unwrap_or_default())
    } else {
        None
    };

    if let Err(e) = async_query::update_async_query_result(
        store,
        &payload.request_id,
        results,
        payload.error.clone(),
    )
    .await
    {
        log::warn!("Failed to update async query result: {}", e);
    }
}

/// Handle an incoming schema list request: list schemas and send back.
async fn handle_incoming_schema_list_request(
    node: &FoldNode,
    payload: &SchemaListRequestPayload,
    master_key: &[u8],
    publisher: &DiscoveryPublisher,
) {
    use crate::fold_node::OperationProcessor;

    log::info!(
        "Processing incoming schema list request {}",
        payload.request_id
    );

    let op = OperationProcessor::new(node.clone());
    let db = match op.get_db_public() {
        Ok(db) => db,
        Err(e) => {
            log::warn!("Failed to get database for schema list: {}", e);
            return;
        }
    };

    let schemas: Vec<SchemaInfo> = match db.schema_manager.get_schemas() {
        Ok(all_schemas) => all_schemas
            .values()
            .map(|s| SchemaInfo {
                name: s.name.clone(),
                descriptive_name: s.descriptive_name.clone(),
            })
            .collect(),
        Err(e) => {
            log::warn!("Failed to get schemas: {}", e);
            return;
        }
    };

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
            log::warn!("Invalid reply public key in schema list request");
            return;
        }
    };

    let encrypted = match connection::encrypt_message(&reply_pk_bytes, &response) {
        Ok(e) => e,
        Err(e) => {
            log::warn!("Failed to encrypt schema list response: {}", e);
            return;
        }
    };

    let sender_pseudonym: uuid::Uuid = match payload.sender_pseudonym.parse() {
        Ok(u) => u,
        Err(_) => {
            log::warn!("Invalid sender pseudonym in schema list request");
            return;
        }
    };

    let encrypted_b64 = B64.encode(&encrypted);
    if let Err(e) = publisher
        .connect(sender_pseudonym, encrypted_b64, Some(our_pseudonym))
        .await
    {
        log::warn!("Failed to send schema list response: {}", e);
    }
}

/// Handle an incoming schema list response: update local async query.
async fn handle_incoming_schema_list_response(
    store: &dyn fold_db::storage::traits::KvStore,
    payload: &SchemaListResponsePayload,
) {
    log::info!(
        "Received schema list response for request {}",
        payload.request_id
    );

    let results = serde_json::to_value(&payload.schemas).unwrap_or_default();
    if let Err(e) =
        async_query::update_async_query_result(store, &payload.request_id, Some(results), None)
            .await
    {
        log::warn!("Failed to update schema list result: {}", e);
    }
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
    let metadata_store = db_ops.metadata_store().inner().clone();

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

/// Request to sync calendar events for sharing.
#[derive(Debug, Clone, Deserialize)]
pub struct SyncCalendarEventsRequest {
    pub events: Vec<CalendarEventInput>,
}

/// A single calendar event from the client.
#[derive(Debug, Clone, Deserialize)]
pub struct CalendarEventInput {
    pub summary: String,
    pub start_time: String,
    pub end_time: String,
    pub location: String,
    pub calendar: String,
}

/// Request to store peer event fingerprints (received from a connection).
#[derive(Debug, Clone, Deserialize)]
pub struct StorePeerEventsRequest {
    pub peer_pseudonym: String,
    pub fingerprints: Vec<EventFingerprint>,
}

/// Response for calendar sharing status.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(
    feature = "ts-bindings",
    ts(export, export_to = "src/fold_node/static-react/src/types/")
)]
pub struct CalendarSharingStatusResponse {
    pub opted_in: bool,
    pub local_event_count: usize,
    pub peer_count: usize,
}

/// Response for syncing calendar events.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(
    feature = "ts-bindings",
    ts(export, export_to = "src/fold_node/static-react/src/types/")
)]
pub struct SyncCalendarEventsResponse {
    pub synced_count: usize,
}

/// Response for shared events detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(
    feature = "ts-bindings",
    ts(export, export_to = "src/fold_node/static-react/src/types/")
)]
pub struct SharedEventsResponse {
    pub shared_events: Vec<SharedEvent>,
    pub connection_count: usize,
}

/// Get calendar sharing opt-in status.
pub async fn calendar_sharing_status(
    node: &FoldNode,
) -> HandlerResult<CalendarSharingStatusResponse> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    let opted_in = calendar_sharing::is_opted_in(&*store)
        .await
        .handler_err("check calendar sharing opt-in")?;

    let local_events = calendar_sharing::load_local_events(&*store)
        .await
        .handler_err("load local events")?;

    let peer_sets = calendar_sharing::load_all_peer_events(&*store)
        .await
        .handler_err("load peer events")?;

    Ok(ApiResponse::success(CalendarSharingStatusResponse {
        opted_in,
        local_event_count: local_events.len(),
        peer_count: peer_sets.len(),
    }))
}

/// Opt in to calendar sharing.
pub async fn calendar_sharing_opt_in(
    node: &FoldNode,
) -> HandlerResult<CalendarSharingStatusResponse> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    calendar_sharing::set_opt_in(&*store, true)
        .await
        .handler_err("opt in to calendar sharing")?;

    let local_events = calendar_sharing::load_local_events(&*store)
        .await
        .handler_err("load local events")?;

    let peer_sets = calendar_sharing::load_all_peer_events(&*store)
        .await
        .handler_err("load peer events")?;

    Ok(ApiResponse::success(CalendarSharingStatusResponse {
        opted_in: true,
        local_event_count: local_events.len(),
        peer_count: peer_sets.len(),
    }))
}

/// Opt out of calendar sharing.
pub async fn calendar_sharing_opt_out(
    node: &FoldNode,
) -> HandlerResult<CalendarSharingStatusResponse> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    calendar_sharing::set_opt_in(&*store, false)
        .await
        .handler_err("opt out of calendar sharing")?;

    Ok(ApiResponse::success(CalendarSharingStatusResponse {
        opted_in: false,
        local_event_count: 0,
        peer_count: 0,
    }))
}

/// Sync calendar events — fingerprint and store locally.
pub async fn sync_calendar_events(
    req: &SyncCalendarEventsRequest,
    node: &FoldNode,
) -> HandlerResult<SyncCalendarEventsResponse> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    let opted_in = calendar_sharing::is_opted_in(&*store)
        .await
        .handler_err("check calendar sharing opt-in")?;

    if !opted_in {
        return Err(HandlerError::BadRequest(
            "Calendar sharing is not enabled. Opt in first.".to_string(),
        ));
    }

    let fingerprints: Vec<EventFingerprint> = req
        .events
        .iter()
        .map(|e| {
            calendar_sharing::fingerprint_event(
                &e.summary,
                &e.start_time,
                &e.end_time,
                &e.location,
                &e.calendar,
            )
        })
        .collect();

    let count = calendar_sharing::save_local_events(&*store, &fingerprints)
        .await
        .handler_err("save local events")?;

    Ok(ApiResponse::success(SyncCalendarEventsResponse {
        synced_count: count,
    }))
}

/// Store event fingerprints received from a peer connection.
pub async fn store_peer_events(
    req: &StorePeerEventsRequest,
    node: &FoldNode,
) -> HandlerResult<SyncCalendarEventsResponse> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    let opted_in = calendar_sharing::is_opted_in(&*store)
        .await
        .handler_err("check calendar sharing opt-in")?;

    if !opted_in {
        return Err(HandlerError::BadRequest(
            "Calendar sharing is not enabled. Opt in first.".to_string(),
        ));
    }

    // Verify the peer is an accepted connection
    let connections = calendar_sharing::get_accepted_connections(&*store)
        .await
        .handler_err("load accepted connections")?;

    if !connections.contains(&req.peer_pseudonym) {
        return Err(HandlerError::BadRequest(
            "Peer is not an accepted connection.".to_string(),
        ));
    }

    let peer_set = PeerEventSet {
        peer_pseudonym: req.peer_pseudonym.clone(),
        fingerprints: req.fingerprints.clone(),
        updated_at: chrono::Utc::now().to_rfc3339(),
    };

    calendar_sharing::save_peer_events(&*store, &peer_set)
        .await
        .handler_err("save peer events")?;

    Ok(ApiResponse::success(SyncCalendarEventsResponse {
        synced_count: peer_set.fingerprints.len(),
    }))
}

/// Detect shared events between local calendar and peer calendars.
pub async fn get_shared_events(node: &FoldNode) -> HandlerResult<SharedEventsResponse> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    let opted_in = calendar_sharing::is_opted_in(&*store)
        .await
        .handler_err("check calendar sharing opt-in")?;

    if !opted_in {
        return Ok(ApiResponse::success(SharedEventsResponse {
            shared_events: Vec::new(),
            connection_count: 0,
        }));
    }

    let local_events = calendar_sharing::load_local_events(&*store)
        .await
        .handler_err("load local events")?;

    let peer_sets = calendar_sharing::load_all_peer_events(&*store)
        .await
        .handler_err("load peer events")?;

    let connection_count = peer_sets.len();
    let shared_events = calendar_sharing::detect_shared_events(&local_events, &peer_sets);

    Ok(ApiResponse::success(SharedEventsResponse {
        shared_events,
        connection_count,
    }))
}

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
#[derive(Debug, Clone, Serialize)]
pub struct FaceEntry {
    pub face_index: usize,
}

/// Response for listing faces on a record.
#[derive(Debug, Clone, Serialize)]
pub struct ListFacesResponse {
    pub faces: Vec<FaceEntry>,
}

/// List all face embeddings stored for a specific record.
/// Returns face indices (without full embedding vectors).
pub async fn list_faces(
    node: &FoldNode,
    schema: &str,
    key: &str,
) -> HandlerResult<ListFacesResponse> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;

    let db_ops = db.get_db_ops();
    let native_index_mgr = db_ops
        .native_index_manager()
        .ok_or_else(|| HandlerError::Internal("Native index not available".to_string()))?;

    let key_value = fold_db::schema::types::key_value::KeyValue::new(Some(key.to_string()), None);
    let faces = native_index_mgr.list_faces(schema, &key_value);

    let entries: Vec<FaceEntry> = faces
        .into_iter()
        .map(|(idx, _embedding)| FaceEntry { face_index: idx })
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

    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;

    let db_ops = db.get_db_ops();
    let native_index_mgr = db_ops
        .native_index_manager()
        .ok_or_else(|| HandlerError::Internal("Native index not available".to_string()))?;

    let key_value =
        fold_db::schema::types::key_value::KeyValue::new(Some(req.source_key.clone()), None);
    let faces = native_index_mgr.list_faces(&req.source_schema, &key_value);

    let (_idx, embedding) = faces
        .into_iter()
        .find(|(idx, _)| *idx == face_index)
        .ok_or_else(|| {
            HandlerError::NotFound(format!(
                "No face at index {} for schema='{}' key='{}'",
                face_index, req.source_schema, req.source_key
            ))
        })?;

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
            .schema_manager
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
        let result_map = db.query_executor.query(query).await;
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

/// Handle an incoming referral query: check if we know the subject and respond if so.
async fn handle_incoming_referral_query(
    node: &FoldNode,
    payload: &ReferralQueryPayload,
    master_key: &[u8],
    publisher: &DiscoveryPublisher,
) {
    // Load contact book
    let op = OperationProcessor::new(node.clone());
    let book_path = match op.contact_book_path() {
        Ok(p) => p,
        Err(e) => {
            log::warn!("Failed to resolve contacts path for referral query: {e}");
            return;
        }
    };
    let contact_book = ContactBook::load_from(&book_path).unwrap_or_default();

    // Check if we know the subject
    let active = contact_book.active_contacts();
    let matched_contact = active.iter().find(|c| {
        c.pseudonym.as_deref() == Some(&payload.subject_pseudonym)
            || c.messaging_pseudonym.as_deref() == Some(&payload.subject_pseudonym)
            || c.messaging_public_key.as_deref() == Some(&payload.subject_public_key)
    });

    let contact = match matched_contact {
        Some(c) => (*c).clone(),
        None => return, // Silence = no
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
    };

    // Decode the querier's reply public key
    let reply_pk_bytes = match B64.decode(&payload.reply_public_key) {
        Ok(b) if b.len() == 32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&b);
            arr
        }
        Ok(_) => {
            log::warn!("Referral query reply key wrong length");
            return;
        }
        Err(e) => {
            log::warn!("Invalid referral query reply key: {e}");
            return;
        }
    };

    let encrypted = match connection::encrypt_message(&reply_pk_bytes, &response) {
        Ok(e) => e,
        Err(e) => {
            log::warn!("Failed to encrypt referral response: {e}");
            return;
        }
    };

    let encrypted_b64 = B64.encode(&encrypted);

    let sender_uuid: uuid::Uuid = match payload.sender_pseudonym.parse() {
        Ok(u) => u,
        Err(e) => {
            log::warn!("Invalid sender pseudonym UUID in referral query: {e}");
            return;
        }
    };

    if let Err(e) = publisher
        .connect(sender_uuid, encrypted_b64, Some(our_pseudonym))
        .await
    {
        log::warn!("Failed to send referral response: {e}");
    } else {
        log::info!(
            "Sent referral response for query {} (known as {})",
            payload.query_id,
            contact.display_name
        );
    }
}

/// Handle an incoming referral response: append the vouch to the connection request.
async fn handle_incoming_referral_response(
    node: &FoldNode,
    store: &dyn fold_db::storage::traits::KvStore,
    payload: &ReferralResponsePayload,
) {
    // Scan for the connection request matching this query_id
    let entries = match store.scan_prefix(b"discovery:conn_req:").await {
        Ok(e) => e,
        Err(e) => {
            log::warn!("Failed to scan connection requests for referral response: {e}");
            return;
        }
    };

    let mut found_key: Option<Vec<u8>> = None;
    let mut found_req: Option<LocalConnectionRequest> = None;
    for (key, value) in &entries {
        if let Ok(local_req) = serde_json::from_slice::<LocalConnectionRequest>(value) {
            if local_req.referral_query_id.as_deref() == Some(&payload.query_id) {
                found_key = Some(key.clone());
                found_req = Some(local_req);
                break;
            }
        }
    }

    let (sled_key, mut local_req) = match (found_key, found_req) {
        (Some(k), Some(r)) => (k, r),
        _ => {
            log::warn!("Referral response for unknown query {}", payload.query_id);
            return;
        }
    };

    // Look up voucher identity from contact book
    let voucher_display_name = {
        let op = OperationProcessor::new(node.clone());
        let book_path = op.contact_book_path().ok();
        let contact_book = book_path
            .and_then(|p| ContactBook::load_from(&p).ok())
            .unwrap_or_default();

        contact_book
            .active_contacts()
            .iter()
            .find(|c| {
                c.messaging_pseudonym.as_deref() == Some(&payload.sender_pseudonym)
                    || c.pseudonym.as_deref() == Some(&payload.sender_pseudonym)
            })
            .map(|c| c.display_name.clone())
            .unwrap_or_else(|| "Unknown contact".to_string())
    };

    local_req.vouches.push(Vouch {
        voucher_display_name,
        known_as: payload.known_as.clone(),
        received_at: chrono::Utc::now().to_rfc3339(),
    });

    let updated = match serde_json::to_vec(&local_req) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("Failed to serialize updated connection request: {e}");
            return;
        }
    };

    if let Err(e) = store.put(&sled_key, updated).await {
        log::warn!("Failed to save updated connection request with vouch: {e}");
    } else {
        log::info!(
            "Added vouch for referral query {} (known as '{}')",
            payload.query_id,
            payload.known_as
        );
    }
}

/// Process a received data share: create schemas if needed, write mutations,
/// and save any included file data.
async fn process_data_share(
    node: &FoldNode,
    payload: &DataSharePayload,
) -> Result<(), HandlerError> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to get db: {e}")))?;

    for record in &payload.records {
        // 1. Create schema if it doesn't exist
        if let Some(ref schema_def) = record.schema_definition {
            let schema_json = serde_json::to_string(schema_def)
                .map_err(|e| HandlerError::Internal(format!("Schema serialization: {e}")))?;

            // Try to load schema — if it already exists, load_schema_from_json returns an error (that's fine)
            match db.schema_manager.load_schema_from_json(&schema_json).await {
                Ok(_) => {
                    log::info!("Created schema '{}' from data share", record.schema_name);
                    // Auto-approve shared schemas so mutations can be written
                    if let Err(e) = db
                        .schema_manager
                        .set_schema_state(
                            &record.schema_name,
                            fold_db::schema::SchemaState::Approved,
                        )
                        .await
                    {
                        log::warn!(
                            "Failed to approve shared schema '{}': {}",
                            record.schema_name,
                            e
                        );
                    }
                }
                Err(e) => {
                    log::debug!(
                        "Schema '{}' load result (may already exist): {}",
                        record.schema_name,
                        e
                    );
                    // Ensure schema is approved even if it already existed
                    if let Err(approve_err) = db
                        .schema_manager
                        .set_schema_state(
                            &record.schema_name,
                            fold_db::schema::SchemaState::Approved,
                        )
                        .await
                    {
                        log::warn!(
                            "Failed to approve existing schema '{}': {}",
                            record.schema_name,
                            approve_err
                        );
                    }
                }
            }
        }

        // 2. Write the mutation with the sender's pub_key
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

        if let Err(e) = db
            .mutation_manager
            .write_mutations_batch_async(vec![mutation])
            .await
        {
            log::warn!(
                "Failed to write shared record for schema '{}': {}",
                record.schema_name,
                e
            );
        }

        // 3. If file data is included, save it to upload storage
        if let Some(ref file_b64) = record.file_data_base64 {
            match B64.decode(file_b64) {
                Ok(file_bytes) => {
                    let file_name = record
                        .file_name
                        .as_deref()
                        .or_else(|| record.fields.get("file_hash").and_then(|v| v.as_str()))
                        .unwrap_or("shared_file");

                    let upload_path = std::env::var("FOLDDB_UPLOAD_PATH")
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|_| std::path::PathBuf::from("data/uploads"));
                    let upload_storage = fold_db::storage::UploadStorage::local(upload_path);

                    if let Err(e) = upload_storage.save_file(file_name, &file_bytes, None).await {
                        log::warn!("Failed to save shared file '{}': {}", file_name, e);
                    }

                    // 4. Run face detection on shared photos
                    #[cfg(feature = "face-detection")]
                    {
                        let db_ops = db.get_db_ops();
                        if let Some(native_idx) = db_ops.native_index_manager() {
                            if native_idx.has_face_processor() {
                                match native_idx
                                    .index_faces(&record.schema_name, &key, &file_bytes)
                                    .await
                                {
                                    Ok(count) if count > 0 => {
                                        log::info!(
                                            "Detected {} face(s) in shared photo '{}'",
                                            count,
                                            file_name
                                        );
                                    }
                                    Ok(_) => {}
                                    Err(e) => {
                                        log::warn!("Face detection failed on shared photo: {}", e);
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    log::warn!("Failed to decode shared file data: {}", e);
                }
            }
        }
    }

    // 5. Store a notification for the UI
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
    if let Err(e) = store
        .put(
            notif_key.as_bytes(),
            serde_json::to_vec(&notification).unwrap_or_default(),
        )
        .await
    {
        log::warn!("Failed to store data share notification: {}", e);
    }

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

// === Notification handlers ===

/// List all notifications stored in the metadata store.
pub async fn list_notifications(node: &FoldNode) -> HandlerResult<serde_json::Value> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {e}")))?;
    let store = get_metadata_store(&db);

    let entries = store
        .scan_prefix(b"notification:")
        .await
        .map_err(|e| HandlerError::Internal(format!("Failed to scan notifications: {e}")))?;

    let notifications: Vec<serde_json::Value> = entries
        .iter()
        .filter_map(|(key, value)| {
            let key_str = String::from_utf8_lossy(key);
            let mut notif: serde_json::Value = serde_json::from_slice(value).ok()?;
            notif
                .as_object_mut()?
                .insert("id".to_string(), serde_json::json!(key_str));
            Some(notif)
        })
        .collect();

    Ok(ApiResponse::success(serde_json::json!({
        "notifications": notifications,
        "count": notifications.len(),
    })))
}

/// Return the count of notifications without loading all bodies.
pub async fn notification_count(node: &FoldNode) -> HandlerResult<serde_json::Value> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {e}")))?;
    let store = get_metadata_store(&db);
    let entries = store
        .scan_prefix(b"notification:")
        .await
        .map_err(|e| HandlerError::Internal(format!("Failed to scan notifications: {e}")))?;
    Ok(ApiResponse::success(serde_json::json!({
        "count": entries.len(),
    })))
}

/// Dismiss (delete) a single notification by its ID.
pub async fn dismiss_notification(
    node: &FoldNode,
    notification_id: &str,
) -> HandlerResult<serde_json::Value> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {e}")))?;
    let store = get_metadata_store(&db);

    store
        .delete(notification_id.as_bytes())
        .await
        .map_err(|e| HandlerError::Internal(format!("Failed to dismiss notification: {e}")))?;

    Ok(ApiResponse::success(serde_json::json!({"dismissed": true})))
}
