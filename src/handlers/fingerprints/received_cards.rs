//! HTTP handlers for the messaging inbox of Identity Cards.
//!
//! Paired with the send half (#524) and the dispatch arm in
//! `src/handlers/discovery/inbound.rs`. Flow:
//!
//!   1. Poll loop decrypts an `identity_card_send` payload.
//!   2. Dispatch writes a `LocalReceivedCard` row with status=pending.
//!   3. User opens the Received sub-tab, calls
//!      `GET /api/fingerprints/received-cards`, sees the row.
//!   4. User clicks Accept →
//!      `POST /api/fingerprints/received-cards/{id}/accept` runs
//!      the existing `import_identity_card` verifier on the stored
//!      payload. On success the row flips to `accepted` and
//!      `accepted_identity_id` is populated.
//!   5. User clicks Dismiss →
//!      `POST /api/fingerprints/received-cards/{id}/dismiss` flips
//!      the row to `dismissed` without running the verifier. The
//!      underlying Identity record is NEVER written for dismissed
//!      rows.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::discovery::received_card::{self, LocalReceivedCard};
use crate::fold_node::FoldNode;
use crate::handlers::fingerprints::import_identity_card::{
    import_identity_card, ImportIdentityCardRequest, IncomingIdentityCard,
};
use crate::handlers::response::{ApiResponse, HandlerError, HandlerResult};

/// Shape returned by the list endpoint. Mirrors `LocalReceivedCard`
/// 1:1 so the frontend can swap it directly into a row without a
/// translation layer.
#[derive(Debug, Clone, Serialize)]
pub struct ReceivedCardView {
    pub message_id: String,
    pub sender_public_key: String,
    pub sender_pseudonym: String,
    pub status: String,
    pub received_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepted_identity_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Extracted from the stored card JSON so the UI doesn't have
    /// to parse the blob client-side. `null` when the card is
    /// malformed — the row is still shown so the user can dismiss.
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issued_at: Option<String>,
    /// Raw card JSON. Included so the UI can optionally render the
    /// full payload in a collapsible block and so the user can
    /// Copy JSON before accepting.
    pub card: serde_json::Value,
}

impl From<LocalReceivedCard> for ReceivedCardView {
    fn from(row: LocalReceivedCard) -> Self {
        let display_name = row
            .card
            .get("display_name")
            .and_then(|v| v.as_str())
            .map(String::from);
        let issued_at = row
            .card
            .get("issued_at")
            .and_then(|v| v.as_str())
            .map(String::from);
        Self {
            message_id: row.message_id,
            sender_public_key: row.sender_public_key,
            sender_pseudonym: row.sender_pseudonym,
            status: row.status,
            received_at: row.received_at,
            resolved_at: row.resolved_at,
            accepted_identity_id: row.accepted_identity_id,
            error: row.error,
            display_name,
            issued_at,
            card: row.card,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ListReceivedCardsResponse {
    pub received_cards: Vec<ReceivedCardView>,
}

/// Optional body on Accept: lets the user link the verified
/// Identity to an existing Persona in the same round trip — same
/// pattern as the paste / QR import.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AcceptReceivedCardRequest {
    #[serde(default)]
    pub link_persona_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AcceptReceivedCardResponse {
    pub received_card: ReceivedCardView,
    /// The identity_id that got written. Also lives on
    /// `received_card.accepted_identity_id` but surfaced at the
    /// top level so the UI can navigate without reaching in.
    pub identity_id: String,
}

/// GET list. Never errors on empty data; an empty inbox is a valid
/// state.
pub async fn list_received_cards(node: Arc<FoldNode>) -> HandlerResult<ListReceivedCardsResponse> {
    let store = load_metadata_store(&node)?;
    let rows = received_card::list_received_cards(&*store)
        .await
        .map_err(HandlerError::Internal)?;
    let views = rows.into_iter().map(ReceivedCardView::from).collect();
    Ok(ApiResponse::success(ListReceivedCardsResponse {
        received_cards: views,
    }))
}

/// POST accept. Loads the stored card, runs the existing importer
/// (which verifies the Ed25519 signature), and on success flips the
/// row to `accepted`. On verification failure the row stays
/// `pending` and the error is captured on the row so the user can
/// retry or dismiss.
pub async fn accept_received_card(
    node: Arc<FoldNode>,
    message_id: String,
    request: AcceptReceivedCardRequest,
) -> HandlerResult<AcceptReceivedCardResponse> {
    let store = load_metadata_store(&node)?;

    let row = received_card::get_received_card(&*store, &message_id)
        .await
        .map_err(HandlerError::Internal)?
        .ok_or_else(|| {
            HandlerError::NotFound(format!("received card '{}' not found", message_id))
        })?;

    if row.status == "accepted" {
        // Idempotent: if the user double-clicks accept, return the
        // already-stored result rather than running the verifier
        // again. The verifier is idempotent too but this saves a
        // round trip to the Identity schema.
        let identity_id = row.accepted_identity_id.clone().unwrap_or_default();
        return Ok(ApiResponse::success(AcceptReceivedCardResponse {
            received_card: row.into(),
            identity_id,
        }));
    }
    if row.status == "dismissed" {
        return Err(HandlerError::BadRequest(
            "cannot accept a dismissed card; it was rejected earlier".to_string(),
        ));
    }

    // Convert the stored JSON payload into the `IncomingIdentityCard`
    // the existing importer expects. If the payload is malformed we
    // capture that on the row and surface a 400 — no silent drop.
    let incoming: IncomingIdentityCard = serde_json::from_value(row.card.clone()).map_err(|e| {
        HandlerError::BadRequest(format!(
            "stored card payload is not a valid IdentityCard shape: {e}"
        ))
    })?;

    match import_identity_card(
        node.clone(),
        ImportIdentityCardRequest {
            card: incoming,
            link_persona_id: request.link_persona_id,
        },
    )
    .await
    {
        Ok(ok_env) => {
            let data = ok_env.data.ok_or_else(|| {
                HandlerError::Internal(
                    "import_identity_card returned success envelope with no data".into(),
                )
            })?;
            let identity_id = data.identity_id.clone();
            let updated = received_card::resolve_received_card(
                &*store,
                &message_id,
                "accepted",
                Some(identity_id.clone()),
                None,
            )
            .await
            .map_err(HandlerError::Internal)?;
            Ok(ApiResponse::success(AcceptReceivedCardResponse {
                received_card: updated.into(),
                identity_id,
            }))
        }
        Err(err) => {
            // Verification or link failed. Record the error on the
            // row so the user sees why, but leave status=pending
            // so they can retry without losing the entry.
            let msg = err.to_user_message();
            let _ = received_card::resolve_received_card(
                &*store,
                &message_id,
                "pending",
                None,
                Some(msg.clone()),
            )
            .await;
            Err(err)
        }
    }
}

/// POST dismiss. Flips status to "dismissed" without running the
/// verifier. Idempotent: dismissing an already-dismissed row is a
/// no-op. Accepting an already-dismissed row is rejected (see
/// above).
pub async fn dismiss_received_card(
    node: Arc<FoldNode>,
    message_id: String,
) -> HandlerResult<ReceivedCardView> {
    let store = load_metadata_store(&node)?;
    let existing = received_card::get_received_card(&*store, &message_id)
        .await
        .map_err(HandlerError::Internal)?
        .ok_or_else(|| {
            HandlerError::NotFound(format!("received card '{}' not found", message_id))
        })?;
    if existing.status == "accepted" {
        return Err(HandlerError::BadRequest(
            "cannot dismiss an accepted card; unlink via the persona first".to_string(),
        ));
    }
    let updated =
        received_card::resolve_received_card(&*store, &message_id, "dismissed", None, None)
            .await
            .map_err(HandlerError::Internal)?;
    Ok(ApiResponse::success(updated.into()))
}

fn load_metadata_store(
    node: &Arc<FoldNode>,
) -> Result<Arc<dyn fold_db::storage::traits::KvStore>, HandlerError> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("failed to open db: {e}")))?;
    Ok(db.get_db_ops().raw_metadata_store())
}

// Small extension so we can capture the error string without
// pulling in a full `Display` invocation on HandlerError.
impl HandlerError {
    fn to_user_message(&self) -> String {
        match self {
            HandlerError::BadRequest(m) => m.clone(),
            HandlerError::NotFound(m) => m.clone(),
            HandlerError::Internal(m) => m.clone(),
            _ => format!("{self:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(overrides: serde_json::Value) -> LocalReceivedCard {
        let mut base = json!({
            "message_id": "msg_1",
            "card": { "display_name": "Alice", "pub_key": "pk", "issued_at": "2026-04-15T12:00:00Z" },
            "sender_public_key": "pk",
            "sender_pseudonym": "ps",
            "status": "pending",
            "received_at": "2026-04-15T12:00:00Z",
        });
        if let serde_json::Value::Object(map) = overrides {
            if let serde_json::Value::Object(base_map) = &mut base {
                for (k, v) in map {
                    base_map.insert(k, v);
                }
            }
        }
        serde_json::from_value(base).expect("test fixture must deserialize")
    }

    #[test]
    fn view_extracts_display_name_from_card() {
        let view: ReceivedCardView = row(json!({})).into();
        assert_eq!(view.display_name.as_deref(), Some("Alice"));
        assert_eq!(view.issued_at.as_deref(), Some("2026-04-15T12:00:00Z"));
        assert_eq!(view.message_id, "msg_1");
        assert_eq!(view.status, "pending");
    }

    #[test]
    fn view_tolerates_missing_display_name() {
        // Strip display_name to simulate a malformed card payload.
        let mut r = row(json!({}));
        r.card = json!({ "pub_key": "pk" });
        let view: ReceivedCardView = r.into();
        assert!(view.display_name.is_none());
        assert!(view.issued_at.is_none());
    }

    #[test]
    fn to_user_message_prefers_explicit_variants() {
        assert_eq!(
            HandlerError::BadRequest("bad".into()).to_user_message(),
            "bad"
        );
        assert_eq!(
            HandlerError::Internal("boom".into()).to_user_message(),
            "boom"
        );
    }
}
