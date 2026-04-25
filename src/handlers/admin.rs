//! Test-admin handlers for local multi-node integration testing.
//!
//! These endpoints bypass normal trust establishment and are gated behind
//! the `FOLDDB_ENABLE_TEST_ADMIN=1` environment variable. They MUST NOT
//! be enabled in production deployments — they let any caller insert
//! contacts into the local trust book without going through the
//! discovery handshake.
//!
//! Use case: spinning up two local nodes (Alice + Bob) and wiring them
//! together for end-to-end sharing tests without the LLM-driven
//! ingestion + publish + connect handshake.

use crate::discovery::connection::get_pseudonym_public_key_b64;
use crate::fold_node::node::FoldNode;
use crate::handlers::response::{
    require_non_empty, ApiResponse, HandlerError, HandlerResult, IntoHandlerError,
};
use crate::trust::contact_book::{Contact, ContactBook, TrustDirection};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

const ENV_ENABLE: &str = "FOLDDB_ENABLE_TEST_ADMIN";

pub fn test_admin_enabled() -> bool {
    std::env::var(ENV_ENABLE)
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn require_enabled() -> Result<(), HandlerError> {
    if test_admin_enabled() {
        Ok(())
    } else {
        Err(HandlerError::Unauthorized(format!(
            "test-admin endpoints are disabled. Set {}=1 to enable (DO NOT USE IN PRODUCTION).",
            ENV_ENABLE
        )))
    }
}

#[derive(Debug, Deserialize)]
pub struct UpsertContactRequest {
    /// Recipient's Ed25519 public key (base64).
    pub node_public_key: String,
    /// Display name for the contact.
    pub display_name: String,
    /// Recipient's messaging pseudonym (UUID string).
    pub messaging_pseudonym: String,
    /// Recipient's X25519 messaging public key (base64).
    pub messaging_public_key: String,
    /// Optional role to assign in the "personal" domain.
    /// Defaults to "friend" if omitted.
    #[serde(default)]
    pub role: Option<String>,
}

crate::handlers::handler_response! {
    pub struct UpsertContactResponse {
        pub contact_public_key: String,
        pub display_name: String,
    }
}

/// Insert or update a contact directly in the local trust book, bypassing
/// the discovery handshake.
pub async fn upsert_contact(
    req: &UpsertContactRequest,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<UpsertContactResponse> {
    require_enabled()?;

    // Validate inputs
    require_non_empty(&req.node_public_key, "node_public_key cannot be empty")?;
    let _ = Uuid::parse_str(&req.messaging_pseudonym).map_err(|e| {
        HandlerError::BadRequest(format!("messaging_pseudonym must be a valid UUID: {}", e))
    })?;
    require_non_empty(
        &req.messaging_public_key,
        "messaging_public_key cannot be empty",
    )?;

    let role_domain = "personal".to_string();
    let role_name = req.role.clone().unwrap_or_else(|| "friend".to_string());
    let mut roles = HashMap::new();
    roles.insert(role_domain, role_name);

    let contact = Contact {
        public_key: req.node_public_key.clone(),
        display_name: req.display_name.clone(),
        contact_hint: None,
        direction: TrustDirection::Mutual,
        connected_at: Utc::now(),
        pseudonym: Some(req.messaging_pseudonym.clone()),
        messaging_pseudonym: Some(req.messaging_pseudonym.clone()),
        messaging_public_key: Some(req.messaging_public_key.clone()),
        identity_pseudonym: None,
        revoked: false,
        roles,
    };

    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("FoldDB not available: {e}")))?;
    let mut book = ContactBook::load(&db)
        .await
        .handler_err("load contact book")?;
    book.upsert_contact(contact);
    book.save(&db).await.handler_err("save contact book")?;

    log::warn!(
        "test-admin: upserted contact {} ({}) — bypassing discovery handshake",
        req.display_name,
        &req.node_public_key[..8.min(req.node_public_key.len())]
    );

    Ok(ApiResponse::success_with_user(
        UpsertContactResponse {
            contact_public_key: req.node_public_key.clone(),
            display_name: req.display_name.clone(),
        },
        user_hash,
    ))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagingKeyEntry {
    pub pseudonym: String,
    pub messaging_public_key: String,
}

crate::handlers::handler_response! {
    pub struct MyMessagingKeysResponse {
        pub keys: Vec<MessagingKeyEntry>,
    }
}

/// Returns this node's pseudonyms paired with their X25519 messaging public
/// keys. Used by the test-admin flow so a peer node can directly populate
/// its contact book without the discovery handshake.
///
/// This is also gated behind `FOLDDB_ENABLE_TEST_ADMIN=1` because exposing
/// the messaging key set lets anyone bypass the handshake. In production
/// the same keys are exchanged through the encrypted discovery payload
/// after a connection request is accepted.
pub async fn my_messaging_keys(
    user_hash: &str,
    node: &FoldNode,
    master_key: &[u8],
) -> HandlerResult<MyMessagingKeysResponse> {
    require_enabled()?;

    let pseudonyms = crate::handlers::discovery::util::collect_our_pseudonyms(node, master_key)
        .await
        .map_err(|e| HandlerError::Internal(format!("collect pseudonyms: {e}")))?;

    let keys = pseudonyms
        .into_iter()
        .map(|p| MessagingKeyEntry {
            pseudonym: p.to_string(),
            messaging_public_key: get_pseudonym_public_key_b64(master_key, &p),
        })
        .collect();

    Ok(ApiResponse::success_with_user(
        MyMessagingKeysResponse { keys },
        user_hash,
    ))
}
