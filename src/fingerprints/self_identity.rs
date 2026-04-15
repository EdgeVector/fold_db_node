//! Self-Identity bootstrap — creates the node owner's identity records
//! at subsystem startup.
//!
//! This is the seed case that validates the whole fingerprint pipeline
//! end-to-end for the owner's own data. It produces four records:
//!
//! 1. **Identity atom** — a signed Identity Card anchored to the node's
//!    Ed25519 pubkey. Same card the node will later hand to peers.
//! 2. **NodePubKey Fingerprint** — the raw identity signal that links
//!    the owner's observed data (photos, notes, emails) to the verified
//!    self-Identity via the resolver's traversal graph.
//! 3. **IdentityReceipt** — audit-log entry for "when this node
//!    verified its own self-Identity." `received_via = Self`,
//!    `trust_level = Self`.
//! 4. **Me Persona** — the built-in Persona everyone starts with. Seeds
//!    with the NodePubKey fingerprint, links to the self-Identity,
//!    `built_in = true` so the backend rejects delete attempts.
//!
//! ## Ordering and transactionality
//!
//! All four records are built into a single `Vec<PlannedRecord>` and
//! written via `writer::write_records` in one call. If any write
//! fails, the caller fails loudly — there is no partial-state
//! recovery path, because a half-bootstrapped node is never a valid
//! state. The caller (node startup sequence) MUST treat this as a
//! startup-gating step.
//!
//! Idempotency: all four records use content-derived primary keys
//! (Identity by pubkey, Fingerprint by sha256 of kind+pubkey,
//! IdentityReceipt and Persona by UUID). Re-running bootstrap on a
//! node that already has its self-Identity is safe for Identity and
//! Fingerprint (upsert by key), but creates fresh UUIDs for the
//! IdentityReceipt and Persona. The caller should only call this
//! once per node lifetime; in practice at first signup.

use ed25519_dalek::{Signer, SigningKey};
use fold_db::error::{FoldDbError, FoldDbResult};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

use crate::fingerprints::canonical_names;
use crate::fingerprints::keys::{fingerprint_id_from_bytes, identity_id, kind};
use crate::fingerprints::planned_record::PlannedRecord;
use crate::fingerprints::schemas::{
    FINGERPRINT, IDENTITY, IDENTITY_RECEIPT, MENTION_BY_FINGERPRINT, PERSONA,
};
use crate::fingerprints::writer::write_records;
use crate::fold_node::FoldNode;

/// Outcome of the self-Identity bootstrap. Callers typically care
/// about `me_persona_id` (for UI wiring) and `self_identity_id`
/// (for trust decisions that pre-date any user input).
#[derive(Debug, Clone)]
pub struct SelfIdentityOutcome {
    /// Descriptive key `id_<pub_key>` of the Identity record.
    pub self_identity_id: String,
    /// The NodePubKey Fingerprint id, used as the Me Persona's seed.
    pub node_pub_key_fingerprint_id: String,
    /// UUID of the Me Persona record.
    pub me_persona_id: String,
    /// UUID of the IdentityReceipt record that logs "this node
    /// verified its own identity on signup."
    pub identity_receipt_id: String,
}

/// The serialized Identity Card payload — the exact bytes the node
/// signs, and the exact bytes a peer re-signs to verify. Kept as a
/// deterministic JSON serialization of the fields so both sides
/// produce the same canonical byte sequence.
///
/// Note: `signature` is NOT included in the signed payload — it's
/// the output. We sign the content-hash of the other fields.
#[derive(Debug, Clone)]
struct IdentityCardPayload<'a> {
    pub_key: &'a str,
    display_name: &'a str,
    birthday: Option<&'a str>,
    face_embedding: Option<&'a [f32]>,
    issued_at: &'a str,
}

impl<'a> IdentityCardPayload<'a> {
    /// Produce the canonical bytes that get signed. Uses
    /// serde_json::to_vec on a deterministic-key-order object.
    fn canonical_bytes(&self) -> Vec<u8> {
        // BTreeMap-like ordering via serde_json — we build the
        // object with sorted keys explicitly.
        let obj = json!({
            "birthday": self.birthday,
            "display_name": self.display_name,
            "face_embedding": self.face_embedding,
            "issued_at": self.issued_at,
            "pub_key": self.pub_key,
        });
        serde_json::to_vec(&obj).expect("canonical_bytes serialization must succeed")
    }
}

/// Bootstrap the node owner's identity records. Safe to call only
/// at first signup — subsequent calls will write duplicate
/// IdentityReceipt and Persona records, which is wasteful but not
/// incorrect.
///
/// `canonical_names::register_phase_1_schemas` MUST have already
/// run on this node, because the writer layer needs the runtime
/// canonical names of every schema we touch.
pub async fn bootstrap_self_identity(
    node: Arc<FoldNode>,
    display_name: String,
) -> FoldDbResult<SelfIdentityOutcome> {
    // Pull identity material from the node. The private key is only
    // held in-scope for the duration of this function.
    let pub_key = node.get_node_public_key().to_string();
    let private_key_b64 = node.get_node_private_key();
    let seed = FoldNode::extract_ed25519_seed(private_key_b64)?;
    let signing_key = SigningKey::from_bytes(&seed);

    let now = chrono::Utc::now().to_rfc3339();

    // Build the card payload and sign it with the node's private key.
    // The signature proves "whoever holds the private key for this
    // pubkey authored these claims" — nothing more. Real-world trust
    // comes from how the card is exchanged out-of-band.
    let card = IdentityCardPayload {
        pub_key: &pub_key,
        display_name: &display_name,
        birthday: None,
        face_embedding: None,
        issued_at: &now,
    };
    let sig_bytes = signing_key.sign(&card.canonical_bytes()).to_bytes();
    let card_signature = base64_encode(&sig_bytes);

    // Keys.
    let self_identity_id = identity_id(&pub_key);
    let node_pub_key_fp_id = fingerprint_id_from_bytes(kind::NODE_PUB_KEY, pub_key.as_bytes());
    let identity_receipt_id = format!("ir_{}", uuid::Uuid::new_v4().simple());
    let me_persona_id = format!("ps_{}", uuid::Uuid::new_v4().simple());

    // Compose records.
    let records = vec![
        identity_record(
            &self_identity_id,
            &pub_key,
            &display_name,
            &card_signature,
            &now,
        ),
        node_pub_key_fingerprint_record(&node_pub_key_fp_id, &pub_key, &now),
        identity_receipt_record(&identity_receipt_id, &self_identity_id, &now),
        me_persona_record(
            &me_persona_id,
            &display_name,
            &node_pub_key_fp_id,
            &self_identity_id,
            &now,
        ),
    ];

    // Verify canonical_names registry is populated before we try to
    // write — surface a clearer error than the writer's "lookup
    // failed" message would give.
    for schema in [IDENTITY, FINGERPRINT, IDENTITY_RECEIPT, PERSONA] {
        canonical_names::lookup(schema).map_err(|_| {
            FoldDbError::Config(format!(
                "bootstrap_self_identity: canonical_names registry missing '{}'. \
                 register_phase_1_schemas() must run before bootstrap.",
                schema
            ))
        })?;
    }

    write_records(node.clone(), &records).await?;

    // Side-effect: drop the unused MENTION_BY_FINGERPRINT import
    // warning since we may reference it in future expansions.
    let _ = MENTION_BY_FINGERPRINT;

    Ok(SelfIdentityOutcome {
        self_identity_id,
        node_pub_key_fingerprint_id: node_pub_key_fp_id,
        me_persona_id,
        identity_receipt_id,
    })
}

// ── Record builders ─────────────────────────────────────────────

fn identity_record(
    id: &str,
    pub_key: &str,
    display_name: &str,
    card_signature: &str,
    now: &str,
) -> PlannedRecord {
    let mut fields = HashMap::new();
    fields.insert("id".to_string(), json!(id));
    fields.insert("pub_key".to_string(), json!(pub_key));
    fields.insert("display_name".to_string(), json!(display_name));
    fields.insert("birthday".to_string(), Value::Null);
    fields.insert("face_embedding".to_string(), Value::Null);
    fields.insert("node_id".to_string(), json!(pub_key));
    fields.insert("card_signature".to_string(), json!(card_signature));
    fields.insert("issued_at".to_string(), json!(now));
    PlannedRecord::hash(IDENTITY, id.to_string(), fields)
}

fn node_pub_key_fingerprint_record(id: &str, pub_key: &str, now: &str) -> PlannedRecord {
    let mut fields = HashMap::new();
    fields.insert("id".to_string(), json!(id));
    fields.insert("kind".to_string(), json!(kind::NODE_PUB_KEY));
    fields.insert("value".to_string(), json!(pub_key));
    fields.insert("first_seen".to_string(), json!(now));
    fields.insert("last_seen".to_string(), json!(now));
    PlannedRecord::hash(FINGERPRINT, id.to_string(), fields)
}

fn identity_receipt_record(id: &str, identity_id: &str, now: &str) -> PlannedRecord {
    let mut fields = HashMap::new();
    fields.insert("id".to_string(), json!(id));
    fields.insert("identity_id".to_string(), json!(identity_id));
    fields.insert("received_at".to_string(), json!(now));
    fields.insert("received_via".to_string(), json!("Self"));
    fields.insert("received_from".to_string(), Value::Null);
    fields.insert("trust_level".to_string(), json!("Self"));
    PlannedRecord::hash(IDENTITY_RECEIPT, id.to_string(), fields)
}

fn me_persona_record(
    id: &str,
    display_name: &str,
    seed_fingerprint_id: &str,
    identity_id: &str,
    now: &str,
) -> PlannedRecord {
    let mut fields = HashMap::new();
    fields.insert("id".to_string(), json!(id));
    fields.insert("name".to_string(), json!(display_name));
    fields.insert(
        "seed_fingerprint_ids".to_string(),
        json!([seed_fingerprint_id]),
    );
    fields.insert("threshold".to_string(), json!(0.9_f32));
    fields.insert(
        "excluded_mention_ids".to_string(),
        json!(Vec::<String>::new()),
    );
    fields.insert("excluded_edge_ids".to_string(), json!(Vec::<String>::new()));
    fields.insert(
        "included_mention_ids".to_string(),
        json!(Vec::<String>::new()),
    );
    fields.insert("aliases".to_string(), json!(Vec::<String>::new()));
    fields.insert("relationship".to_string(), json!("self"));
    fields.insert("trust_tier".to_string(), json!(4));
    // The Persona schema declares `identity_id` as
    // `OneOf([SchemaRef("Identity"), Null])`, which requires a
    // reference-object shape: `{"schema": "Identity", "key": "..."}`.
    // fold_db's SchemaRef validator rejects a bare string. The
    // resolver doesn't care about the shape — it never reads this
    // field — but the writer path has to satisfy the schema.
    //
    // `schema` is the DESCRIPTIVE schema name `"Identity"`, not the
    // canonical identity_hash, because that's what the SchemaRef
    // variant in the schema definition carries.
    fields.insert(
        "identity_id".to_string(),
        json!({ "schema": "Identity", "key": identity_id }),
    );
    fields.insert("user_confirmed".to_string(), json!(true));
    fields.insert("built_in".to_string(), json!(true));
    fields.insert("created_at".to_string(), json!(now));
    PlannedRecord::hash(PERSONA, id.to_string(), fields)
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Canonical payload determinism ──────────────────────────

    #[test]
    fn canonical_bytes_are_deterministic_for_same_inputs() {
        let a = IdentityCardPayload {
            pub_key: "pk_abc",
            display_name: "Tom Tang",
            birthday: None,
            face_embedding: None,
            issued_at: "2026-04-14T12:00:00Z",
        };
        let b = IdentityCardPayload {
            pub_key: "pk_abc",
            display_name: "Tom Tang",
            birthday: None,
            face_embedding: None,
            issued_at: "2026-04-14T12:00:00Z",
        };
        assert_eq!(a.canonical_bytes(), b.canonical_bytes());
    }

    #[test]
    fn canonical_bytes_differ_on_display_name_change() {
        let a = IdentityCardPayload {
            pub_key: "pk_abc",
            display_name: "Tom Tang",
            birthday: None,
            face_embedding: None,
            issued_at: "2026-04-14T12:00:00Z",
        };
        let b = IdentityCardPayload {
            pub_key: "pk_abc",
            display_name: "Tom Smith",
            birthday: None,
            face_embedding: None,
            issued_at: "2026-04-14T12:00:00Z",
        };
        assert_ne!(a.canonical_bytes(), b.canonical_bytes());
    }

    // ── Record shape ────────────────────────────────────────────

    #[test]
    fn identity_record_has_required_fields() {
        let rec = identity_record("id_abc", "pk_abc", "Tom", "sig_xxx", "2026-04-14T12:00:00Z");
        assert_eq!(rec.descriptive_schema, IDENTITY);
        assert_eq!(rec.hash_key, "id_abc");
        assert_eq!(rec.fields.get("pub_key").unwrap(), &json!("pk_abc"));
        assert_eq!(rec.fields.get("display_name").unwrap(), &json!("Tom"));
        assert_eq!(rec.fields.get("card_signature").unwrap(), &json!("sig_xxx"));
        assert_eq!(rec.fields.get("node_id").unwrap(), &json!("pk_abc"));
        assert!(rec.fields.get("birthday").unwrap().is_null());
        assert!(rec.fields.get("face_embedding").unwrap().is_null());
    }

    #[test]
    fn node_pub_key_fingerprint_record_has_node_pub_key_kind() {
        let rec = node_pub_key_fingerprint_record("fp_abc", "pk_abc", "2026-04-14T12:00:00Z");
        assert_eq!(rec.descriptive_schema, FINGERPRINT);
        assert_eq!(rec.hash_key, "fp_abc");
        assert_eq!(rec.fields.get("kind").unwrap(), &json!(kind::NODE_PUB_KEY));
        assert_eq!(rec.fields.get("value").unwrap(), &json!("pk_abc"));
    }

    #[test]
    fn identity_receipt_record_is_self_trust_level() {
        let rec = identity_receipt_record("ir_abc", "id_abc", "2026-04-14T12:00:00Z");
        assert_eq!(rec.descriptive_schema, IDENTITY_RECEIPT);
        assert_eq!(rec.fields.get("received_via").unwrap(), &json!("Self"));
        assert_eq!(rec.fields.get("trust_level").unwrap(), &json!("Self"));
        assert!(rec.fields.get("received_from").unwrap().is_null());
    }

    #[test]
    fn me_persona_record_has_built_in_flag_and_self_relationship() {
        let rec = me_persona_record(
            "ps_abc",
            "Tom",
            "fp_nodepubkey",
            "id_selfidentity",
            "2026-04-14T12:00:00Z",
        );
        assert_eq!(rec.descriptive_schema, PERSONA);
        assert_eq!(rec.hash_key, "ps_abc");
        assert_eq!(rec.fields.get("name").unwrap(), &json!("Tom"));
        assert_eq!(rec.fields.get("built_in").unwrap(), &json!(true));
        assert_eq!(rec.fields.get("user_confirmed").unwrap(), &json!(true));
        assert_eq!(rec.fields.get("relationship").unwrap(), &json!("self"));
        // identity_id is a SchemaRef-shaped object, not a bare string.
        assert_eq!(
            rec.fields.get("identity_id").unwrap(),
            &json!({ "schema": "Identity", "key": "id_selfidentity" })
        );
        let seeds = rec
            .fields
            .get("seed_fingerprint_ids")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(seeds.len(), 1);
        assert_eq!(seeds[0], json!("fp_nodepubkey"));
        // Threshold is the build-time default for a built-in persona.
        assert!((rec.fields.get("threshold").unwrap().as_f64().unwrap() - 0.9).abs() < 1e-6);
    }

    // ── Signature verifies round-trip ──────────────────────────

    #[test]
    fn signed_card_verifies_with_corresponding_public_key() {
        use ed25519_dalek::{Verifier, VerifyingKey};
        let seed: [u8; 32] = [7; 32];
        let signing_key = SigningKey::from_bytes(&seed);
        let verifying_key: VerifyingKey = signing_key.verifying_key();

        let card = IdentityCardPayload {
            pub_key: "pk_fake",
            display_name: "Tom Tang",
            birthday: None,
            face_embedding: None,
            issued_at: "2026-04-14T12:00:00Z",
        };
        let bytes = card.canonical_bytes();
        let sig = signing_key.sign(&bytes);
        verifying_key
            .verify(&bytes, &sig)
            .expect("signature must verify");
    }

    #[test]
    fn signature_fails_verification_with_different_payload() {
        use ed25519_dalek::{Verifier, VerifyingKey};
        let seed: [u8; 32] = [7; 32];
        let signing_key = SigningKey::from_bytes(&seed);
        let verifying_key: VerifyingKey = signing_key.verifying_key();

        let card_a = IdentityCardPayload {
            pub_key: "pk_fake",
            display_name: "Tom Tang",
            birthday: None,
            face_embedding: None,
            issued_at: "2026-04-14T12:00:00Z",
        };
        let card_b = IdentityCardPayload {
            pub_key: "pk_fake",
            display_name: "Tom Smith",
            birthday: None,
            face_embedding: None,
            issued_at: "2026-04-14T12:00:00Z",
        };
        let sig_a = signing_key.sign(&card_a.canonical_bytes());
        verifying_key
            .verify(&card_b.canonical_bytes(), &sig_a)
            .expect_err("signature for card_a must NOT verify against card_b");
    }
}
