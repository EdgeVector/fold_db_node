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
    self, ConnectionPayload, IdentityCardPayload, LocalConnectionRequest, LocalSentRequest,
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
use crate::handlers::response::{ApiResponse, HandlerError, HandlerResult, IntoHandlerError, IntoTypedHandlerError};
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
        .await
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

    let payload = ConnectionPayload {
        message_type: "request".to_string(),
        message: req.message.clone(),
        sender_public_key: sender_pk_b64.clone(),
        sender_pseudonym: sender_pseudonym.to_string(),
        reply_public_key: sender_pk_b64,
        identity_card: None,
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
        .await
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
                if let Err(e) = connection::update_sent_request_status(
                    &*store,
                    &payload.sender_pseudonym,
                    "accepted",
                )
                .await
                {
                    log::warn!("Failed to update sent request: {}", e);
                }

                // Auto-create trust relationship from accepted connection
                if payload.identity_card.is_some() {
                    if let Err(e) =
                        process_accepted_connection(node, &payload, "acquaintance").await
                    {
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
                    log::warn!("Failed to update sent request: {}", e);
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
    let db = match op.get_db_public().await {
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
        .await
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
        publisher
            .connect(sender_pseudonym, encrypted_b64, Some(our_pseudonym))
            .await
            .handler_err("send acceptance response")?;
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
        .await
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
        .await
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
            .search_with_threshold(centroid, 20, Some(cat_name.clone()), None, Some(0.15))
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
        .await
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
        .await
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
        .await
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
        .await
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
        .await
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
        .await
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
        .await
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
        .await
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
        .await
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
        .await
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
        .await
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
        .await
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
        .await
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    let shared_moments = moments::list_shared_moments(&*store)
        .await
        .handler_err("list shared moments")?;

    Ok(ApiResponse::success(SharedMomentsResponse {
        moments: shared_moments,
    }))
}
