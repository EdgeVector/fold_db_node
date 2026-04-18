//! Async inter-node query types and local storage.
//!
//! Query requests and responses travel as encrypted blobs through the
//! messaging_service bulletin board. This module defines the payload types
//! (serialized inside encrypted blobs) and local Sled storage for tracking
//! pending/completed queries.

use fold_db::storage::traits::KvStore;
use serde::{Deserialize, Serialize};

const ASYNC_QUERY_PREFIX: &str = "async_query:";

// ===== Payload types (sent inside encrypted blobs) =====

/// Query request sent to a remote node via messaging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryRequestPayload {
    /// Always "query_request"
    pub message_type: String,
    /// Correlation ID (UUID)
    pub request_id: String,
    /// Schema to query on the remote node
    pub schema_name: String,
    /// Fields to return (empty = all authorized fields)
    pub fields: Vec<String>,
    /// Sender's Ed25519 public key (for access control on the remote side)
    pub sender_public_key: String,
    /// Sender's bulletin board pseudonym (for routing the response)
    pub sender_pseudonym: String,
    /// Sender's X25519 public key (for encrypting the response)
    pub reply_public_key: String,
}

/// Query response sent back from the remote node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResponsePayload {
    /// Always "query_response"
    pub message_type: String,
    /// Correlation ID matching the request
    pub request_id: String,
    /// Whether the query succeeded
    pub success: bool,
    /// Query results (if success)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub results: Option<Vec<serde_json::Value>>,
    /// Error message (if !success)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Responder's pseudonym
    pub sender_pseudonym: String,
    /// Responder's X25519 public key
    pub reply_public_key: String,
}

/// Schema info for remote browsing (matches remote.rs SharedSchemaInfo).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaInfo {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub descriptive_name: Option<String>,
}

/// Schema list request sent to a remote node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaListRequestPayload {
    /// Always "schema_list_request"
    pub message_type: String,
    /// Correlation ID
    pub request_id: String,
    /// Sender's Ed25519 public key
    pub sender_public_key: String,
    /// Sender's bulletin board pseudonym
    pub sender_pseudonym: String,
    /// Sender's X25519 public key
    pub reply_public_key: String,
}

/// Schema list response from the remote node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaListResponsePayload {
    /// Always "schema_list_response"
    pub message_type: String,
    /// Correlation ID
    pub request_id: String,
    /// Available schemas on the remote node
    pub schemas: Vec<SchemaInfo>,
    /// Responder's pseudonym
    pub sender_pseudonym: String,
    /// Responder's X25519 public key
    pub reply_public_key: String,
}

/// Identity Card sent to a remote node via messaging. Wraps the
/// signed card payload (`MyIdentityCardResponse`-shaped JSON) so
/// the recipient can run the exact same signature verification
/// that `POST /api/fingerprints/identity-cards/import` runs on a
/// pasted card.
///
/// This is **one-shot** — unlike query requests there is no
/// reply. The recipient may choose to send their own card back
/// as a separate message, but that's a follow-up not a response.
///
/// The `card` field is a `serde_json::Value` rather than a strongly
/// typed struct so the payload stays compatible across minor card-
/// schema changes: the verifier on the receiving side is the
/// authoritative parser, and it already tolerates the one-of-many
/// shapes we expect.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityCardMessagePayload {
    /// Always "identity_card_send"
    pub message_type: String,
    /// Dedup / audit correlation id (UUID). The recipient's inbox
    /// uses this as the Sled key so a replay of the same message
    /// is a no-op rather than a duplicate entry.
    pub message_id: String,
    /// The signed Identity Card as JSON. Must match the shape of
    /// `MyIdentityCardResponse` on the sender — verifier on the
    /// receiver reconstructs the canonical bytes from these fields.
    pub card: serde_json::Value,
    /// Sender's Ed25519 public key — the same key used to sign the
    /// card. The receiver checks that `card.pub_key == sender_public_key`
    /// before acceptance to prevent "bob sends alice's card" replay.
    pub sender_public_key: String,
    /// Sender's bulletin board pseudonym — the inbox records this so
    /// the UI can show "from Alice" rather than a raw pubkey.
    pub sender_pseudonym: String,
}

// ===== Local storage =====

/// A locally tracked async query (outgoing).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalAsyncQuery {
    pub request_id: String,
    /// Ed25519 public key of the contact we queried
    pub contact_public_key: String,
    /// Display name for UI
    pub contact_display_name: String,
    /// Schema name (None for schema list requests)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_name: Option<String>,
    /// Requested fields
    #[serde(default)]
    pub fields: Vec<String>,
    /// "query" or "schema_list"
    pub query_type: String,
    /// "pending", "completed", "error"
    pub status: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub results: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Save an async query to local store.
pub async fn save_async_query(store: &dyn KvStore, query: &LocalAsyncQuery) -> Result<(), String> {
    let key = format!("{}{}", ASYNC_QUERY_PREFIX, query.request_id);
    let value =
        serde_json::to_vec(query).map_err(|e| format!("Failed to serialize async query: {}", e))?;
    store
        .put(key.as_bytes(), value)
        .await
        .map_err(|e| format!("Failed to save async query: {}", e))
}

/// List all async queries.
pub async fn list_async_queries(store: &dyn KvStore) -> Result<Vec<LocalAsyncQuery>, String> {
    let entries = store
        .scan_prefix(ASYNC_QUERY_PREFIX.as_bytes())
        .await
        .map_err(|e| format!("Failed to scan async queries: {}", e))?;

    let mut queries = Vec::new();
    for (_key, value) in entries {
        match serde_json::from_slice(&value) {
            Ok(q) => queries.push(q),
            Err(e) => log::warn!("Failed to deserialize async query: {}", e),
        }
    }

    queries.sort_by(|a: &LocalAsyncQuery, b: &LocalAsyncQuery| b.created_at.cmp(&a.created_at));
    Ok(queries)
}

/// Get a specific async query by request_id.
pub async fn get_async_query(
    store: &dyn KvStore,
    request_id: &str,
) -> Result<Option<LocalAsyncQuery>, String> {
    let key = format!("{}{}", ASYNC_QUERY_PREFIX, request_id);
    let value = store
        .get(key.as_bytes())
        .await
        .map_err(|e| format!("Failed to get async query: {}", e))?;

    match value {
        Some(data) => {
            let query = serde_json::from_slice(&data)
                .map_err(|e| format!("Failed to deserialize async query: {}", e))?;
            Ok(Some(query))
        }
        None => Ok(None),
    }
}

/// Update an async query with results (or error).
pub async fn update_async_query_result(
    store: &dyn KvStore,
    request_id: &str,
    results: Option<serde_json::Value>,
    error: Option<String>,
) -> Result<(), String> {
    let key = format!("{}{}", ASYNC_QUERY_PREFIX, request_id);
    let value = store
        .get(key.as_bytes())
        .await
        .map_err(|e| format!("Failed to get async query: {}", e))?
        .ok_or_else(|| format!("Async query {} not found", request_id))?;

    let mut query: LocalAsyncQuery = serde_json::from_slice(&value)
        .map_err(|e| format!("Failed to deserialize async query: {}", e))?;

    query.completed_at = Some(chrono::Utc::now().to_rfc3339());
    if let Some(err) = &error {
        query.status = "error".to_string();
        query.error = Some(err.clone());
    } else {
        query.status = "completed".to_string();
        query.results = results;
    }

    let updated = serde_json::to_vec(&query)
        .map_err(|e| format!("Failed to serialize async query: {}", e))?;
    store
        .put(key.as_bytes(), updated)
        .await
        .map_err(|e| format!("Failed to save async query: {}", e))
}

/// Delete an async query from local store.
pub async fn delete_async_query(store: &dyn KvStore, request_id: &str) -> Result<(), String> {
    let key = format!("{}{}", ASYNC_QUERY_PREFIX, request_id);
    store
        .delete(key.as_bytes())
        .await
        .map(|_| ())
        .map_err(|e| format!("Failed to delete async query: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_request_payload_roundtrip() {
        let payload = QueryRequestPayload {
            message_type: "query_request".to_string(),
            request_id: "test-123".to_string(),
            schema_name: "notes".to_string(),
            fields: vec!["title".to_string(), "body".to_string()],
            sender_public_key: "pk_base64".to_string(),
            sender_pseudonym: "pseudo-uuid".to_string(),
            reply_public_key: "rpk_base64".to_string(),
        };

        let json = serde_json::to_string(&payload).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["message_type"].as_str().unwrap(), "query_request");
        assert_eq!(parsed["schema_name"].as_str().unwrap(), "notes");
    }

    #[test]
    fn test_query_response_payload_roundtrip() {
        let payload = QueryResponsePayload {
            message_type: "query_response".to_string(),
            request_id: "test-123".to_string(),
            success: true,
            results: Some(vec![serde_json::json!({"title": "Hello"})]),
            error: None,
            sender_pseudonym: "pseudo-uuid".to_string(),
            reply_public_key: "rpk_base64".to_string(),
        };

        let json = serde_json::to_string(&payload).unwrap();
        let parsed: QueryResponsePayload = serde_json::from_str(&json).unwrap();
        assert!(parsed.success);
        assert_eq!(parsed.results.unwrap().len(), 1);
    }

    #[test]
    fn test_schema_list_payload_roundtrip() {
        let payload = SchemaListResponsePayload {
            message_type: "schema_list_response".to_string(),
            request_id: "test-456".to_string(),
            schemas: vec![
                SchemaInfo {
                    name: "notes".to_string(),
                    descriptive_name: Some("Personal Notes".to_string()),
                },
                SchemaInfo {
                    name: "photos".to_string(),
                    descriptive_name: None,
                },
            ],
            sender_pseudonym: "pseudo-uuid".to_string(),
            reply_public_key: "rpk_base64".to_string(),
        };

        let json = serde_json::to_string(&payload).unwrap();
        let parsed: SchemaListResponsePayload = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.schemas.len(), 2);
        assert_eq!(
            parsed.schemas[0].descriptive_name.as_deref(),
            Some("Personal Notes")
        );
    }
}
