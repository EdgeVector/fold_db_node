//! Inbox storage for Identity Cards received over the messaging
//! layer (Phase 3 receive half — companion to the send half in
//! `async_query::IdentityCardMessagePayload` / `/api/remote/send-
//! identity-card`).
//!
//! Shape mirrors `LocalAsyncQuery` deliberately so the two feeds
//! feel consistent in the UI. Each row lives under the key
//! `received_card:<message_id>`; message_id is the UUID the sender
//! put on the payload, which gives us free idempotency (replay =
//! no-op).
//!
//! Ingest → Accept flow:
//!
//!   1. Poll loop decrypts an `identity_card_send` payload and calls
//!      `save_received_card` with `status = "pending"`. The raw
//!      card JSON is stored as-is; the backend does NOT verify the
//!      signature on ingest.
//!   2. The user opens the Received panel, sees a new row, clicks
//!      Accept. The HTTP handler loads the row, feeds the card JSON
//!      into the existing `import_identity_card` handler (same
//!      Ed25519 verifier paste + QR imports use), and on success
//!      flips status to "accepted".
//!   3. A Dismiss action flips status to "dismissed" without
//!      running the importer. Underlying Identity records are never
//!      written for dismissed rows.
//!
//! Why defer signature verification to Accept? Three reasons:
//! - Keeps the poll loop fast — a bad card can't stall the inbox.
//! - Gives the user a choice: seeing "Alice wants to send her card"
//!   before cryptography is meaningful UX; auto-importing on
//!   receive would land silent identities on the node.
//! - Matches the paste/QR flow — those also defer verification
//!   until the user commits.

use fold_db::storage::traits::KvStore;
use serde::{Deserialize, Serialize};

const RECEIVED_CARD_PREFIX: &str = "received_card:";

/// One inbox row. `message_id` is the UUID from the sender; the
/// Sled key is `received_card:<message_id>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalReceivedCard {
    /// UUID from the `IdentityCardMessagePayload`. Primary key.
    pub message_id: String,
    /// Raw card payload, as received. Same shape as
    /// `MyIdentityCardResponse` — the verifier on Accept will
    /// parse this and check the signature.
    pub card: serde_json::Value,
    /// Ed25519 pubkey the sender claimed. On Accept we also
    /// verify `card.pub_key == sender_public_key` to block the
    /// "Bob forwards Alice's card under his pseudonym" trick.
    pub sender_public_key: String,
    /// Bulletin-board pseudonym the sender used. Purely audit
    /// metadata — the UI can render "received via pseudonym X" so
    /// the user can spot weirdness.
    pub sender_pseudonym: String,
    /// "pending", "accepted", "dismissed".
    pub status: String,
    /// RFC3339 timestamp of when we wrote this row locally.
    pub received_at: String,
    /// RFC3339 timestamp of when the user resolved the row
    /// (accepted or dismissed). `None` while pending.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<String>,
    /// Populated on Accept: the `identity_id` written by the
    /// verifier. Lets the UI link directly to the Identities tab
    /// row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepted_identity_id: Option<String>,
    /// Populated on verify failure: the error message returned by
    /// the importer. Row stays pending so the user can retry
    /// (e.g. after re-receiving a corrected card) or dismiss it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Persist a newly-received card as a pending inbox row.
/// Idempotent on `message_id`: a replay of the same payload
/// overwrites the row with the same data, which is a no-op from
/// the user's point of view (status stays pending, received_at
/// refreshes). The poll loop is the only caller.
pub async fn save_received_card(
    store: &dyn KvStore,
    card: &LocalReceivedCard,
) -> Result<(), String> {
    let key = format!("{}{}", RECEIVED_CARD_PREFIX, card.message_id);
    let value = serde_json::to_vec(card)
        .map_err(|e| format!("Failed to serialize received card: {}", e))?;
    store
        .put(key.as_bytes(), value)
        .await
        .map_err(|e| format!("Failed to save received card: {}", e))
}

/// List every received card, newest-first by `received_at`.
pub async fn list_received_cards(store: &dyn KvStore) -> Result<Vec<LocalReceivedCard>, String> {
    let entries = store
        .scan_prefix(RECEIVED_CARD_PREFIX.as_bytes())
        .await
        .map_err(|e| format!("Failed to scan received cards: {}", e))?;

    let mut rows: Vec<LocalReceivedCard> = Vec::new();
    for (_key, value) in entries {
        match serde_json::from_slice(&value) {
            Ok(r) => rows.push(r),
            Err(e) => log::warn!("Failed to deserialize received card: {}", e),
        }
    }
    rows.sort_by(|a, b| b.received_at.cmp(&a.received_at));
    Ok(rows)
}

/// Load one card by `message_id`.
pub async fn get_received_card(
    store: &dyn KvStore,
    message_id: &str,
) -> Result<Option<LocalReceivedCard>, String> {
    let key = format!("{}{}", RECEIVED_CARD_PREFIX, message_id);
    let value = store
        .get(key.as_bytes())
        .await
        .map_err(|e| format!("Failed to get received card: {}", e))?;
    match value {
        Some(data) => {
            let row = serde_json::from_slice(&data)
                .map_err(|e| format!("Failed to deserialize received card: {}", e))?;
            Ok(Some(row))
        }
        None => Ok(None),
    }
}

/// Update status + populate resolution metadata. Used by the
/// Accept and Dismiss handlers. Keeps the row in place so the
/// user has an audit trail of every card they've seen.
pub async fn resolve_received_card(
    store: &dyn KvStore,
    message_id: &str,
    new_status: &str,
    accepted_identity_id: Option<String>,
    error: Option<String>,
) -> Result<LocalReceivedCard, String> {
    let key = format!("{}{}", RECEIVED_CARD_PREFIX, message_id);
    let existing = store
        .get(key.as_bytes())
        .await
        .map_err(|e| format!("Failed to load received card: {}", e))?
        .ok_or_else(|| format!("Received card {} not found", message_id))?;
    let mut row: LocalReceivedCard = serde_json::from_slice(&existing)
        .map_err(|e| format!("Failed to deserialize received card: {}", e))?;
    row.status = new_status.to_string();
    row.resolved_at = Some(chrono::Utc::now().to_rfc3339());
    row.accepted_identity_id = accepted_identity_id;
    row.error = error;
    let bytes = serde_json::to_vec(&row)
        .map_err(|e| format!("Failed to serialize received card: {}", e))?;
    store
        .put(key.as_bytes(), bytes)
        .await
        .map_err(|e| format!("Failed to save received card: {}", e))?;
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, received_at: &str) -> LocalReceivedCard {
        LocalReceivedCard {
            message_id: id.to_string(),
            card: serde_json::json!({"pub_key":"pk"}),
            sender_public_key: "pk".to_string(),
            sender_pseudonym: "ps".to_string(),
            status: "pending".to_string(),
            received_at: received_at.to_string(),
            resolved_at: None,
            accepted_identity_id: None,
            error: None,
        }
    }

    #[test]
    fn newest_first_sort() {
        let mut rows = [
            row("a", "2026-01-01T00:00:00Z"),
            row("b", "2026-04-01T00:00:00Z"),
            row("c", "2026-03-01T00:00:00Z"),
        ]
        .to_vec();
        rows.sort_by(|a, b| b.received_at.cmp(&a.received_at));
        assert_eq!(rows[0].message_id, "b");
        assert_eq!(rows[1].message_id, "c");
        assert_eq!(rows[2].message_id, "a");
    }

    #[test]
    fn received_card_key_prefix_is_stable() {
        // Regression lock: if someone renames RECEIVED_CARD_PREFIX
        // the stored rows become orphaned. Tie the constant to a
        // literal check so that kind of drift shows up loudly.
        assert_eq!(RECEIVED_CARD_PREFIX, "received_card:");
    }
}
