//! Sharing Handlers
//!
//! Handlers for cross-user sharing operations: rules, invites, subscriptions,
//! and pending invites received via the bulletin board.

use crate::fold_node::node::FoldNode;
use crate::handlers::current_caller_pubkey;
use crate::handlers::response::{ApiResponse, HandlerError, HandlerResult, IntoHandlerError};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use fold_db::security::Ed25519KeyPair;
use fold_db::sharing::{
    signing, store,
    types::{ShareInvite, ShareRule, ShareScope, ShareSubscription},
};
use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_secs()
}

pub async fn get_sled_pool(
    node: &FoldNode,
) -> Result<std::sync::Arc<fold_db::storage::SledPool>, HandlerError> {
    crate::handlers::org::get_sled_pool(node).await
}

/// Build an `Ed25519KeyPair` from the node's base64-encoded private key.
fn node_keypair(node: &FoldNode) -> Result<Ed25519KeyPair, HandlerError> {
    let private_key_b64 = node.get_node_private_key();
    let key_bytes = B64
        .decode(private_key_b64)
        .handler_err("decode node private key")?;
    Ed25519KeyPair::from_secret_key(&key_bytes).handler_err("build node keypair")
}

/// Reconfigure the sync engine's sharing targets from the current on-disk
/// state (memberships + rules + subscriptions), then force a sync cycle.
///
/// Called after any share-related CRUD so runtime changes take effect before
/// the next timer-based sync cycle.
async fn reconfigure_and_force_sync(node: &FoldNode) {
    node.configure_org_sync_if_needed().await;
    if let Ok(db) = node.get_fold_db() {
        if let Err(e) = db.force_sync().await {
            tracing::warn!("force_sync after sharing reconfigure failed: {e}");
        }
    }
}

crate::handlers::handler_response! {
    pub struct ShareRuleResponse {
        pub rule: ShareRule,
    }
}

crate::handlers::handler_response! {
    pub struct ListShareRulesResponse {
        pub rules: Vec<ShareRule>,
    }
}

crate::handlers::handler_response! {
    /// The `ok` flag lives on the `ApiResponse` envelope — no additional payload.
    pub struct OkResponse {}
}

crate::handlers::handler_response! {
    pub struct ShareInviteResponse {
        pub invite: ShareInvite,
    }
}

crate::handlers::handler_response! {
    pub struct AcceptInviteResponse {
        pub subscription: ShareSubscription,
    }
}

crate::handlers::handler_response! {
    pub struct PendingInvitesResponse {
        pub invites: Vec<ShareInvite>,
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateRuleRequest {
    pub recipient_pubkey: String,
    pub recipient_display_name: String,
    pub scope: ShareScope,
}

#[derive(Debug, Deserialize)]
pub struct GenerateInviteRequest {
    pub rule_id: String,
    pub scope_description: String,
}

#[derive(Debug, Deserialize)]
pub struct AcceptInviteRequest {
    pub invite: ShareInvite,
}

pub async fn create_rule(
    req: &CreateRuleRequest,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<ShareRuleResponse> {
    let pool = get_sled_pool(node).await?;
    let my_pubkey = current_caller_pubkey(node);

    let mut e2e_bytes = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut e2e_bytes);

    let my_hash = crate::utils::crypto::user_hash_from_pubkey(&my_pubkey);
    let recipient_hash = crate::utils::crypto::user_hash_from_pubkey(&req.recipient_pubkey);
    let share_prefix = format!("share:{}:{}", my_hash, recipient_hash);

    let mut rule = ShareRule {
        rule_id: uuid::Uuid::new_v4().to_string(),
        recipient_pubkey: req.recipient_pubkey.clone(),
        recipient_display_name: req.recipient_display_name.clone(),
        scope: req.scope.clone(),
        share_prefix,
        share_e2e_secret: e2e_bytes.to_vec(),
        active: true,
        created_at: now_secs(),
        writer_pubkey: my_pubkey.clone(),
        signature: String::new(),
    };

    // Sign the rule with the node's keypair. The signature binds the rule
    // contents to `writer_pubkey` so any observer can verify authenticity.
    let kp = node_keypair(node)?;
    rule.signature = signing::sign_share_rule(&rule, &kp);

    store::create_share_rule(&pool, rule.clone()).handler_err("create share rule")?;

    // Reconfigure the sync engine and trigger an immediate sync cycle so the
    // new target starts uploading right away.
    let node_clone = node.clone();
    tokio::spawn(async move {
        reconfigure_and_force_sync(&node_clone).await;
    });

    Ok(ApiResponse::success_with_user(
        ShareRuleResponse { rule },
        user_hash,
    ))
}

pub async fn list_rules(user_hash: &str, node: &FoldNode) -> HandlerResult<ListShareRulesResponse> {
    let pool = get_sled_pool(node).await?;
    let rules = store::list_share_rules(&pool).handler_err("list share rules")?;

    Ok(ApiResponse::success_with_user(
        ListShareRulesResponse { rules },
        user_hash,
    ))
}

pub async fn deactivate_rule(
    rule_id: &str,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<OkResponse> {
    let pool = get_sled_pool(node).await?;
    store::deactivate_share_rule(&pool, rule_id).handler_err("deactivate share rule")?;

    // Rule is no longer active — refresh sync engine targets to drop it.
    let node_clone = node.clone();
    tokio::spawn(async move {
        reconfigure_and_force_sync(&node_clone).await;
    });

    Ok(ApiResponse::success_with_user(OkResponse {}, user_hash))
}

/// Build a `ShareInvite` from a stored rule.
///
/// The `share_e2e_secret` on the returned invite is encrypted to the
/// recipient's X25519 messaging public key (sealed-box style using
/// [`crate::discovery::connection::encrypt_message`]). Callers are expected
/// to either decrypt it in-place on the receiver side ([`accept_invite`]) or
/// forward the invite as-is via the bulletin board.
///
/// `recipient_messaging_pubkey` is the 32-byte Curve25519 public key of the
/// recipient's messaging pseudonym, looked up from the contact book.
fn build_encrypted_invite(
    rule: &ShareRule,
    sender_pubkey: String,
    sender_display_name: String,
    scope_description: String,
    recipient_messaging_pubkey: &[u8; 32],
) -> Result<ShareInvite, HandlerError> {
    let encrypted_secret = crate::discovery::connection::encrypt_message(
        recipient_messaging_pubkey,
        &rule.share_e2e_secret,
    )
    .handler_err("encrypt invite secret")?;

    Ok(ShareInvite {
        sender_pubkey,
        sender_display_name,
        share_prefix: rule.share_prefix.clone(),
        // `share_e2e_secret` holds the ciphertext (see module doc).
        share_e2e_secret: encrypted_secret,
        scope_description,
    })
}

pub async fn generate_invite(
    req: &GenerateInviteRequest,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<ShareInviteResponse> {
    let pool = get_sled_pool(node).await?;

    let rules = store::list_share_rules(&pool).handler_err("list rules to generate invite")?;
    let rule = rules
        .into_iter()
        .find(|r| r.rule_id == req.rule_id)
        .ok_or_else(|| HandlerError::NotFound(format!("Rule not found: {}", req.rule_id)))?;

    // Look up recipient's messaging public key from the contact book. This
    // key is populated via the discovery connection flow; if it's missing
    // the two nodes haven't exchanged messaging pseudonyms yet.
    let op = crate::fold_node::OperationProcessor::from_ref(node);
    let book = op.load_contact_book().await.handler_err("load contacts")?;

    let contact = book.get(&rule.recipient_pubkey).ok_or_else(|| {
        HandlerError::BadRequest(format!(
            "No contact found for recipient {} — connect via discovery first",
            rule.recipient_pubkey
        ))
    })?;

    let messaging_pk_b64 = contact.messaging_public_key.as_ref().ok_or_else(|| {
        HandlerError::BadRequest(
            "Contact has no messaging public key. Connect via discovery first.".to_string(),
        )
    })?;
    let messaging_pk_bytes = B64
        .decode(messaging_pk_b64)
        .map_err(|e| HandlerError::Internal(format!("Invalid messaging public key: {e}")))?;
    if messaging_pk_bytes.len() != 32 {
        return Err(HandlerError::Internal(
            "Messaging public key must be 32 bytes".to_string(),
        ));
    }
    let mut target_pk = [0u8; 32];
    target_pk.copy_from_slice(&messaging_pk_bytes);

    let my_pubkey = current_caller_pubkey(node);
    let my_display_name = match node.get_fold_db() {
        Ok(db) => crate::trust::identity_card::IdentityCard::load(&db)
            .await
            .ok()
            .flatten()
            .map(|c| c.display_name)
            .unwrap_or_else(|| format!("node-{}", &my_pubkey[..8.min(my_pubkey.len())])),
        Err(_) => format!("node-{}", &my_pubkey[..8.min(my_pubkey.len())]),
    };

    let invite = build_encrypted_invite(
        &rule,
        my_pubkey,
        my_display_name,
        req.scope_description.clone(),
        &target_pk,
    )?;

    // Note: bulletin-board delivery of the invite happens in the
    // `send_invite_via_bulletin_board` helper; callers that have a
    // discovery URL + auth token (the HTTP route wrapper) invoke that after
    // this function returns. We still return the invite so the route can
    // forward it and so tests can inject it directly.

    Ok(ApiResponse::success_with_user(
        ShareInviteResponse { invite },
        user_hash,
    ))
}

pub async fn accept_invite(
    req: &AcceptInviteRequest,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<AcceptInviteResponse> {
    let pool = get_sled_pool(node).await?;

    // Decrypt the invite's share_e2e_secret with the node's node-identity
    // secret. NOTE: this handler expects the invite secret to be encrypted
    // against a key the node can decrypt (either the node's X25519 key
    // directly, or a messaging pseudonym key). For the MVP path, invites
    // delivered via the bulletin board are decrypted by the polling loop
    // (which has the pseudonym secret) and stored in the pending-invites
    // queue with `share_e2e_secret` already in the clear. Invites posted
    // directly to this endpoint with still-encrypted secrets will fail
    // downstream with an "invalid E2E key length" sync error — callers
    // should go through the pending-invites flow.
    let sub = ShareSubscription {
        sender_pubkey: req.invite.sender_pubkey.clone(),
        share_prefix: req.invite.share_prefix.clone(),
        share_e2e_secret: req.invite.share_e2e_secret.clone(),
        accepted_at: now_secs(),
        active: true,
    };

    store::create_share_subscription(&pool, sub.clone())
        .handler_err("create share subscription")?;

    // Remove from pending-invites list (best-effort; no-op if not present).
    let _ = store::remove_pending_invite(&pool, &req.invite.sender_pubkey);

    // Reconfigure sync engine so the new inbound target starts downloading.
    let node_clone = node.clone();
    tokio::spawn(async move {
        reconfigure_and_force_sync(&node_clone).await;
    });

    Ok(ApiResponse::success_with_user(
        AcceptInviteResponse { subscription: sub },
        user_hash,
    ))
}

/// Generate an invite AND deliver it to the recipient via the bulletin board.
///
/// This is the "normal" flow: after creating a share rule, the sender calls
/// this to push the invite to the recipient's messaging pseudonym. The
/// recipient's inbound poller picks it up, decrypts, and stores it in the
/// pending-invites queue. The invite struct is also returned to the caller
/// for UI display / offline fallback.
pub async fn generate_and_send_invite(
    req: &GenerateInviteRequest,
    user_hash: &str,
    node: &FoldNode,
    discovery_url: &str,
    auth_token: &str,
    master_key: &[u8],
) -> HandlerResult<ShareInviteResponse> {
    use crate::discovery::connection::ShareInvitePayload;
    use crate::discovery::publisher::DiscoveryPublisher;

    let pool = get_sled_pool(node).await?;

    let rules = store::list_share_rules(&pool).handler_err("list rules to send invite")?;
    let rule = rules
        .into_iter()
        .find(|r| r.rule_id == req.rule_id)
        .ok_or_else(|| HandlerError::NotFound(format!("Rule not found: {}", req.rule_id)))?;

    // Look up recipient contact + messaging pseudonym + X25519 pubkey.
    let op = crate::fold_node::OperationProcessor::from_ref(node);
    let book = op.load_contact_book().await.handler_err("load contacts")?;

    let contact = book.get(&rule.recipient_pubkey).ok_or_else(|| {
        HandlerError::BadRequest(format!(
            "No contact for recipient {} — connect via discovery first",
            rule.recipient_pubkey
        ))
    })?;

    let messaging_pk_b64 = contact.messaging_public_key.as_ref().ok_or_else(|| {
        HandlerError::BadRequest("Contact has no messaging public key".to_string())
    })?;
    let messaging_pseudonym = contact.messaging_pseudonym.as_ref().ok_or_else(|| {
        HandlerError::BadRequest("Contact has no messaging pseudonym".to_string())
    })?;
    let target_pseudonym: uuid::Uuid = messaging_pseudonym.parse().map_err(|_| {
        HandlerError::Internal("Invalid messaging pseudonym UUID in contact".to_string())
    })?;
    let messaging_pk_bytes = B64
        .decode(messaging_pk_b64)
        .map_err(|e| HandlerError::Internal(format!("Invalid messaging public key: {e}")))?;
    if messaging_pk_bytes.len() != 32 {
        return Err(HandlerError::Internal(
            "Messaging public key must be 32 bytes".to_string(),
        ));
    }
    let mut target_pk = [0u8; 32];
    target_pk.copy_from_slice(&messaging_pk_bytes);

    let my_pubkey = current_caller_pubkey(node);
    let my_display_name = match node.get_fold_db() {
        Ok(db) => crate::trust::identity_card::IdentityCard::load(&db)
            .await
            .ok()
            .flatten()
            .map(|c| c.display_name)
            .unwrap_or_else(|| format!("node-{}", &my_pubkey[..8.min(my_pubkey.len())])),
        Err(_) => format!("node-{}", &my_pubkey[..8.min(my_pubkey.len())]),
    };

    // Encrypt the raw e2e_secret to the recipient's messaging pubkey (inner
    // layer). The outer envelope is a second X25519 layer applied by
    // `encrypt_message` below — the two layers use independent ephemeral
    // keys so compromise of one transient does not reveal the secret.
    let encrypted_secret =
        crate::discovery::connection::encrypt_message(&target_pk, &rule.share_e2e_secret)
            .map_err(|e| HandlerError::Internal(format!("encrypt share secret: {e}")))?;

    let share_invite_payload = ShareInvitePayload {
        message_type: "share_invite".to_string(),
        sender_pubkey: my_pubkey.clone(),
        sender_display_name: my_display_name.clone(),
        share_prefix: rule.share_prefix.clone(),
        scope_description: req.scope_description.clone(),
        share_e2e_secret_encrypted: encrypted_secret,
    };

    // Wrap in the envelope encrypted to the recipient's messaging pubkey.
    let envelope = crate::discovery::connection::encrypt_message(&target_pk, &share_invite_payload)
        .map_err(|e| HandlerError::Internal(format!("encrypt envelope: {e}")))?;

    // Sender pseudonym (for the bulletin board "from" field) — same derivation
    // as used by `send_data_share`.
    let sender_pseudonym = {
        let hash = crate::discovery::pseudonym::content_hash("connection-sender");
        crate::discovery::pseudonym::derive_pseudonym(master_key, &hash)
    };

    let publisher = DiscoveryPublisher::new(
        master_key.to_vec(),
        discovery_url.to_string(),
        auth_token.to_string(),
    );

    let envelope_b64 = B64.encode(&envelope);
    publisher
        .connect(target_pseudonym, envelope_b64, Some(sender_pseudonym))
        .await
        .handler_err("send share invite")?;

    // Also return the (outer) invite struct for the caller. `share_e2e_secret`
    // here is the ciphertext (same as the `ShareInvitePayload` inner field).
    let invite = ShareInvite {
        sender_pubkey: my_pubkey,
        sender_display_name: my_display_name,
        share_prefix: rule.share_prefix,
        share_e2e_secret: share_invite_payload.share_e2e_secret_encrypted,
        scope_description: req.scope_description.clone(),
    };

    Ok(ApiResponse::success_with_user(
        ShareInviteResponse { invite },
        user_hash,
    ))
}

/// List invites that have arrived via the bulletin board and are awaiting
/// user acceptance. The user selects one and POSTs it to `/api/sharing/accept`
/// to create a subscription.
pub async fn list_pending_invites(
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<PendingInvitesResponse> {
    let pool = get_sled_pool(node).await?;
    let invites = store::list_pending_invites(&pool).handler_err("list pending invites")?;
    Ok(ApiResponse::success_with_user(
        PendingInvitesResponse { invites },
        user_hash,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fold_db::security::Ed25519KeyPair;
    use fold_db::sharing::signing::verify_share_rule;
    use fold_db::sharing::types::ShareScope;
    use rand::rngs::OsRng;
    use x25519_dalek::{PublicKey, StaticSecret};

    #[test]
    fn build_encrypted_invite_roundtrips_secret() {
        // Recipient generates a messaging keypair.
        let recipient_secret = StaticSecret::random_from_rng(OsRng);
        let recipient_public = PublicKey::from(&recipient_secret);

        // Build a rule with a known plaintext secret.
        let plaintext_secret = vec![0x42u8; 32];
        let rule = ShareRule {
            rule_id: "rule-1".to_string(),
            recipient_pubkey: "recipient-pk".to_string(),
            recipient_display_name: "Bob".to_string(),
            scope: ShareScope::AllSchemas,
            share_prefix: "share:alice:bob".to_string(),
            share_e2e_secret: plaintext_secret.clone(),
            active: true,
            created_at: 1_700_000_000,
            writer_pubkey: "alice-pk".to_string(),
            signature: String::new(),
        };

        let invite = build_encrypted_invite(
            &rule,
            "alice-pk".to_string(),
            "Alice".to_string(),
            "photos".to_string(),
            recipient_public.as_bytes(),
        )
        .expect("encrypt");

        // Ciphertext must differ from plaintext and carry the sealed-box envelope.
        assert_ne!(invite.share_e2e_secret, plaintext_secret);
        assert!(invite.share_e2e_secret.len() > 32 + 12);

        // Recipient decrypts with their secret.
        let decrypted: Vec<u8> = crate::discovery::connection::decrypt_message_raw(
            &recipient_secret,
            &invite.share_e2e_secret,
        )
        .and_then(|v| serde_json::from_value(v).map_err(|e| e.to_string()))
        .expect("decrypt");
        assert_eq!(decrypted, plaintext_secret);
    }

    #[test]
    fn sign_share_rule_produces_verifiable_signature() {
        let kp = Ed25519KeyPair::generate().unwrap();
        let mut rule = ShareRule {
            rule_id: "rule-1".to_string(),
            recipient_pubkey: "rec".to_string(),
            recipient_display_name: "R".to_string(),
            scope: ShareScope::AllSchemas,
            share_prefix: "share:a:b".to_string(),
            share_e2e_secret: vec![1u8; 32],
            active: true,
            created_at: 1_700_000_000,
            writer_pubkey: kp.public_key_base64(),
            signature: String::new(),
        };
        rule.signature = fold_db::sharing::signing::sign_share_rule(&rule, &kp);
        assert!(!rule.signature.is_empty());
        assert!(verify_share_rule(&rule).unwrap());
    }
}
