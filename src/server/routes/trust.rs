use crate::fold_node::OperationProcessor;
use crate::handlers::{ApiResponse, IntoHandlerError, IntoTypedHandlerError};
use crate::server::http_server::AppState;
use crate::server::routes::{handler_result_to_response, node_or_return};
use crate::trust::trust_invite::TrustInvite;
use actix_web::{web, Responder};
use fold_db::access::TrustTier;
use serde::{Deserialize, Serialize};

/// Fix base64 public keys that get mangled by URL path decoding.
/// Actix decodes `%2B` → `+` correctly, but some clients/proxies
/// decode `+` → space. This reverses that corruption.
fn fix_pubkey_from_path(key: &str) -> String {
    key.replace(' ', "+")
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

// ===== Trust management endpoints =====

/// POST /api/trust/grant — assign a role to a public key (role determines tier)
pub async fn grant_trust(
    body: web::Json<TrustGrantRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            op.assign_role_to_contact(&body.public_key, &body.role)
                .await
                .typed_handler_err()?;
            Ok(ApiResponse::success_with_user(
                serde_json::json!({"granted": true, "role": body.role}),
                user_hash,
            ))
        }
        .await,
    )
}

/// DELETE /api/trust/revoke/{key} — revoke trust for a public key
pub async fn revoke_trust(path: web::Path<String>, state: web::Data<AppState>) -> impl Responder {
    let public_key = fix_pubkey_from_path(&path.into_inner());
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            op.revoke_trust(&public_key).await.typed_handler_err()?;
            Ok(ApiResponse::success_with_user(
                serde_json::json!({"revoked": true}),
                user_hash,
            ))
        }
        .await,
    )
}

/// GET /api/trust/grants — list all trust assignments
pub async fn list_trust_grants(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            let grants = op.list_trust_grants().await.typed_handler_err()?;
            let entries: Vec<TrustGrantEntry> = grants
                .into_iter()
                .map(|(public_key, tier)| TrustGrantEntry { public_key, tier })
                .collect();
            Ok(ApiResponse::success_with_user(
                TrustGrantsResponse { grants: entries },
                user_hash,
            ))
        }
        .await,
    )
}

/// GET /api/trust/resolve/{key} — check resolved trust tier for a key
pub async fn resolve_trust(path: web::Path<String>, state: web::Data<AppState>) -> impl Responder {
    let public_key = fix_pubkey_from_path(&path.into_inner());
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            let tier = op
                .resolve_trust_tier(&public_key)
                .await
                .typed_handler_err()?;
            Ok(ApiResponse::success_with_user(
                TrustResolveResponse { public_key, tier },
                user_hash,
            ))
        }
        .await,
    )
}

// ===== Schema policy endpoints =====

/// PUT /api/schema/{name}/field/{field}/policy — set field access policy
pub async fn set_field_policy(
    path: web::Path<(String, String)>,
    body: web::Json<SetFieldPolicyRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (schema_name, field_name) = path.into_inner();
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            op.set_field_access_policy(&schema_name, &field_name, body.into_inner().policy)
                .await
                .typed_handler_err()?;
            Ok(ApiResponse::success_with_user(
                serde_json::json!({"policy_set": true}),
                user_hash,
            ))
        }
        .await,
    )
}

/// GET /api/schema/{name}/field/{field}/policy — get field access policy
pub async fn get_field_policy(
    path: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (schema_name, field_name) = path.into_inner();
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
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
        .await,
    )
}

/// GET /api/schema/{name}/policies — get all field access policies for a schema
pub async fn get_all_field_policies(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let schema_name = path.into_inner();
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            let policies = op
                .get_all_field_policies(&schema_name)
                .await
                .typed_handler_err()?;
            let policies_json =
                serde_json::to_value(&policies).handler_err("serialize policies")?;
            Ok(ApiResponse::success_with_user(
                serde_json::json!({
                    "schema_name": schema_name,
                    "field_policies": policies_json,
                }),
                user_hash,
            ))
        }
        .await,
    )
}

// ===== Audit log endpoint =====

/// GET /api/trust/audit?limit=100 — get recent audit events
pub async fn get_audit_log(
    query: web::Query<AuditLogQuery>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    let limit = query.limit.unwrap_or(100);
    handler_result_to_response(
        async {
            let events = op.get_audit_log(limit).await.typed_handler_err()?;
            let count = events.as_array().map_or(0, |a| a.len());
            Ok(ApiResponse::success_with_user(
                AuditLogResponse { events, count },
                user_hash,
            ))
        }
        .await,
    )
}

#[derive(Debug, Deserialize)]
pub struct AuditLogQuery {
    pub limit: Option<usize>,
}

// ===== Identity card endpoints =====

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetIdentityCardRequest {
    pub display_name: String,
    #[serde(default)]
    pub contact_hint: Option<String>,
    #[serde(default)]
    pub birthday: Option<String>,
}

/// GET /api/identity/card — get the current identity card
pub async fn get_identity_card(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            let card = op.get_identity_card().typed_handler_err()?;
            Ok(ApiResponse::success_with_user(
                serde_json::json!({ "identity_card": card }),
                user_hash,
            ))
        }
        .await,
    )
}

/// PUT /api/identity/card — set or update the identity card
pub async fn set_identity_card(
    body: web::Json<SetIdentityCardRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    let req = body.into_inner();
    handler_result_to_response(
        async {
            op.set_identity_card(req.display_name, req.contact_hint, req.birthday)
                .typed_handler_err()?;
            Ok(ApiResponse::success_with_user(
                serde_json::json!({"saved": true}),
                user_hash,
            ))
        }
        .await,
    )
}

// ===== Contact book endpoints =====

/// GET /api/contacts — list all active contacts
pub async fn list_contacts(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            let contacts = op.list_contacts().typed_handler_err()?;
            Ok(ApiResponse::success_with_user(
                serde_json::json!({ "contacts": contacts }),
                user_hash,
            ))
        }
        .await,
    )
}

/// GET /api/contacts/{key} — get a specific contact
pub async fn get_contact(path: web::Path<String>, state: web::Data<AppState>) -> impl Responder {
    let public_key = fix_pubkey_from_path(&path.into_inner());
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            let contact = op.get_contact(&public_key).typed_handler_err()?;
            Ok(ApiResponse::success_with_user(
                serde_json::json!({ "contact": contact }),
                user_hash,
            ))
        }
        .await,
    )
}

// ===== Trust invite endpoints =====

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

/// POST /api/trust/invite — create a signed trust invite token
pub async fn create_trust_invite(
    body: web::Json<CreateInviteRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            let invite = op
                .create_trust_invite(&body.proposed_role)
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
        .await,
    )
}

/// POST /api/trust/invite/accept — accept a trust invite token
pub async fn accept_trust_invite(
    body: web::Json<AcceptInviteRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    let req = body.into_inner();
    handler_result_to_response(
        async {
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
        .await,
    )
}

/// POST /api/trust/invite/preview — preview a trust invite without accepting
pub async fn preview_trust_invite(
    body: web::Json<serde_json::Value>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, _node) = node_or_return!(state);
    handler_result_to_response(
        async {
            let token = body
                .get("token")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    fold_db::schema::SchemaError::InvalidData("Missing 'token' field".to_string())
                })
                .typed_handler_err()?;

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
        .await,
    )
}

/// DELETE /api/contacts/{key} — revoke trust and remove contact
pub async fn revoke_contact(path: web::Path<String>, state: web::Data<AppState>) -> impl Responder {
    let public_key = fix_pubkey_from_path(&path.into_inner());
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            op.revoke_contact(&public_key).await.typed_handler_err()?;
            Ok(ApiResponse::success_with_user(
                serde_json::json!({"revoked": true}),
                user_hash,
            ))
        }
        .await,
    )
}

// ===== Trust invite relay (via Exemem discovery service) =====

/// POST /api/trust/invite/share — upload invite token to Exemem relay, return short ID
pub async fn share_trust_invite(
    body: web::Json<serde_json::Value>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, _node) = node_or_return!(state);

    let (url, key, token) = match get_discovery_config_and_token() {
        Ok(cfg) => cfg,
        Err(e) => {
            return actix_web::HttpResponse::ServiceUnavailable()
                .json(serde_json::json!({"error": e}));
        }
    };

    let publisher = crate::discovery::publisher::DiscoveryPublisher::new(key, url, token);

    let invite_token = match body.get("token").and_then(|v| v.as_str()) {
        Some(t) => t.to_string(),
        None => {
            return actix_web::HttpResponse::BadRequest()
                .json(serde_json::json!({"error": "Missing 'token' field"}));
        }
    };

    match publisher.store_trust_invite(&invite_token).await {
        Ok(invite_id) => actix_web::HttpResponse::Ok().json(ApiResponse::success_with_user(
            serde_json::json!({
                "invite_id": invite_id,
                "shared": true,
            }),
            user_hash,
        )),
        Err(e) => {
            actix_web::HttpResponse::InternalServerError().json(serde_json::json!({"error": e}))
        }
    }
}

/// GET /api/trust/invite/fetch?id=xxx — fetch invite token from Exemem relay by ID
pub async fn fetch_shared_invite(
    query: web::Query<std::collections::HashMap<String, String>>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, _node) = node_or_return!(state);

    let (url, key, token) = match get_discovery_config_and_token() {
        Ok(cfg) => cfg,
        Err(e) => {
            return actix_web::HttpResponse::ServiceUnavailable()
                .json(serde_json::json!({"error": e}));
        }
    };

    let publisher = crate::discovery::publisher::DiscoveryPublisher::new(key, url, token);

    let invite_id = match query.get("id") {
        Some(id) => id.clone(),
        None => {
            return actix_web::HttpResponse::BadRequest()
                .json(serde_json::json!({"error": "Missing 'id' query parameter"}));
        }
    };

    match publisher.fetch_trust_invite(&invite_id).await {
        Ok(token) => {
            actix_web::HttpResponse::Ok().json(serde_json::json!({"ok": true, "token": token}))
        }
        Err(e) => actix_web::HttpResponse::NotFound().json(serde_json::json!({"error": e})),
    }
}

/// Helper: get discovery URL, master key, and auth token from env/credentials.
fn get_discovery_config_and_token() -> Result<(String, Vec<u8>, String), String> {
    let url = std::env::var("DISCOVERY_SERVICE_URL").map_err(|_| {
        "Email invites and link sharing require Exemem cloud backup. \
         Enable cloud backup in Settings to use these features."
            .to_string()
    })?;
    let key_hex = std::env::var("DISCOVERY_MASTER_KEY").map_err(|_| {
        "Email invites and link sharing require Exemem cloud backup. \
         Enable cloud backup in Settings to use these features."
            .to_string()
    })?;
    let key = hex::decode(&key_hex).map_err(|_| "Invalid discovery configuration".to_string())?;

    let token = std::env::var("DISCOVERY_AUTH_TOKEN")
        .or_else(|_| {
            crate::keychain::load_credentials()
                .ok()
                .flatten()
                .and_then(|c| {
                    let t = &c.session_token;
                    if t.is_empty() {
                        return None;
                    }
                    // Check if token is expired
                    let parts: Vec<&str> = t.split('.').collect();
                    if parts.len() >= 3 {
                        if let Ok(expiry) = parts[2].parse::<i64>() {
                            let now = chrono::Utc::now().timestamp();
                            if now > expiry {
                                log::warn!(
                                    "Exemem session token expired ({} seconds ago). \
                                     Re-authenticate in Settings to refresh.",
                                    now - expiry
                                );
                                return None;
                            }
                        }
                    }
                    Some(t.clone())
                })
                .ok_or_else(|| "no token".to_string())
        })
        .map_err(|_| {
            "Exemem session expired or not signed in. \
             Re-authenticate in Settings > Cloud Backup to refresh."
                .to_string()
        })?;

    Ok((url, key, token))
}

/// POST /api/trust/invite/send-verified — send invite with email verification
pub async fn send_verified_invite(
    body: web::Json<serde_json::Value>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, _node) = node_or_return!(state);

    let (url, key, token) = match get_discovery_config_and_token() {
        Ok(cfg) => cfg,
        Err(e) => {
            return actix_web::HttpResponse::ServiceUnavailable()
                .json(serde_json::json!({"error": e}));
        }
    };

    let publisher = crate::discovery::publisher::DiscoveryPublisher::new(key, url, token);

    let invite_token = match body.get("token").and_then(|v| v.as_str()) {
        Some(t) => t.to_string(),
        None => {
            return actix_web::HttpResponse::BadRequest()
                .json(serde_json::json!({"error": "Missing 'token' field"}));
        }
    };
    let recipient_email = match body.get("recipient_email").and_then(|v| v.as_str()) {
        Some(e) => e.to_string(),
        None => {
            return actix_web::HttpResponse::BadRequest()
                .json(serde_json::json!({"error": "Missing 'recipient_email' field"}));
        }
    };
    let sender_name = match body.get("sender_name").and_then(|v| v.as_str()) {
        Some(n) => n.to_string(),
        None => {
            return actix_web::HttpResponse::BadRequest()
                .json(serde_json::json!({"error": "Missing 'sender_name' field"}));
        }
    };

    match publisher
        .send_verified_invite(&invite_token, &recipient_email, &sender_name)
        .await
    {
        Ok(invite_id) => actix_web::HttpResponse::Ok().json(ApiResponse::success_with_user(
            serde_json::json!({"ok": true, "invite_id": invite_id}),
            user_hash,
        )),
        Err(e) => {
            actix_web::HttpResponse::InternalServerError().json(serde_json::json!({"error": e}))
        }
    }
}

/// POST /api/trust/invite/verify — verify a code and fetch the invite token
pub async fn verify_invite_code(
    body: web::Json<serde_json::Value>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, _node) = node_or_return!(state);

    let (url, key, token) = match get_discovery_config_and_token() {
        Ok(cfg) => cfg,
        Err(e) => {
            return actix_web::HttpResponse::ServiceUnavailable()
                .json(serde_json::json!({"error": e}));
        }
    };

    let publisher = crate::discovery::publisher::DiscoveryPublisher::new(key, url, token);

    let invite_id = match body.get("invite_id").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => {
            return actix_web::HttpResponse::BadRequest()
                .json(serde_json::json!({"error": "Missing 'invite_id' field"}));
        }
    };
    let code = match body.get("code").and_then(|v| v.as_str()) {
        Some(c) => c.to_string(),
        None => {
            return actix_web::HttpResponse::BadRequest()
                .json(serde_json::json!({"error": "Missing 'code' field"}));
        }
    };

    match publisher.verify_invite_code(&invite_id, &code).await {
        Ok(invite_token) => actix_web::HttpResponse::Ok()
            .json(serde_json::json!({"ok": true, "token": invite_token})),
        Err(e) => actix_web::HttpResponse::BadRequest().json(serde_json::json!({"error": e})),
    }
}

// ===== Sharing roles endpoints =====

/// GET /api/sharing/roles — list all role definitions
pub async fn list_sharing_roles(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, _node) = node_or_return!(state);
    let config = crate::trust::sharing_roles::SharingRoleConfig::load().unwrap_or_default();
    actix_web::HttpResponse::Ok().json(serde_json::json!({"roles": config.roles}))
}

#[derive(Debug, Deserialize)]
pub struct AssignRoleRequest {
    pub role_name: String,
}

/// POST /api/contacts/{key}/role — assign a role to a contact
pub async fn assign_contact_role(
    path: web::Path<String>,
    body: web::Json<AssignRoleRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let public_key = fix_pubkey_from_path(&path.into_inner());
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            op.assign_role_to_contact(&public_key, &body.role_name)
                .await
                .typed_handler_err()?;
            Ok(ApiResponse::success_with_user(
                serde_json::json!({"assigned": true, "role": body.role_name}),
                user_hash,
            ))
        }
        .await,
    )
}

/// DELETE /api/contacts/{key}/role/{domain} — remove role from contact in domain
pub async fn remove_contact_role(
    path: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (pk_raw, domain) = path.into_inner();
    let public_key = fix_pubkey_from_path(&pk_raw);
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            op.remove_role_from_contact(&public_key, &domain)
                .await
                .typed_handler_err()?;
            Ok(ApiResponse::success_with_user(
                serde_json::json!({"removed": true, "domain": domain}),
                user_hash,
            ))
        }
        .await,
    )
}

/// GET /api/sharing/audit/{key} — audit what a contact can see
pub async fn sharing_audit(path: web::Path<String>, state: web::Data<AppState>) -> impl Responder {
    let public_key = fix_pubkey_from_path(&path.into_inner());
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            let result = op
                .audit_contact_access(&public_key)
                .await
                .typed_handler_err()?;
            Ok(ApiResponse::success_with_user(result, user_hash))
        }
        .await,
    )
}

/// GET /api/sharing/posture — overview of the node's sharing posture
pub async fn sharing_posture(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            let result = op.sharing_posture().await.typed_handler_err()?;
            Ok(ApiResponse::success_with_user(result, user_hash))
        }
        .await,
    )
}

/// POST /api/sharing/apply-defaults — apply classification-based access policies
/// to all approved schemas. Query param ?force=true overwrites existing policies.
pub async fn apply_defaults_all(
    query: web::Query<std::collections::HashMap<String, String>>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    let force = query.get("force").map(|v| v == "true").unwrap_or(false);
    handler_result_to_response(
        async {
            let db = op.get_db_public().await.typed_handler_err()?;
            let schemas = db
                .schema_manager
                .get_schemas_with_states()
                .typed_handler_err()?;
            drop(db);

            let mut total_applied = 0usize;
            let mut schemas_updated = 0usize;

            for sws in &schemas {
                if sws.state != fold_db::schema::SchemaState::Approved {
                    continue;
                }
                match op
                    .apply_classification_defaults_with_force(&sws.schema.name, force)
                    .await
                {
                    Ok(count) if count > 0 => {
                        total_applied += count;
                        schemas_updated += 1;
                    }
                    _ => {}
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
        .await,
    )
}

// ===== Decline invites =====

/// POST /api/trust/invite/decline — decline a trust invite (record locally)
pub async fn decline_trust_invite(
    body: web::Json<serde_json::Value>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, _node) = node_or_return!(state);
    handler_result_to_response(
        async {
            let token = body
                .get("token")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    fold_db::schema::SchemaError::InvalidData("Missing 'token' field".to_string())
                })
                .typed_handler_err()?;

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
        .await,
    )
}

/// GET /api/trust/invite/declined — list all declined invites
pub async fn list_declined_invites(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, _node) = node_or_return!(state);
    handler_result_to_response(
        async {
            let store = crate::trust::declined_invites::DeclinedInviteStore::load()
                .map_err(fold_db::schema::SchemaError::InvalidData)
                .typed_handler_err()?;
            Ok(ApiResponse::success(
                serde_json::json!({"declined_invites": store.invites}),
            ))
        }
        .await,
    )
}

/// DELETE /api/trust/invite/declined/{nonce} — undo a decline (change mind)
pub async fn undecline_invite(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let nonce = path.into_inner();
    let (_user_hash, _node) = node_or_return!(state);
    handler_result_to_response(
        async {
            let mut store = crate::trust::declined_invites::DeclinedInviteStore::load()
                .map_err(fold_db::schema::SchemaError::InvalidData)
                .typed_handler_err()?;
            let removed = store.undecline(&nonce);
            store
                .save()
                .map_err(fold_db::schema::SchemaError::InvalidData)
                .typed_handler_err()?;
            Ok(ApiResponse::success(
                serde_json::json!({"undeclined": removed}),
            ))
        }
        .await,
    )
}

// ===== Sent invites =====

/// GET /api/trust/invite/sent — list all sent invites with status
pub async fn list_sent_invites(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, _node) = node_or_return!(state);
    handler_result_to_response(
        async {
            let store = crate::trust::sent_invites::SentInviteStore::load()
                .map_err(fold_db::schema::SchemaError::InvalidData)
                .typed_handler_err()?;
            Ok(ApiResponse::success(
                serde_json::json!({"sent_invites": store.invites}),
            ))
        }
        .await,
    )
}

/// GET /api/sharing/exemem-status — check Exemem connectivity and token validity
pub async fn exemem_status(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, _node) = node_or_return!(state);

    let config_result = get_discovery_config_and_token();
    match config_result {
        Err(msg) => actix_web::HttpResponse::Ok().json(serde_json::json!({
            "connected": false,
            "reason": msg,
        })),
        Ok((url, _key, token)) => {
            // Check token expiry
            let parts: Vec<&str> = token.split('.').collect();
            let token_info = if parts.len() >= 3 {
                if let Ok(expiry) = parts[2].parse::<i64>() {
                    let now = chrono::Utc::now().timestamp();
                    let remaining = expiry - now;
                    if remaining <= 0 {
                        serde_json::json!({"valid": false, "expired_ago_secs": -remaining})
                    } else {
                        serde_json::json!({"valid": true, "expires_in_secs": remaining})
                    }
                } else {
                    serde_json::json!({"valid": true, "format": "unknown"})
                }
            } else {
                serde_json::json!({"valid": true, "format": "opaque"})
            };

            actix_web::HttpResponse::Ok().json(serde_json::json!({
                "connected": true,
                "discovery_url": url,
                "token": token_info,
            }))
        }
    }
}
