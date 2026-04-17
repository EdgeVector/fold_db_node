//! "My Identity Card" view — the exportable, signed card for the
//! node owner, ready for direct peer sharing over the E2E messaging
//! layer (Phase 3 plan per `docs/designs/fingerprints.md`
//! §Identity Card exchange).
//!
//! This endpoint reads the self-Identity record written by
//! `bootstrap_self_identity` at signup. It does NOT regenerate the
//! signature or the card — the card is immutable once issued. If
//! the user later updates their display name or birthday, they
//! issue a *new* card via a rotation flow (design doc §Key
//! rotation) that is out of scope here.
//!
//! ## Endpoint
//!
//! `GET /api/fingerprints/my-identity-card` → `MyIdentityCardResponse`
//!
//! Returns `404` if no self-Identity record exists yet (the user
//! hasn't completed the setup wizard that seeds their IdentityCard).
//!
//! ## Trust boundary
//!
//! The response contains only public card material (pubkey,
//! display_name, birthday, signature, issued_at). No private keys.
//! The signature is Ed25519 over the other fields and is verifiable
//! without any backend interaction — a recipient who receives the
//! card over a trusted channel (QR scan, verified messaging) can
//! verify it standalone.

use std::sync::Arc;

use fold_db::schema::types::field::HashRangeFilter;
use fold_db::schema::types::operations::Query;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::fingerprints::canonical_names;
use crate::fingerprints::keys::identity_id;
use crate::fingerprints::schemas::IDENTITY;
use crate::fold_node::FoldNode;
use crate::handlers::response::{ApiResponse, HandlerError, HandlerResult};

/// Payload returned by `GET /my-identity-card`. Mirrors the Identity
/// schema 1:1 minus the `id` field (which is always `id_<pub_key>`
/// and derivable from `pub_key`). Null optional fields are
/// serialized as `null` so the JSON shape matches the signed card
/// exactly — a recipient can round-trip this response through the
/// same signature verifier the sender would use.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MyIdentityCardResponse {
    pub pub_key: String,
    pub display_name: String,
    pub birthday: Option<String>,
    /// The self-attested face embedding. Today this is always
    /// `null` because `bootstrap_self_identity` doesn't collect one
    /// at signup. A future "take a selfie" setup step would
    /// populate it. Kept in the response shape so the recipient
    /// doesn't have to care whether the sender filled it in.
    pub face_embedding: Option<Vec<f32>>,
    pub node_id: String,
    pub card_signature: String,
    pub issued_at: String,
}

/// Fetch the node owner's self-Identity card.
pub async fn get_my_identity_card(node: Arc<FoldNode>) -> HandlerResult<MyIdentityCardResponse> {
    let canonical = canonical_names::lookup(IDENTITY).map_err(|e| {
        HandlerError::Internal(format!(
            "fingerprints: canonical_names not initialized for '{}': {}",
            IDENTITY, e
        ))
    })?;

    let pub_key = node.get_node_public_key().to_string();
    let self_id = identity_id(&pub_key);

    let processor = crate::fold_node::OperationProcessor::new(node.clone());
    let query = Query {
        schema_name: canonical,
        fields: identity_fields(),
        filter: Some(HashRangeFilter::HashKey(self_id.clone())),
        as_of: None,
        rehydrate_depth: None,
        sort_order: None,
        value_filters: None,
    };
    let records = processor
        .execute_query_json(query)
        .await
        .map_err(|e| HandlerError::Internal(format!("identity query failed: {}", e)))?;

    let record = records.first().ok_or_else(|| {
        HandlerError::NotFound(
            "self-Identity not yet issued — complete the setup wizard first".to_string(),
        )
    })?;
    let fields = record.get("fields").ok_or_else(|| {
        HandlerError::Internal("identity record missing 'fields' envelope".to_string())
    })?;

    Ok(ApiResponse::success(extract_card(fields)?))
}

fn identity_fields() -> Vec<String> {
    vec![
        "pub_key".to_string(),
        "display_name".to_string(),
        "birthday".to_string(),
        "face_embedding".to_string(),
        "node_id".to_string(),
        "card_signature".to_string(),
        "issued_at".to_string(),
    ]
}

fn extract_card(fields: &Value) -> Result<MyIdentityCardResponse, HandlerError> {
    Ok(MyIdentityCardResponse {
        pub_key: string_field(fields, "pub_key")
            .ok_or_else(|| HandlerError::Internal("identity record missing 'pub_key'".into()))?,
        display_name: string_field(fields, "display_name").unwrap_or_default(),
        birthday: optional_string_field(fields, "birthday"),
        face_embedding: fields
            .get("face_embedding")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_f64().map(|f| f as f32))
                    .collect()
            }),
        node_id: string_field(fields, "node_id").unwrap_or_default(),
        card_signature: string_field(fields, "card_signature").unwrap_or_default(),
        issued_at: string_field(fields, "issued_at").unwrap_or_default(),
    })
}

fn string_field(fields: &Value, name: &str) -> Option<String> {
    fields
        .get(name)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Return `None` when the field is missing or JSON null; otherwise
/// the string value. Used for the optional `birthday` field that the
/// Identity schema stores as `OneOf([String, Null])`.
fn optional_string_field(fields: &Value, name: &str) -> Option<String> {
    match fields.get(name) {
        Some(v) if v.is_null() => None,
        Some(v) => v.as_str().map(|s| s.to_string()),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_card_happy_path() {
        let fields = json!({
            "pub_key": "pk_abc",
            "display_name": "Tom Tang",
            "birthday": "1990-04-17",
            "face_embedding": [0.1_f64, 0.2, 0.3],
            "node_id": "pk_abc",
            "card_signature": "sig_xxx",
            "issued_at": "2026-04-14T12:00:00Z",
        });
        let card = extract_card(&fields).unwrap();
        assert_eq!(card.pub_key, "pk_abc");
        assert_eq!(card.display_name, "Tom Tang");
        assert_eq!(card.birthday.as_deref(), Some("1990-04-17"));
        assert_eq!(card.face_embedding.unwrap().len(), 3);
        assert_eq!(card.node_id, "pk_abc");
        assert_eq!(card.card_signature, "sig_xxx");
        assert_eq!(card.issued_at, "2026-04-14T12:00:00Z");
    }

    #[test]
    fn extract_card_handles_null_birthday() {
        let fields = json!({
            "pub_key": "pk_abc",
            "display_name": "Tom",
            "birthday": Value::Null,
            "face_embedding": Value::Null,
            "node_id": "pk_abc",
            "card_signature": "sig",
            "issued_at": "now",
        });
        let card = extract_card(&fields).unwrap();
        assert!(card.birthday.is_none());
        assert!(card.face_embedding.is_none());
    }

    #[test]
    fn extract_card_errors_when_pub_key_missing() {
        let fields = json!({
            "display_name": "Tom",
        });
        let err = extract_card(&fields).unwrap_err();
        match err {
            HandlerError::Internal(msg) => assert!(msg.contains("pub_key")),
            _ => panic!("expected Internal error"),
        }
    }
}
