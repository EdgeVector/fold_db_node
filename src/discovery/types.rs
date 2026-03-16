use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A single entry to upload to the discovery service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryUploadEntry {
    pub pseudonym: Uuid,
    pub embedding: Vec<f32>,
    pub category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_preview: Option<String>,
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

/// Connection request sent to the discovery service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryConnectRequest {
    pub target_pseudonym: Uuid,
    pub requester_pseudonym: Uuid,
    pub message: String,
}

/// Opt-out request to the discovery service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryOptOutRequest {
    pub schema_name: String,
}

/// An incoming connection request (polled by the node).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingConnectionRequest {
    pub request_id: String,
    pub target_pseudonym: Uuid,
    pub requester_pseudonym: Uuid,
    pub message: String,
    pub status: String,
    pub created_at: String,
}

/// Response from polling connection requests.
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
