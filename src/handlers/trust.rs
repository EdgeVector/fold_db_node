//! Shared Trust Handlers
//!
//! Framework-agnostic handlers for trust management, field policies,
//! identity cards, contacts, trust invites, sharing roles, and audit log.
//!
//! Routes in `server/routes/trust.rs` are thin wrappers that extract request
//! data from HTTP, call these handlers, and convert `HandlerResult<T>` to
//! `HttpResponse`. No business logic lives in the route layer.

use crate::discovery::publisher::DiscoveryPublisher;
use crate::fold_node::node::FoldNode;
use crate::fold_node::OperationProcessor;
use crate::handlers::response::{ApiResponse, HandlerError, HandlerResult, IntoTypedHandlerError};
use crate::trust::identity_card::IdentityCard;
use crate::trust::trust_invite::TrustInvite;
use fold_db::access::TrustTier;
use serde::{Deserialize, Serialize};

/// Resolve the trust-invite sender display name from a local identity card.
///
/// SECURITY: this MUST be the only source of `sender_name` passed to the
/// messaging service. Never accept it from client request bodies — a malicious
/// client could otherwise set `sender_name = "PayPal Security Team"` and
/// phish recipients under the node's SES identity.
pub(crate) fn resolve_sender_name_from_identity(
    identity: Option<IdentityCard>,
) -> Result<String, HandlerError> {
    let Some(card) = identity else {
        return Err(HandlerError::BadRequest(
            "Cannot send invite — set your display name in Settings first.".to_string(),
        ));
    };
    let name = card.display_name;
    if name.trim().is_empty() {
        return Err(HandlerError::BadRequest(
            "Cannot send invite — set your display name in Settings first.".to_string(),
        ));
    }
    Ok(name)
}

// ===== Request/Response types =====

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustGrantRequest {
    pub public_key: String,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustGrantsResponse {
    pub grants: Vec<TrustGrantEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustGrantEntry {
    pub public_key: String,
    pub tier: TrustTier,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustResolveResponse {
    pub public_key: String,
    pub tier: Option<TrustTier>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetFieldPolicyRequest {
    pub policy: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldPolicyResponse {
    pub schema_name: String,
    pub field_name: String,
    pub policy: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogResponse {
    pub events: serde_json::Value,
    pub count: usize,
}

#[derive(Debug, Deserialize)]
pub struct AuditLogQuery {
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetIdentityCardRequest {
    pub display_name: String,
    #[serde(default)]
    pub contact_hint: Option<String>,
    #[serde(default)]
    pub birthday: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateInviteRequest {
    pub proposed_role: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AcceptInviteRequest {
    /// The trust invite token (base64url).
    pub token: String,
    /// Override the proposed role (optional).
    #[serde(default)]
    pub accept_role: Option<String>,
    /// Whether to trust back (create reciprocal invite).
    #[serde(default)]
    pub trust_back: bool,
}

#[derive(Debug, Deserialize)]
pub struct AssignRoleRequest {
    pub role_name: String,
}

// ===== Trust management =====

/// Assign a role to a public key (role determines tier).
pub async fn grant_trust(
    req: &TrustGrantRequest,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<serde_json::Value> {
    let op = OperationProcessor::new(node.clone());
    op.assign_role_to_contact(&req.public_key, &req.role)
        .await
        .typed_handler_err()?;
    Ok(ApiResponse::success_with_user(
        serde_json::json!({"granted": true, "role": req.role}),
        user_hash,
    ))
}

/// Revoke trust for a public key.
pub async fn revoke_trust(
    public_key: &str,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<serde_json::Value> {
    let op = OperationProcessor::new(node.clone());
    op.revoke_trust(public_key).await.typed_handler_err()?;
    Ok(ApiResponse::success_with_user(
        serde_json::json!({"revoked": true}),
        user_hash,
    ))
}

/// List all trust assignments.
pub async fn list_trust_grants(
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<TrustGrantsResponse> {
    let op = OperationProcessor::new(node.clone());
    let grants = op.list_trust_grants().await.typed_handler_err()?;
    let entries = grants
        .into_iter()
        .map(|(public_key, tier)| TrustGrantEntry { public_key, tier })
        .collect();
    Ok(ApiResponse::success_with_user(
        TrustGrantsResponse { grants: entries },
        user_hash,
    ))
}

/// Check resolved trust tier for a key.
pub async fn resolve_trust(
    public_key: String,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<TrustResolveResponse> {
    let op = OperationProcessor::new(node.clone());
    let tier = op
        .resolve_trust_tier(&public_key)
        .await
        .typed_handler_err()?;
    Ok(ApiResponse::success_with_user(
        TrustResolveResponse { public_key, tier },
        user_hash,
    ))
}

// ===== Schema policy =====

/// Set a field access policy.
pub async fn set_field_policy(
    schema_name: &str,
    field_name: &str,
    req: SetFieldPolicyRequest,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<serde_json::Value> {
    let op = OperationProcessor::new(node.clone());
    op.set_field_access_policy(schema_name, field_name, req.policy)
        .await
        .typed_handler_err()?;
    Ok(ApiResponse::success_with_user(
        serde_json::json!({"policy_set": true}),
        user_hash,
    ))
}

/// Get a field access policy.
pub async fn get_field_policy(
    schema_name: String,
    field_name: String,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<FieldPolicyResponse> {
    let op = OperationProcessor::new(node.clone());
    let policy = op
        .get_field_access_policy(&schema_name, &field_name)
        .await
        .typed_handler_err()?;
    Ok(ApiResponse::success_with_user(
        FieldPolicyResponse {
            schema_name,
            field_name,
            policy,
        },
        user_hash,
    ))
}

/// Get all field access policies for a schema.
pub async fn get_all_field_policies(
    schema_name: String,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<serde_json::Value> {
    let op = OperationProcessor::new(node.clone());
    let policies = op
        .get_all_field_policies(&schema_name)
        .await
        .typed_handler_err()?;
    let policies_json = serde_json::to_value(&policies)
        .map_err(|e| HandlerError::Internal(format!("Failed to serialize policies: {}", e)))?;
    Ok(ApiResponse::success_with_user(
        serde_json::json!({
            "schema_name": schema_name,
            "field_policies": policies_json,
        }),
        user_hash,
    ))
}

// ===== Audit log =====

/// Get recent audit events.
pub async fn get_audit_log(
    limit: usize,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<AuditLogResponse> {
    let op = OperationProcessor::new(node.clone());
    let events = op.get_audit_log(limit).await.typed_handler_err()?;
    let count = events.as_array().map_or(0, |a| a.len());
    Ok(ApiResponse::success_with_user(
        AuditLogResponse { events, count },
        user_hash,
    ))
}

// ===== Identity card =====

/// Get the current identity card.
pub async fn get_identity_card(
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<serde_json::Value> {
    let op = OperationProcessor::new(node.clone());
    let card = op.get_identity_card().typed_handler_err()?;
    Ok(ApiResponse::success_with_user(
        serde_json::json!({ "identity_card": card }),
        user_hash,
    ))
}

/// Set or update the identity card.
pub async fn set_identity_card(
    req: SetIdentityCardRequest,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<serde_json::Value> {
    let op = OperationProcessor::new(node.clone());
    op.set_identity_card(req.display_name, req.contact_hint, req.birthday)
        .typed_handler_err()?;
    Ok(ApiResponse::success_with_user(
        serde_json::json!({"saved": true}),
        user_hash,
    ))
}

// ===== Contacts =====

/// List all active contacts.
pub async fn list_contacts(user_hash: &str, node: &FoldNode) -> HandlerResult<serde_json::Value> {
    let op = OperationProcessor::new(node.clone());
    let contacts = op.list_contacts().typed_handler_err()?;
    Ok(ApiResponse::success_with_user(
        serde_json::json!({ "contacts": contacts }),
        user_hash,
    ))
}

/// Get a specific contact.
pub async fn get_contact(
    public_key: &str,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<serde_json::Value> {
    let op = OperationProcessor::new(node.clone());
    let contact = op.get_contact(public_key).typed_handler_err()?;
    Ok(ApiResponse::success_with_user(
        serde_json::json!({ "contact": contact }),
        user_hash,
    ))
}

/// Revoke trust and remove a contact.
pub async fn revoke_contact(
    public_key: &str,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<serde_json::Value> {
    let op = OperationProcessor::new(node.clone());
    op.revoke_contact(public_key).await.typed_handler_err()?;
    Ok(ApiResponse::success_with_user(
        serde_json::json!({"revoked": true}),
        user_hash,
    ))
}

// ===== Trust invites =====

/// Create a signed trust invite token.
pub async fn create_trust_invite(
    req: &CreateInviteRequest,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<serde_json::Value> {
    let op = OperationProcessor::new(node.clone());
    let invite = op
        .create_trust_invite(&req.proposed_role)
        .typed_handler_err()?;
    let token = invite
        .to_token()
        .map_err(fold_db::schema::SchemaError::InvalidData)
        .typed_handler_err()?;
    Ok(ApiResponse::success_with_user(
        serde_json::json!({
            "invite": invite,
            "token": token,
        }),
        user_hash,
    ))
}

/// Accept a trust invite token.
pub async fn accept_trust_invite(
    req: AcceptInviteRequest,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<serde_json::Value> {
    let op = OperationProcessor::new(node.clone());
    let invite = TrustInvite::from_token(&req.token)
        .map_err(fold_db::schema::SchemaError::InvalidData)
        .typed_handler_err()?;

    let reciprocal = op
        .accept_trust_invite(&invite, req.accept_role.as_deref(), req.trust_back)
        .await
        .typed_handler_err()?;

    let reciprocal_token = match &reciprocal {
        Some(inv) => Some(
            inv.to_token()
                .map_err(fold_db::schema::SchemaError::InvalidData)
                .typed_handler_err()?,
        ),
        None => None,
    };

    Ok(ApiResponse::success_with_user(
        serde_json::json!({
            "accepted": true,
            "sender": {
                "display_name": invite.sender_identity.display_name,
                "contact_hint": invite.sender_identity.contact_hint,
                "public_key": invite.sender_pub_key,
            },
            "reciprocal_invite": reciprocal,
            "reciprocal_token": reciprocal_token,
        }),
        user_hash,
    ))
}

/// Preview a trust invite token without accepting.
pub async fn preview_trust_invite(token: &str) -> HandlerResult<serde_json::Value> {
    let invite = TrustInvite::from_token(token)
        .map_err(fold_db::schema::SchemaError::InvalidData)
        .typed_handler_err()?;
    let valid = invite
        .verify()
        .map_err(fold_db::schema::SchemaError::InvalidData)
        .typed_handler_err()?;

    Ok(ApiResponse::success(serde_json::json!({
        "valid": valid,
        "sender": {
            "display_name": invite.sender_identity.display_name,
            "contact_hint": invite.sender_identity.contact_hint,
            "public_key": invite.sender_pub_key,
            "fingerprint": invite.fingerprint(),
        },
        "proposed_role": invite.proposed_role,
        "created_at": invite.created_at,
    })))
}

// ===== Discovery-relay-backed invite sharing =====

/// Upload an invite token to the Exemem relay, return short ID.
pub async fn share_trust_invite(
    publisher: &DiscoveryPublisher,
    invite_token: &str,
    user_hash: &str,
) -> HandlerResult<serde_json::Value> {
    let invite_id = publisher
        .store_trust_invite(invite_token)
        .await
        .map_err(HandlerError::Internal)?;
    Ok(ApiResponse::success_with_user(
        serde_json::json!({
            "invite_id": invite_id,
            "shared": true,
        }),
        user_hash,
    ))
}

/// Fetch an invite token from the Exemem relay by ID.
pub async fn fetch_shared_invite(
    publisher: &DiscoveryPublisher,
    invite_id: &str,
) -> HandlerResult<serde_json::Value> {
    let token = publisher
        .fetch_trust_invite(invite_id)
        .await
        .map_err(HandlerError::NotFound)?;
    Ok(ApiResponse::success(
        serde_json::json!({"ok": true, "token": token}),
    ))
}

/// Send invite with email verification.
///
/// SECURITY: `sender_name` is resolved server-side from the local identity card.
/// Never trust client-supplied sender names — a malicious client could set
/// `sender_name = "PayPal Security Team"` and send a phishing email under the
/// node's SES identity. The display name must come from the server-owned
/// identity card.
pub async fn send_verified_invite(
    publisher: &DiscoveryPublisher,
    node: &FoldNode,
    invite_token: &str,
    recipient_email: &str,
    user_hash: &str,
) -> HandlerResult<serde_json::Value> {
    let op = OperationProcessor::new(node.clone());
    let identity_card = op.get_identity_card().typed_handler_err()?;
    let sender_name = resolve_sender_name_from_identity(identity_card)?;
    let invite_id = publisher
        .send_verified_invite(invite_token, recipient_email, &sender_name)
        .await
        .map_err(HandlerError::Internal)?;
    Ok(ApiResponse::success_with_user(
        serde_json::json!({"ok": true, "invite_id": invite_id}),
        user_hash,
    ))
}

/// Verify a code and fetch the invite token.
pub async fn verify_invite_code(
    publisher: &DiscoveryPublisher,
    invite_id: &str,
    code: &str,
) -> HandlerResult<serde_json::Value> {
    let invite_token = publisher
        .verify_invite_code(invite_id, code)
        .await
        .map_err(HandlerError::BadRequest)?;
    Ok(ApiResponse::success(
        serde_json::json!({"ok": true, "token": invite_token}),
    ))
}

// ===== Sharing roles =====

/// List all role definitions.
pub fn list_sharing_roles() -> HandlerResult<serde_json::Value> {
    let config = crate::trust::sharing_roles::SharingRoleConfig::load().unwrap_or_default();
    Ok(ApiResponse::success(
        serde_json::json!({"roles": config.roles}),
    ))
}

/// Assign a role to a contact.
pub async fn assign_contact_role(
    public_key: &str,
    req: &AssignRoleRequest,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<serde_json::Value> {
    let op = OperationProcessor::new(node.clone());
    op.assign_role_to_contact(public_key, &req.role_name)
        .await
        .typed_handler_err()?;
    Ok(ApiResponse::success_with_user(
        serde_json::json!({"assigned": true, "role": req.role_name}),
        user_hash,
    ))
}

/// Remove a role from a contact in a domain.
pub async fn remove_contact_role(
    public_key: &str,
    domain: &str,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<serde_json::Value> {
    let op = OperationProcessor::new(node.clone());
    op.remove_role_from_contact(public_key, domain)
        .await
        .typed_handler_err()?;
    Ok(ApiResponse::success_with_user(
        serde_json::json!({"removed": true, "domain": domain}),
        user_hash,
    ))
}

/// Audit what a contact can see.
pub async fn sharing_audit(
    public_key: &str,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<crate::trust::sharing_audit::SharingAuditResult> {
    let op = OperationProcessor::new(node.clone());
    let result = op
        .audit_contact_access(public_key)
        .await
        .typed_handler_err()?;
    Ok(ApiResponse::success_with_user(result, user_hash))
}

/// Overview of the node's sharing posture.
pub async fn sharing_posture(user_hash: &str, node: &FoldNode) -> HandlerResult<serde_json::Value> {
    let op = OperationProcessor::new(node.clone());
    let result = op.sharing_posture().await.typed_handler_err()?;
    Ok(ApiResponse::success_with_user(result, user_hash))
}

/// Apply classification-based access policies to all approved schemas.
pub async fn apply_defaults_all(
    force: bool,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<serde_json::Value> {
    let op = OperationProcessor::new(node.clone());
    let db = op.get_db_public().typed_handler_err()?;
    let schemas = db
        .schema_manager()
        .get_schemas_with_states()
        .map_err(|e| HandlerError::Internal(format!("Failed to get schemas with states: {}", e)))?;
    drop(db);

    let mut total_applied = 0usize;
    let mut schemas_updated = 0usize;

    for sws in &schemas {
        if sws.state != fold_db::schema::SchemaState::Approved {
            continue;
        }
        if let Ok(count) = op
            .apply_classification_defaults_with_force(&sws.schema.name, force)
            .await
        {
            if count > 0 {
                total_applied += count;
                schemas_updated += 1;
            }
        }
    }

    Ok(ApiResponse::success_with_user(
        serde_json::json!({
            "schemas_updated": schemas_updated,
            "fields_updated": total_applied,
        }),
        user_hash,
    ))
}

// ===== Declined invites =====

/// Decline a trust invite (record locally).
pub async fn decline_trust_invite(token: &str) -> HandlerResult<serde_json::Value> {
    let invite = TrustInvite::from_token(token)
        .map_err(fold_db::schema::SchemaError::InvalidData)
        .typed_handler_err()?;

    let mut store = crate::trust::declined_invites::DeclinedInviteStore::load()
        .map_err(fold_db::schema::SchemaError::InvalidData)
        .typed_handler_err()?;

    store.decline(crate::trust::declined_invites::DeclinedInvite {
        sender_pub_key: invite.sender_pub_key.clone(),
        sender_display_name: invite.sender_identity.display_name.clone(),
        sender_contact_hint: invite.sender_identity.contact_hint.clone(),
        proposed_role: invite.proposed_role.clone(),
        declined_at: chrono::Utc::now(),
        nonce: invite.nonce.clone(),
    });

    store
        .save()
        .map_err(fold_db::schema::SchemaError::InvalidData)
        .typed_handler_err()?;

    Ok(ApiResponse::success(serde_json::json!({
        "declined": true,
        "sender": invite.sender_identity.display_name,
    })))
}

/// List all declined invites.
pub async fn list_declined_invites() -> HandlerResult<serde_json::Value> {
    let store = crate::trust::declined_invites::DeclinedInviteStore::load()
        .map_err(fold_db::schema::SchemaError::InvalidData)
        .typed_handler_err()?;
    Ok(ApiResponse::success(
        serde_json::json!({"declined_invites": store.invites}),
    ))
}

/// Undo a decline (change mind).
pub async fn undecline_invite(nonce: &str) -> HandlerResult<serde_json::Value> {
    let mut store = crate::trust::declined_invites::DeclinedInviteStore::load()
        .map_err(fold_db::schema::SchemaError::InvalidData)
        .typed_handler_err()?;
    let removed = store.undecline(nonce);
    store
        .save()
        .map_err(fold_db::schema::SchemaError::InvalidData)
        .typed_handler_err()?;
    Ok(ApiResponse::success(
        serde_json::json!({"undeclined": removed}),
    ))
}

// ===== Sent invites =====

/// List all sent invites with status.
pub async fn list_sent_invites() -> HandlerResult<serde_json::Value> {
    let store = crate::trust::sent_invites::SentInviteStore::load()
        .map_err(fold_db::schema::SchemaError::InvalidData)
        .typed_handler_err()?;
    Ok(ApiResponse::success(
        serde_json::json!({"sent_invites": store.invites}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_sender_name_returns_display_name_from_card() {
        let card = IdentityCard::new("Alice".to_string(), None, None);
        let name = resolve_sender_name_from_identity(Some(card))
            .expect("valid identity card should yield a sender name");
        assert_eq!(name, "Alice");
    }

    #[test]
    fn resolve_sender_name_rejects_missing_identity_card() {
        let err = resolve_sender_name_from_identity(None)
            .expect_err("missing identity card must be a BadRequest error");
        match err {
            HandlerError::BadRequest(msg) => {
                assert!(
                    msg.contains("display name"),
                    "error message should mention display name, got: {msg}"
                );
            }
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn resolve_sender_name_rejects_blank_display_name() {
        let card = IdentityCard::new("   ".to_string(), None, None);
        let err = resolve_sender_name_from_identity(Some(card))
            .expect_err("blank display name must be rejected, not silently passed through");
        assert!(matches!(err, HandlerError::BadRequest(_)));
    }

    #[test]
    fn resolve_sender_name_does_not_fall_back_to_placeholder() {
        // Regression guard: this helper must never return an
        // attacker-friendly fallback like "Anonymous" or the user_hash.
        // Missing identity is an explicit error, not a silent default.
        let err = resolve_sender_name_from_identity(None).unwrap_err();
        let HandlerError::BadRequest(msg) = err else {
            panic!("expected BadRequest");
        };
        assert!(!msg.to_lowercase().contains("anonymous"));
    }
}
