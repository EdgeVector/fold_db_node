//! "Import Identity Card" — accept an Identity Card issued by a peer
//! node, verify its Ed25519 signature, and commit it to this node as
//! a verified Identity record.
//!
//! This is the Phase 3b companion to Phase 3a's `my_identity_card`
//! GET handler. Phase 3a exports the local node's card; Phase 3b
//! imports a peer's. Together they implement the "Identity Card
//! exchange" flow from `docs/designs/fingerprints.md` (Phase 3).
//!
//! ## Endpoint
//!
//! `POST /api/fingerprints/identity-cards/import` with body:
//!
//! ```json
//! {
//!   "card": {
//!     "pub_key": "<base64 ed25519 pubkey>",
//!     "display_name": "Alice",
//!     "birthday": null,
//!     "face_embedding": null,
//!     "node_id": "<base64 ed25519 pubkey>",
//!     "card_signature": "<base64 ed25519 signature>",
//!     "issued_at": "2026-04-14T12:00:00Z"
//!   },
//!   "link_persona_id": "ps_..."   // optional
//! }
//! ```
//!
//! The handler verifies the `card_signature` by reconstructing the
//! canonical byte sequence that `bootstrap_self_identity` signed on
//! the issuing node and running `ed25519_dalek::Verifier::verify`
//! with `pub_key` as the verifying key. On success it writes two
//! records:
//!
//! 1. **Identity** — the verified card, keyed by `id_<pub_key>`.
//!    Idempotent: if a record already exists at this key the write
//!    is skipped and the existing `identity_id` is returned.
//! 2. **IdentityReceipt** — audit-log entry with
//!    `received_via = "Paste"`, `trust_level = "Attested"` so future
//!    trust decisions can distinguish "I verified this myself" from
//!    "someone handed me this over an unverified channel".
//!
//! If `link_persona_id` is present, the handler then applies a
//! `PersonaPatch { link_identity_id: Some(...) }` to that persona so
//! the Persona detail view immediately renders the verified badge.
//!
//! ## Failure modes (all return `400 BadRequest`)
//!
//! - Malformed base64 on `pub_key` or `card_signature`.
//! - Signature length ≠ 64 bytes.
//! - Pubkey length ≠ 32 bytes.
//! - Signature does not verify against the claimed pub_key over the
//!   canonical bytes — the most common "not really Alice" case.
//! - `link_persona_id` supplied but not found.

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use fold_db::schema::types::field::HashRangeFilter;
use fold_db::schema::types::operations::Query;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::fingerprints::canonical_names;
use crate::fingerprints::keys::{
    edge_id, edge_kind, fingerprint_id_for_face_embedding, fingerprint_id_from_bytes, identity_id,
    kind,
};
use crate::fingerprints::planned_record::PlannedRecord;
use crate::fingerprints::schemas::{EDGE, FINGERPRINT, IDENTITY, IDENTITY_RECEIPT};
use crate::fingerprints::self_identity::IdentityCardPayload;
use crate::fingerprints::writer::write_records;
use crate::fold_node::FoldNode;
use crate::handlers::fingerprints::personas::{
    apply_persona_patch, PersonaDetailResponse, PersonaPatch,
};
use crate::handlers::response::{ApiResponse, HandlerError, HandlerResult};

/// Incoming card body. Shape mirrors `MyIdentityCardResponse` so a
/// node can paste the JSON from another node's `/my-identity-card`
/// response directly into this handler.
#[derive(Debug, Clone, Deserialize)]
pub struct IncomingIdentityCard {
    pub pub_key: String,
    pub display_name: String,
    pub birthday: Option<String>,
    pub face_embedding: Option<Vec<f32>>,
    pub node_id: String,
    pub card_signature: String,
    pub issued_at: String,
}

/// Request body for the import endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct ImportIdentityCardRequest {
    pub card: IncomingIdentityCard,
    /// Optional: a Persona id on this node to link to the imported
    /// Identity so the verified badge renders immediately. Typical
    /// flow is "I paste Alice's card AND mark her existing Alice
    /// Persona as verified" — one round trip.
    pub link_persona_id: Option<String>,
}

/// Response body. `was_already_present` lets the UI distinguish "new
/// card written" from "same card we already had" so a second paste
/// doesn't look like a new event.
#[derive(Debug, Clone, Serialize)]
pub struct ImportIdentityCardResponse {
    pub identity_id: String,
    pub verified: bool,
    pub was_already_present: bool,
    /// The freshly-resolved persona detail when `link_persona_id`
    /// was supplied. `None` when no link was requested.
    pub linked_persona: Option<PersonaDetailResponse>,
}

/// Verify an incoming card, persist it (idempotent), and optionally
/// link it to an existing Persona.
pub async fn import_identity_card(
    node: Arc<FoldNode>,
    req: ImportIdentityCardRequest,
) -> HandlerResult<ImportIdentityCardResponse> {
    // 1. Verify the signature. This is the whole point — without a
    //    valid signature the card is just unverified JSON.
    verify_card_signature(&req.card)?;

    // 2. Check whether the Identity record already exists. We keep
    //    imports idempotent: pasting the same card twice is a no-op,
    //    not an error and not a duplicate write.
    let self_id = identity_id(&req.card.pub_key);
    let was_already_present = identity_exists(&node, &self_id).await?;

    // 3. Write Identity + IdentityReceipt if this is new. If the
    //    card carries a face embedding, also derive a face-kind
    //    Fingerprint and an `identity_face` Edge linking it to the
    //    card's NodePubKey Fingerprint so the graph picks up the
    //    declaration. All records are content-keyed, so re-imports
    //    of the same card collapse into the same primary keys and
    //    overwrite idempotently.
    if !was_already_present {
        let now = chrono::Utc::now().to_rfc3339();
        let mut records: Vec<PlannedRecord> = Vec::with_capacity(4);
        records.push(build_identity_record(&self_id, &req.card));
        records.push(build_identity_receipt_record(&self_id, &now));
        if let Some(face_embedding) = req.card.face_embedding.as_deref() {
            records.extend(build_face_fingerprint_and_edge_records(
                &req.card.pub_key,
                face_embedding,
                &now,
            ));
        }
        write_records(node.clone(), &records).await.map_err(|e| {
            HandlerError::Internal(format!(
                "import_identity_card: failed to persist Identity/IdentityReceipt: {}",
                e
            ))
        })?;
        log::info!(
            "fingerprints.handler: imported Identity Card for pub_key='{}' (display_name='{}', face={})",
            req.card.pub_key,
            req.card.display_name,
            req.card.face_embedding.is_some(),
        );
    } else {
        log::info!(
            "fingerprints.handler: Identity for pub_key='{}' already present — skipping write",
            req.card.pub_key,
        );
    }

    // 4. Link to an existing Persona if the caller asked for it.
    let linked_persona = if let Some(persona_id) = req.link_persona_id {
        let detail = apply_persona_patch(
            node,
            persona_id,
            PersonaPatch {
                link_identity_id: Some(self_id.clone()),
                ..Default::default()
            },
        )
        .await?;
        // apply_persona_patch returns the ApiResponse envelope; we
        // need the inner data for our response payload. A successful
        // response always carries Some(data); an absent payload is a
        // contract bug upstream, so fail loudly rather than papering
        // over it with None.
        Some(detail.data.ok_or_else(|| {
            HandlerError::Internal(
                "apply_persona_patch returned success envelope with no data".to_string(),
            )
        })?)
    } else {
        None
    };

    Ok(ApiResponse::success(ImportIdentityCardResponse {
        identity_id: self_id,
        verified: true,
        was_already_present,
        linked_persona,
    }))
}

// ── Signature verification ──────────────────────────────────────────

fn verify_card_signature(card: &IncomingIdentityCard) -> Result<(), HandlerError> {
    let pub_key_bytes = decode_base64(&card.pub_key, "pub_key")?;
    if pub_key_bytes.len() != 32 {
        return Err(HandlerError::BadRequest(format!(
            "pub_key must decode to exactly 32 bytes (got {})",
            pub_key_bytes.len()
        )));
    }
    let pub_key_array: [u8; 32] = pub_key_bytes
        .as_slice()
        .try_into()
        .expect("length checked to be 32");
    let verifying_key = VerifyingKey::from_bytes(&pub_key_array).map_err(|e| {
        HandlerError::BadRequest(format!("pub_key is not a valid ed25519 point: {}", e))
    })?;

    let sig_bytes = decode_base64(&card.card_signature, "card_signature")?;
    if sig_bytes.len() != 64 {
        return Err(HandlerError::BadRequest(format!(
            "card_signature must decode to exactly 64 bytes (got {})",
            sig_bytes.len()
        )));
    }
    let sig_array: [u8; 64] = sig_bytes
        .as_slice()
        .try_into()
        .expect("length checked to be 64");
    let signature = Signature::from_bytes(&sig_array);

    // Rebuild canonical bytes using the same payload shape + key
    // ordering the issuing node used. Any drift between the two
    // paths silently breaks verification, so we deliberately go
    // through the same IdentityCardPayload struct.
    let payload = IdentityCardPayload {
        pub_key: &card.pub_key,
        display_name: &card.display_name,
        birthday: card.birthday.as_deref(),
        face_embedding: card.face_embedding.as_deref(),
        issued_at: &card.issued_at,
    };
    let bytes = payload.canonical_bytes();

    verifying_key.verify(&bytes, &signature).map_err(|_| {
        HandlerError::BadRequest(
            "card_signature does not verify against pub_key + canonical bytes. \
             The card was either tampered with, issued for a different pub_key, \
             or encoded with a different canonical shape."
                .to_string(),
        )
    })
}

fn decode_base64(value: &str, field_name: &str) -> Result<Vec<u8>, HandlerError> {
    base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|e| HandlerError::BadRequest(format!("{} is not valid base64: {}", field_name, e)))
}

// ── Record builders ────────────────────────────────────────────────

fn build_identity_record(self_id: &str, card: &IncomingIdentityCard) -> PlannedRecord {
    let mut fields: HashMap<String, Value> = HashMap::new();
    fields.insert("id".to_string(), json!(self_id));
    fields.insert("pub_key".to_string(), json!(card.pub_key));
    fields.insert("display_name".to_string(), json!(card.display_name));
    fields.insert(
        "birthday".to_string(),
        card.birthday
            .as_ref()
            .map(|s| json!(s))
            .unwrap_or(Value::Null),
    );
    fields.insert(
        "face_embedding".to_string(),
        card.face_embedding
            .as_ref()
            .map(|e| json!(e))
            .unwrap_or(Value::Null),
    );
    fields.insert("node_id".to_string(), json!(card.node_id));
    fields.insert("card_signature".to_string(), json!(card.card_signature));
    fields.insert("issued_at".to_string(), json!(card.issued_at));
    PlannedRecord::hash(IDENTITY, self_id.to_string(), fields)
}

fn build_identity_receipt_record(identity_id: &str, now: &str) -> PlannedRecord {
    let id = format!("ir_{}", uuid::Uuid::new_v4().simple());
    let mut fields: HashMap<String, Value> = HashMap::new();
    fields.insert("id".to_string(), json!(id));
    fields.insert("identity_id".to_string(), json!(identity_id));
    fields.insert("received_at".to_string(), json!(now));
    // "Paste" = imported via the paste-JSON UI. The peer handed us
    // the card through some out-of-band channel (AirDrop, email, QR);
    // the current node cannot distinguish between those so we use a
    // single bucket. Future channels (messaging, QR verified, etc.)
    // should add new received_via values rather than reusing Paste.
    fields.insert("received_via".to_string(), json!("Paste"));
    fields.insert("received_from".to_string(), Value::Null);
    // "Attested" = the card's signature verified. This is the
    // cryptographic level of trust. Higher tiers (e.g. verified
    // out-of-band via QR + in-person) would bump this to "VerifiedInPerson"
    // when we add such a channel.
    fields.insert("trust_level".to_string(), json!("Attested"));
    PlannedRecord::hash(IDENTITY_RECEIPT, id, fields)
}

/// Build a `Fingerprint(kind="face_embedding")` record for the
/// card's declared face and an `Edge(kind="identity_face")` linking
/// that face Fingerprint to the card's NodePubKey Fingerprint. The
/// NodePubKey Fingerprint id is recomputed the same way
/// `bootstrap_self_identity` writes it on the issuing node, so the
/// edge lands on the same key regardless of import order.
///
/// Both records are content-keyed: re-importing the same card
/// overwrites at the same primary key.
fn build_face_fingerprint_and_edge_records(
    pub_key: &str,
    face_embedding: &[f32],
    now: &str,
) -> Vec<PlannedRecord> {
    let face_fp_id = fingerprint_id_for_face_embedding(face_embedding);
    let node_pub_key_fp_id = fingerprint_id_from_bytes(kind::NODE_PUB_KEY, pub_key.as_bytes());
    let eg_id = edge_id(&face_fp_id, &node_pub_key_fp_id, edge_kind::IDENTITY_FACE);

    let mut face_fields: HashMap<String, Value> = HashMap::new();
    face_fields.insert("id".to_string(), json!(face_fp_id));
    face_fields.insert("kind".to_string(), json!(kind::FACE_EMBEDDING));
    face_fields.insert("value".to_string(), json!(face_embedding));
    face_fields.insert("first_seen".to_string(), json!(now));
    face_fields.insert("last_seen".to_string(), json!(now));

    let mut edge_fields: HashMap<String, Value> = HashMap::new();
    edge_fields.insert("id".to_string(), json!(eg_id));
    edge_fields.insert("a".to_string(), json!(face_fp_id));
    edge_fields.insert("b".to_string(), json!(node_pub_key_fp_id));
    edge_fields.insert("kind".to_string(), json!(edge_kind::IDENTITY_FACE));
    // Weight = 1.0: the card's face is a cryptographically-signed
    // self-declaration by the card's pub_key. It is not a noisy
    // photo-graph coincidence, so we don't discount it.
    edge_fields.insert("weight".to_string(), json!(1.0_f32));
    // `evidence_mention_ids` is required by the Edge schema; the
    // card itself is our only evidence and it isn't a Mention, so
    // send an empty array rather than inventing a pseudo-Mention.
    edge_fields.insert(
        "evidence_mention_ids".to_string(),
        json!(Vec::<String>::new()),
    );
    edge_fields.insert("created_at".to_string(), json!(now));

    vec![
        PlannedRecord::hash(FINGERPRINT, face_fp_id, face_fields),
        PlannedRecord::hash(EDGE, eg_id, edge_fields),
    ]
}

// ── Identity-exists probe ──────────────────────────────────────────

async fn identity_exists(node: &Arc<FoldNode>, identity_id: &str) -> Result<bool, HandlerError> {
    let canonical = canonical_names::lookup(IDENTITY).map_err(|e| {
        HandlerError::Internal(format!(
            "fingerprints: canonical_names not initialized for '{}': {}",
            IDENTITY, e
        ))
    })?;
    let processor = crate::fold_node::OperationProcessor::new(node.clone());
    let query = Query {
        schema_name: canonical,
        fields: vec!["id".to_string()],
        filter: Some(HashRangeFilter::HashKey(identity_id.to_string())),
        as_of: None,
        rehydrate_depth: None,
        sort_order: None,
        value_filters: None,
    };
    let records = processor
        .execute_query_json(query)
        .await
        .map_err(|e| HandlerError::Internal(format!("identity probe query failed: {}", e)))?;
    Ok(!records.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn make_valid_card(signing_key: &SigningKey, display_name: &str) -> IncomingIdentityCard {
        let pub_key = base64::engine::general_purpose::STANDARD
            .encode(signing_key.verifying_key().to_bytes());
        let issued_at = "2026-04-17T12:00:00Z".to_string();
        let payload = IdentityCardPayload {
            pub_key: &pub_key,
            display_name,
            birthday: None,
            face_embedding: None,
            issued_at: &issued_at,
        };
        let sig = signing_key.sign(&payload.canonical_bytes());
        let card_signature = base64::engine::general_purpose::STANDARD.encode(sig.to_bytes());
        IncomingIdentityCard {
            pub_key: pub_key.clone(),
            display_name: display_name.to_string(),
            birthday: None,
            face_embedding: None,
            node_id: pub_key,
            card_signature,
            issued_at,
        }
    }

    #[test]
    fn verify_accepts_valid_card_from_matching_private_key() {
        let seed: [u8; 32] = [3; 32];
        let sk = SigningKey::from_bytes(&seed);
        let card = make_valid_card(&sk, "Alice");
        verify_card_signature(&card).expect("valid card must verify");
    }

    #[test]
    fn verify_rejects_tampered_display_name() {
        let seed: [u8; 32] = [4; 32];
        let sk = SigningKey::from_bytes(&seed);
        let mut card = make_valid_card(&sk, "Alice");
        // Flip the name after the card was signed — the signature no
        // longer matches the canonical bytes for this card.
        card.display_name = "Mallory".to_string();
        let err = verify_card_signature(&card).expect_err("tampered card must not verify");
        match err {
            HandlerError::BadRequest(msg) => assert!(msg.contains("card_signature")),
            _ => panic!("expected BadRequest, got {:?}", err),
        }
    }

    #[test]
    fn verify_rejects_signature_from_wrong_key() {
        let seed_a: [u8; 32] = [5; 32];
        let seed_b: [u8; 32] = [6; 32];
        let sk_a = SigningKey::from_bytes(&seed_a);
        let sk_b = SigningKey::from_bytes(&seed_b);
        // Build the card's content with key A's pub_key but sign it
        // with key B. The result is a card that looks plausible but
        // whose pub_key doesn't match the signer.
        let mut card = make_valid_card(&sk_a, "Alice");
        let payload = IdentityCardPayload {
            pub_key: &card.pub_key,
            display_name: &card.display_name,
            birthday: card.birthday.as_deref(),
            face_embedding: card.face_embedding.as_deref(),
            issued_at: &card.issued_at,
        };
        let sig_b = sk_b.sign(&payload.canonical_bytes());
        card.card_signature = base64::engine::general_purpose::STANDARD.encode(sig_b.to_bytes());
        let err = verify_card_signature(&card).expect_err("wrong-signer card must not verify");
        assert!(matches!(err, HandlerError::BadRequest(_)));
    }

    #[test]
    fn verify_rejects_malformed_pubkey_base64() {
        let card = IncomingIdentityCard {
            pub_key: "not-base64!!!".to_string(),
            display_name: "x".to_string(),
            birthday: None,
            face_embedding: None,
            node_id: "x".to_string(),
            card_signature: "AAAA".to_string(),
            issued_at: "2026-04-17T12:00:00Z".to_string(),
        };
        let err = verify_card_signature(&card).expect_err("malformed base64 must not verify");
        match err {
            HandlerError::BadRequest(msg) => assert!(msg.contains("pub_key")),
            _ => panic!("expected BadRequest, got {:?}", err),
        }
    }

    #[test]
    fn verify_rejects_wrong_length_pubkey() {
        // 31 zero bytes base64-encoded → not a valid ed25519 key.
        let short_pubkey = base64::engine::general_purpose::STANDARD.encode([0u8; 31]);
        let short_sig = base64::engine::general_purpose::STANDARD.encode([0u8; 64]);
        let card = IncomingIdentityCard {
            pub_key: short_pubkey,
            display_name: "x".to_string(),
            birthday: None,
            face_embedding: None,
            node_id: "x".to_string(),
            card_signature: short_sig,
            issued_at: "2026-04-17T12:00:00Z".to_string(),
        };
        let err = verify_card_signature(&card).expect_err("short pubkey must not verify");
        match err {
            HandlerError::BadRequest(msg) => assert!(msg.contains("32 bytes")),
            _ => panic!("expected BadRequest, got {:?}", err),
        }
    }

    #[test]
    fn build_identity_record_has_all_fields() {
        let seed: [u8; 32] = [7; 32];
        let sk = SigningKey::from_bytes(&seed);
        let card = make_valid_card(&sk, "Bob");
        let rec = build_identity_record("id_abc", &card);
        assert_eq!(rec.descriptive_schema, IDENTITY);
        assert_eq!(rec.hash_key, "id_abc");
        assert_eq!(rec.fields.get("pub_key").unwrap(), &json!(card.pub_key));
        assert_eq!(rec.fields.get("display_name").unwrap(), &json!("Bob"));
        assert_eq!(rec.fields.get("node_id").unwrap(), &json!(card.node_id));
        assert_eq!(
            rec.fields.get("card_signature").unwrap(),
            &json!(card.card_signature)
        );
        assert!(rec.fields.get("birthday").unwrap().is_null());
        assert!(rec.fields.get("face_embedding").unwrap().is_null());
    }

    #[test]
    fn face_fingerprint_and_edge_records_use_content_keys_and_identity_face_kind() {
        let embedding: Vec<f32> = (0..8).map(|i| i as f32 * 0.1).collect();
        let records =
            build_face_fingerprint_and_edge_records("pk_abc", &embedding, "2026-04-18T12:00:00Z");
        assert_eq!(records.len(), 2);

        // Fingerprint record.
        let fp = &records[0];
        assert_eq!(fp.descriptive_schema, FINGERPRINT);
        let expected_fp_id = fingerprint_id_for_face_embedding(&embedding);
        assert_eq!(fp.hash_key, expected_fp_id);
        assert_eq!(fp.fields.get("id").unwrap(), &json!(expected_fp_id));
        assert_eq!(fp.fields.get("kind").unwrap(), &json!(kind::FACE_EMBEDDING));
        assert_eq!(fp.fields.get("value").unwrap(), &json!(embedding));

        // Edge record.
        let eg = &records[1];
        assert_eq!(eg.descriptive_schema, EDGE);
        let node_pub_key_fp_id = fingerprint_id_from_bytes(kind::NODE_PUB_KEY, "pk_abc".as_bytes());
        let expected_eg_id = edge_id(
            &expected_fp_id,
            &node_pub_key_fp_id,
            edge_kind::IDENTITY_FACE,
        );
        assert_eq!(eg.hash_key, expected_eg_id);
        assert_eq!(
            eg.fields.get("kind").unwrap(),
            &json!(edge_kind::IDENTITY_FACE)
        );
        // Endpoints are stored verbatim; the edge_id already handles
        // canonical ordering so we don't need to re-sort here.
        assert_eq!(eg.fields.get("a").unwrap(), &json!(expected_fp_id));
        assert_eq!(eg.fields.get("b").unwrap(), &json!(node_pub_key_fp_id));
        assert!((eg.fields.get("weight").unwrap().as_f64().unwrap() - 1.0).abs() < 1e-6);
        assert_eq!(
            eg.fields.get("evidence_mention_ids").unwrap(),
            &json!(Vec::<String>::new())
        );
    }

    #[test]
    fn face_records_are_idempotent_under_repeated_builds() {
        // Same (pub_key, embedding) must produce identical primary
        // keys on every call — that's what makes re-imports of the
        // same card collapse into the same records rather than
        // multiplying the graph.
        let embedding: Vec<f32> = vec![0.5_f32; 512];
        let a =
            build_face_fingerprint_and_edge_records("pk_abc", &embedding, "2026-04-18T12:00:00Z");
        let b =
            build_face_fingerprint_and_edge_records("pk_abc", &embedding, "2026-04-19T12:00:00Z");
        assert_eq!(a[0].hash_key, b[0].hash_key);
        assert_eq!(a[1].hash_key, b[1].hash_key);
    }

    #[test]
    fn build_identity_receipt_uses_paste_and_attested() {
        let rec = build_identity_receipt_record("id_abc", "2026-04-17T12:00:00Z");
        assert_eq!(rec.descriptive_schema, IDENTITY_RECEIPT);
        assert_eq!(rec.fields.get("received_via").unwrap(), &json!("Paste"));
        assert_eq!(rec.fields.get("trust_level").unwrap(), &json!("Attested"));
        assert_eq!(rec.fields.get("identity_id").unwrap(), &json!("id_abc"));
        assert!(rec.fields.get("received_from").unwrap().is_null());
    }
}
