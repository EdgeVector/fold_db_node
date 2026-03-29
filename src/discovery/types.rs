use serde::{Deserialize, Serialize};
use uuid::Uuid;

fn default_fragment_type() -> String {
    "field".to_string()
}

/// A single entry to upload to the discovery service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryUploadEntry {
    pub pseudonym: Uuid,
    pub embedding: Vec<f32>,
    pub category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_preview: Option<String>,
    #[serde(default = "default_fragment_type")]
    pub fragment_type: String,
    /// X25519 public key for this pseudonym (base64-encoded, 32 bytes)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
}

/// Owner mapping sent alongside upload entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnerEntry {
    pub pseudonym: Uuid,
    pub schema_name: String,
}

/// Batch upload request to the discovery service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryUploadRequest {
    pub entries: Vec<DiscoveryUploadEntry>,
    pub owner_entries: Vec<OwnerEntry>,
}

/// Response from upload endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryUploadResponse {
    pub accepted: usize,
    pub quarantined: usize,
    pub total: usize,
}

/// Search request to the discovery service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverySearchRequest {
    pub embedding: Vec<f32>,
    pub top_k: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category_filter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub similarity_threshold: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
}

/// A single search result from the discovery service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverySearchResult {
    pub pseudonym: Uuid,
    pub similarity: f32,
    pub category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_preview: Option<String>,
}

/// Response from search endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverySearchResponse {
    pub results: Vec<DiscoverySearchResult>,
}

/// Connection request sent to the discovery service (encrypted bulletin board).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryConnectRequest {
    pub target_pseudonym: Uuid,
    pub encrypted_blob: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_pseudonym: Option<Uuid>,
}

/// Opt-out request to the discovery service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryOptOutRequest {
    pub schema_name: String,
}

/// An encrypted message from the discovery bulletin board.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedMessage {
    pub message_id: String,
    pub encrypted_blob: String,
    pub target_pseudonym: String,
    #[serde(default)]
    pub sender_pseudonym: Option<String>,
    pub created_at: String,
}

/// Response from looking up a pseudonym's public key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicKeyResponse {
    pub public_key: String,
}

/// Response from polling encrypted messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedMessagesResponse {
    pub messages: Vec<EncryptedMessage>,
}

/// Legacy: An incoming connection request (polled by the node).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingConnectionRequest {
    pub request_id: String,
    pub target_pseudonym: Uuid,
    pub requester_pseudonym: Uuid,
    pub message: String,
    pub status: String,
    pub created_at: String,
}

/// Legacy: Response from polling connection requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionRequestsResponse {
    pub requests: Vec<IncomingConnectionRequest>,
}

/// Result of publishing a schema's embeddings.
#[derive(Debug, Clone)]
pub struct PublishResult {
    pub accepted: usize,
    pub quarantined: usize,
    pub total: usize,
    pub skipped: usize,
}

/// A category entry returned by the browse/categories endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowseCategory {
    pub category: String,
    pub entry_count: i64,
    pub user_count: i64,
}

/// Response from the browse/categories endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowseCategoriesResponse {
    pub categories: Vec<BrowseCategory>,
}
