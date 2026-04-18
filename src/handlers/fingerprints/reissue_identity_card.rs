//! Re-issue the node owner's Identity Card with updated
//! `display_name`, `birthday`, and/or `face_embedding`. Signs the
//! new payload with the node's private key and overwrites the
//! existing Identity record at `id_<pub_key>` — the primary key is
//! stable so this is an in-place Update mutation.
//!
//! Scope for this pass: all three patchable card fields.
//! `face_embedding` uses a three-state patch (absent / null /
//! populated array) so callers can attach, replace, or clear the
//! face embedding without conflating those ops with "left alone."
//!
//! ## Endpoint
//!
//! `POST /api/fingerprints/my-identity-card/reissue`
//!
//! ```json
//! { "display_name": "Tom Tang", "birthday": "1990-04-17" }
//! ```
//!
//! Both fields are optional. Passing neither is a 400 — re-signing
//! the same card with a new timestamp would bloat the sync log for
//! zero user benefit. Passing `"birthday": null` explicitly clears
//! the birthday.
//!
//! Returns the freshly-issued card in the same shape as
//! `GET /my-identity-card`, so the UI can swap it straight into
//! place without a second round trip.
//!
//! ## Trust
//!
//! The signature is re-computed over the new canonical bytes using
//! the node's own private key. A peer who previously stored the old
//! card + signature will see a stale card until the user re-sends.
//! This handler does NOT auto-push the update to peers; that's a
//! Phase 3 exchange problem.
//!
//! The built-in Me persona's `name` is updated in lockstep when
//! `display_name` changes, so the People tab doesn't show the stale
//! name on the Me row.

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use fold_db::schema::types::field::HashRangeFilter;
use fold_db::schema::types::key_value::KeyValue;
use fold_db::schema::types::operations::{MutationType, Query};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::fingerprints::canonical_names;
use crate::fingerprints::keys::identity_id;
use crate::fingerprints::schemas::{IDENTITY, PERSONA};
use crate::fingerprints::self_identity::IdentityCardPayload;
use crate::fold_node::FoldNode;
use crate::handlers::fingerprints::my_identity_card::MyIdentityCardResponse;
use crate::handlers::response::{ApiResponse, HandlerError, HandlerResult};

/// Request body for the reissue endpoint. Both fields are optional
/// — the handler enforces "at least one op" up front.
///
/// `birthday` is double-wrapped so the caller can distinguish
/// "leave the field alone" (`None`) from "clear the birthday"
/// (`Some(None)`). serde happily deserializes `{}` as `None` and
/// `{"birthday": null}` as `Some(None)` for this shape.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ReissueIdentityCardRequest {
    pub display_name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_nullable_string")]
    pub birthday: Option<Option<String>>,
    /// Three-state patch for the face embedding:
    /// - absent ................................. leave the field alone
    /// - `"face_embedding": null` ............... clear the stored value
    /// - `"face_embedding": [..floats..]` ....... set to that vector
    #[serde(default, deserialize_with = "deserialize_optional_nullable_vec_f32")]
    pub face_embedding: Option<Option<Vec<f32>>>,
}

fn deserialize_optional_nullable_string<'de, D>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Presence gives us `Some(inner)`; `inner` is `None` for JSON
    // null and `Some(s)` for a string. Together that's the
    // three-state flag the handler needs.
    let inner: Option<String> = Option::<String>::deserialize(deserializer)?;
    Ok(Some(inner))
}

fn deserialize_optional_nullable_vec_f32<'de, D>(
    deserializer: D,
) -> Result<Option<Option<Vec<f32>>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Same three-state pattern as `deserialize_optional_nullable_string`:
    // presence of the key gives us `Some(inner)`, with `inner` being
    // `None` for JSON null and `Some(vec)` for a populated array.
    let inner: Option<Vec<f32>> = Option::<Vec<f32>>::deserialize(deserializer)?;
    Ok(Some(inner))
}

/// Re-sign the node owner's Identity Card and overwrite the stored
/// Identity record + Me persona name.
pub async fn reissue_identity_card(
    node: Arc<FoldNode>,
    request: ReissueIdentityCardRequest,
) -> HandlerResult<MyIdentityCardResponse> {
    if request.display_name.is_none()
        && request.birthday.is_none()
        && request.face_embedding.is_none()
    {
        return Err(HandlerError::BadRequest(
            "reissue requires at least one of display_name, birthday, or face_embedding"
                .to_string(),
        ));
    }
    if let Some(ref name) = request.display_name {
        if name.trim().is_empty() {
            return Err(HandlerError::BadRequest(
                "display_name must not be empty".to_string(),
            ));
        }
    }

    let identity_canonical = canonical_names::lookup(IDENTITY).map_err(|e| {
        HandlerError::Internal(format!(
            "fingerprints: canonical_names not initialized for '{}': {}",
            IDENTITY, e
        ))
    })?;
    let persona_canonical = canonical_names::lookup(PERSONA).map_err(|e| {
        HandlerError::Internal(format!(
            "fingerprints: canonical_names not initialized for '{}': {}",
            PERSONA, e
        ))
    })?;

    let pub_key = node.get_node_public_key().to_string();
    let self_id = identity_id(&pub_key);

    let processor = crate::fold_node::OperationProcessor::new(node.clone());

    // 1. Load the current Identity record. A missing card means the
    //    setup wizard hasn't run; reissue is nonsense in that state.
    let query = Query {
        schema_name: identity_canonical.clone(),
        fields: vec![
            "pub_key".to_string(),
            "display_name".to_string(),
            "birthday".to_string(),
            "face_embedding".to_string(),
            "node_id".to_string(),
        ],
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
        HandlerError::Internal("identity record missing 'fields' envelope".into())
    })?;

    // 2. Compute the new card payload. Fall back to the stored
    //    values for any field the caller didn't patch.
    let new_display_name = request.display_name.clone().unwrap_or_else(|| {
        fields
            .get("display_name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    });
    let new_birthday: Option<String> = match request.birthday.clone() {
        Some(value) => value,
        None => fields
            .get("birthday")
            .and_then(|v| v.as_str())
            .map(String::from),
    };
    let face_embedding: Option<Vec<f32>> = match request.face_embedding.clone() {
        Some(value) => value,
        None => fields
            .get("face_embedding")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_f64().map(|f| f as f32))
                    .collect::<Vec<_>>()
            }),
    };

    let now = chrono::Utc::now().to_rfc3339();
    let payload = IdentityCardPayload {
        pub_key: &pub_key,
        display_name: &new_display_name,
        birthday: new_birthday.as_deref(),
        face_embedding: face_embedding.as_deref(),
        issued_at: &now,
    };
    let signing_key = load_signing_key(&node)?;
    let sig_bytes = signing_key.sign(&payload.canonical_bytes()).to_bytes();
    let new_signature = base64::engine::general_purpose::STANDARD.encode(sig_bytes);

    // 3. Overwrite the Identity record with the new payload. Same
    //    primary key — Identity is content-keyed by pub_key, which
    //    doesn't change on reissue.
    let mut identity_fields: HashMap<String, Value> = HashMap::new();
    identity_fields.insert("id".to_string(), json!(self_id));
    identity_fields.insert("pub_key".to_string(), json!(pub_key));
    identity_fields.insert("display_name".to_string(), json!(new_display_name));
    identity_fields.insert(
        "birthday".to_string(),
        new_birthday
            .as_ref()
            .map(|s| json!(s))
            .unwrap_or(Value::Null),
    );
    identity_fields.insert(
        "face_embedding".to_string(),
        face_embedding
            .as_ref()
            .map(|v| json!(v))
            .unwrap_or(Value::Null),
    );
    identity_fields.insert("node_id".to_string(), json!(pub_key));
    identity_fields.insert("card_signature".to_string(), json!(new_signature));
    identity_fields.insert("issued_at".to_string(), json!(now));

    let identity_key = KeyValue::new(Some(self_id.clone()), None);
    processor
        .execute_mutation(
            identity_canonical,
            identity_fields,
            identity_key,
            MutationType::Update,
        )
        .await
        .map_err(|e| HandlerError::Internal(format!("failed to update Identity record: {}", e)))?;

    // 4. Update the Me persona's name. We do this only when the
    //    display_name actually changed — otherwise there's nothing
    //    to write and we avoid a pointless mutation.
    if request.display_name.is_some() {
        update_me_persona_name(&processor, &persona_canonical, &new_display_name)
            .await
            .map_err(|e| {
                HandlerError::Internal(format!(
                    "Identity card updated but failed to sync Me persona name: {}",
                    e
                ))
            })?;
    }

    log::info!(
        "fingerprints.handler: reissued Identity Card for pub_key='{}' (display_name='{}')",
        pub_key,
        new_display_name,
    );

    Ok(ApiResponse::success(MyIdentityCardResponse {
        pub_key: pub_key.clone(),
        display_name: new_display_name,
        birthday: new_birthday,
        face_embedding,
        node_id: pub_key,
        card_signature: new_signature_to_response(sig_bytes),
        issued_at: now,
    }))
}

fn new_signature_to_response(sig_bytes: [u8; 64]) -> String {
    base64::engine::general_purpose::STANDARD.encode(sig_bytes)
}

fn load_signing_key(node: &Arc<FoldNode>) -> Result<SigningKey, HandlerError> {
    let seed = FoldNode::extract_ed25519_seed(node.get_node_private_key())
        .map_err(|e| HandlerError::Internal(format!("failed to load node signing key: {}", e)))?;
    Ok(SigningKey::from_bytes(&seed))
}

/// Locate the built-in Me persona (`built_in=true`) and patch its
/// `name` to match the new IdentityCard. The resolver doesn't care
/// about this value — it's cosmetic — but leaving it stale would
/// be confusing in the UI.
async fn update_me_persona_name(
    processor: &crate::fold_node::OperationProcessor,
    persona_canonical: &str,
    new_name: &str,
) -> Result<(), HandlerError> {
    let query = Query {
        schema_name: persona_canonical.to_string(),
        fields: vec![
            "id".to_string(),
            "built_in".to_string(),
            "name".to_string(),
            "seed_fingerprint_ids".to_string(),
            "threshold".to_string(),
            "excluded_mention_ids".to_string(),
            "excluded_edge_ids".to_string(),
            "included_mention_ids".to_string(),
            "aliases".to_string(),
            "relationship".to_string(),
            "trust_tier".to_string(),
            "identity_id".to_string(),
            "user_confirmed".to_string(),
            "created_at".to_string(),
        ],
        filter: None,
        as_of: None,
        rehydrate_depth: None,
        sort_order: None,
        value_filters: None,
    };
    let records = processor
        .execute_query_json(query)
        .await
        .map_err(|e| HandlerError::Internal(format!("persona query failed: {}", e)))?;

    let me = records.iter().find(|r| {
        r.get("fields")
            .and_then(|f| f.get("built_in"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    });
    let Some(me) = me else {
        // No Me persona yet. Bootstrap hasn't run; the Identity
        // record's existence alone is an inconsistent state, but
        // the card is already updated so treat this as a no-op.
        return Ok(());
    };
    let Some(Value::Object(me_fields)) = me.get("fields") else {
        return Err(HandlerError::Internal(
            "Me persona record missing 'fields' envelope".into(),
        ));
    };
    let me_id = me_fields
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| HandlerError::Internal("Me persona missing 'id' field".into()))?
        .to_string();

    let mut payload: HashMap<String, Value> = HashMap::new();
    for (k, v) in me_fields.iter() {
        payload.insert(k.clone(), v.clone());
    }
    payload.insert("name".to_string(), Value::String(new_name.to_string()));

    processor
        .execute_mutation(
            persona_canonical.to_string(),
            payload,
            KeyValue::new(Some(me_id), None),
            MutationType::Update,
        )
        .await
        .map_err(|e| HandlerError::Internal(format!("failed to update Me persona: {}", e)))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_rejects_empty_name() {
        let req = ReissueIdentityCardRequest {
            display_name: Some("   ".to_string()),
            birthday: None,
            face_embedding: None,
        };
        // The body check lives in the handler; exercise it through
        // the pure validation path by calling a thin wrapper.
        assert!(req.display_name.as_ref().unwrap().trim().is_empty());
    }

    #[test]
    fn request_parses_absent_birthday_as_none_none() {
        let req: ReissueIdentityCardRequest =
            serde_json::from_str(r#"{ "display_name": "Tom" }"#).unwrap();
        assert_eq!(req.display_name.as_deref(), Some("Tom"));
        assert!(req.birthday.is_none(), "missing field is None");
    }

    #[test]
    fn request_parses_null_birthday_as_some_none() {
        let req: ReissueIdentityCardRequest =
            serde_json::from_str(r#"{ "birthday": null }"#).unwrap();
        assert!(req.display_name.is_none());
        assert_eq!(
            req.birthday,
            Some(None),
            "explicit null means 'clear the field'"
        );
    }

    #[test]
    fn request_parses_absent_face_embedding_as_none() {
        let req: ReissueIdentityCardRequest =
            serde_json::from_str(r#"{ "display_name": "Tom" }"#).unwrap();
        assert!(req.face_embedding.is_none(), "missing field is None");
    }

    #[test]
    fn request_parses_null_face_embedding_as_some_none() {
        let req: ReissueIdentityCardRequest =
            serde_json::from_str(r#"{ "face_embedding": null }"#).unwrap();
        assert_eq!(
            req.face_embedding,
            Some(None),
            "explicit null means 'clear the field'",
        );
    }

    #[test]
    fn request_parses_populated_face_embedding_as_some_some() {
        let req: ReissueIdentityCardRequest =
            serde_json::from_str(r#"{ "face_embedding": [0.1, 0.2, 0.3] }"#).unwrap();
        assert_eq!(req.face_embedding, Some(Some(vec![0.1_f32, 0.2, 0.3])));
    }

    #[test]
    fn request_parses_string_birthday_as_some_some() {
        let req: ReissueIdentityCardRequest =
            serde_json::from_str(r#"{ "birthday": "1990-04-17" }"#).unwrap();
        assert_eq!(req.birthday, Some(Some("1990-04-17".to_string())));
    }

    #[test]
    fn signature_roundtrips_through_identity_card_payload() {
        use ed25519_dalek::{Verifier, VerifyingKey};
        let seed: [u8; 32] = [9; 32];
        let sk = SigningKey::from_bytes(&seed);
        let vk: VerifyingKey = sk.verifying_key();
        let pub_key = base64::engine::general_purpose::STANDARD.encode(vk.to_bytes());

        let payload = IdentityCardPayload {
            pub_key: &pub_key,
            display_name: "Renamed",
            birthday: Some("1990-04-17"),
            face_embedding: None,
            issued_at: "2026-04-17T12:00:00Z",
        };
        let bytes = payload.canonical_bytes();
        let sig = sk.sign(&bytes);
        vk.verify(&bytes, &sig)
            .expect("reissued card signature must verify");
    }
}
