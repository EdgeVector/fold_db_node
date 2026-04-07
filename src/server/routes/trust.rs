use crate::fold_node::OperationProcessor;
use crate::handlers::{ApiResponse, IntoHandlerError};
use crate::server::http_server::AppState;
use crate::server::routes::{handler_result_to_response, node_or_return};
use crate::trust::trust_invite::TrustInvite;
use actix_web::{web, Responder};
use serde::{Deserialize, Serialize};

// ===== Request/Response types =====

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustGrantRequest {
    pub public_key: String,
    pub distance: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustOverrideRequest {
    pub public_key: String,
    pub distance: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustGrantsResponse {
    pub grants: Vec<TrustGrantEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustGrantEntry {
    pub public_key: String,
    pub distance: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustResolveResponse {
    pub public_key: String,
    pub distance: Option<u64>,
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

/// POST /api/trust/grant — assign trust to a public key at a distance
pub async fn grant_trust(
    body: web::Json<TrustGrantRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            op.grant_trust(&body.public_key, body.distance)
                .await
                .handler_err("grant trust")?;
            Ok(ApiResponse::success_with_user(
                serde_json::json!({"granted": true}),
                user_hash,
            ))
        }
        .await,
    )
}

/// DELETE /api/trust/revoke/{key} — revoke trust for a public key
pub async fn revoke_trust(path: web::Path<String>, state: web::Data<AppState>) -> impl Responder {
    let public_key = path.into_inner();
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            op.revoke_trust(&public_key)
                .await
                .handler_err("revoke trust")?;
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
            let grants = op.list_trust_grants().await.handler_err("list grants")?;
            let entries: Vec<TrustGrantEntry> = grants
                .into_iter()
                .map(|(public_key, distance)| TrustGrantEntry {
                    public_key,
                    distance,
                })
                .collect();
            Ok(ApiResponse::success_with_user(
                TrustGrantsResponse { grants: entries },
                user_hash,
            ))
        }
        .await,
    )
}

/// PUT /api/trust/override — set explicit distance override
pub async fn set_trust_override(
    body: web::Json<TrustOverrideRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            op.set_trust_override(&body.public_key, body.distance)
                .await
                .handler_err("set override")?;
            Ok(ApiResponse::success_with_user(
                serde_json::json!({"override_set": true}),
                user_hash,
            ))
        }
        .await,
    )
}

/// GET /api/trust/resolve/{key} — check resolved distance for a key
pub async fn resolve_trust(path: web::Path<String>, state: web::Data<AppState>) -> impl Responder {
    let public_key = path.into_inner();
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            let distance = op
                .resolve_trust_distance(&public_key)
                .await
                .handler_err("resolve trust")?;
            Ok(ApiResponse::success_with_user(
                TrustResolveResponse {
                    public_key,
                    distance,
                },
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
                .handler_err("set field policy")?;
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
                .handler_err("get field policy")?;
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
                .handler_err("get all field policies")?;
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
            let events = op.get_audit_log(limit).await.handler_err("get audit log")?;
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

// ===== Capability token endpoints =====

#[derive(Debug, Clone, Deserialize)]
pub struct IssueCapabilityRequest {
    pub schema_name: String,
    pub field_name: String,
    pub public_key: String,
    pub kind: serde_json::Value,
    pub quota: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RevokeCapabilityRequest {
    pub schema_name: String,
    pub field_name: String,
    pub public_key: String,
    pub kind: serde_json::Value,
}

/// POST /api/capabilities/issue — issue a capability token
pub async fn issue_capability(
    body: web::Json<IssueCapabilityRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    let req = body.into_inner();
    handler_result_to_response(
        async {
            let constraint = serde_json::json!({
                "public_key": req.public_key,
                "kind": req.kind,
                "quota": req.quota,
            });
            op.issue_capability(&req.schema_name, &req.field_name, constraint)
                .await
                .handler_err("issue capability")?;
            Ok(ApiResponse::success_with_user(
                serde_json::json!({"issued": true}),
                user_hash,
            ))
        }
        .await,
    )
}

/// DELETE /api/capabilities/revoke — revoke a capability token
pub async fn revoke_capability(
    body: web::Json<RevokeCapabilityRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    let req = body.into_inner();
    handler_result_to_response(
        async {
            op.revoke_capability(&req.schema_name, &req.field_name, &req.public_key, req.kind)
                .await
                .handler_err("revoke capability")?;
            Ok(ApiResponse::success_with_user(
                serde_json::json!({"revoked": true}),
                user_hash,
            ))
        }
        .await,
    )
}

/// GET /api/capabilities/list/{schema}/{field} — list capabilities for a field
pub async fn list_capabilities(
    path: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (schema_name, field_name) = path.into_inner();
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            let caps = op
                .list_capabilities(&schema_name, &field_name)
                .await
                .handler_err("list capabilities")?;
            let caps_json = serde_json::to_value(&caps).handler_err("serialize capabilities")?;
            Ok(ApiResponse::success_with_user(caps_json, user_hash))
        }
        .await,
    )
}

// ===== Payment gate endpoints =====

#[derive(Debug, Clone, Deserialize)]
pub struct SetPaymentGateRequest {
    pub gate: serde_json::Value,
}

/// PUT /api/schema/{name}/payment-gate — set payment gate
pub async fn set_payment_gate(
    path: web::Path<String>,
    body: web::Json<SetPaymentGateRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let schema_name = path.into_inner();
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            op.set_payment_gate(&schema_name, body.into_inner().gate)
                .await
                .handler_err("set payment gate")?;
            Ok(ApiResponse::success_with_user(
                serde_json::json!({"payment_gate_set": true}),
                user_hash,
            ))
        }
        .await,
    )
}

/// GET /api/schema/{name}/payment-gate — get payment gate
pub async fn get_payment_gate(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let schema_name = path.into_inner();
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            let gate = op
                .get_payment_gate(&schema_name)
                .await
                .handler_err("get payment gate")?;
            Ok(ApiResponse::success_with_user(
                serde_json::json!({"schema_name": schema_name, "payment_gate": gate}),
                user_hash,
            ))
        }
        .await,
    )
}

// ===== Identity card endpoints =====

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetIdentityCardRequest {
    pub display_name: String,
    #[serde(default)]
    pub contact_hint: Option<String>,
}

/// GET /api/identity/card — get the current identity card
pub async fn get_identity_card(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            let card = op.get_identity_card().handler_err("get identity card")?;
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
            op.set_identity_card(req.display_name, req.contact_hint)
                .handler_err("set identity card")?;
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
            let contacts = op.list_contacts().handler_err("list contacts")?;
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
    let public_key = path.into_inner();
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            let contact = op.get_contact(&public_key).handler_err("get contact")?;
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
    pub proposed_distance: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AcceptInviteRequest {
    /// The trust invite token (base64url).
    pub token: String,
    /// Override the proposed distance (optional).
    #[serde(default)]
    pub accept_distance: Option<u64>,
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
                .create_trust_invite(body.proposed_distance)
                .handler_err("create trust invite")?;
            let token = invite
                .to_token()
                .map_err(fold_db::schema::SchemaError::InvalidData)
                .handler_err("encode invite token")?;
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
                .handler_err("decode invite token")?;

            let reciprocal = op
                .accept_trust_invite(&invite, req.accept_distance, req.trust_back)
                .await
                .handler_err("accept trust invite")?;

            let reciprocal_token = match &reciprocal {
                Some(inv) => Some(
                    inv.to_token()
                        .map_err(fold_db::schema::SchemaError::InvalidData)
                        .handler_err("encode reciprocal token")?,
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
                .handler_err("parse preview request")?;

            let invite = TrustInvite::from_token(token)
                .map_err(fold_db::schema::SchemaError::InvalidData)
                .handler_err("decode invite token")?;

            let valid = invite
                .verify()
                .map_err(fold_db::schema::SchemaError::InvalidData)
                .handler_err("verify invite")?;

            Ok(ApiResponse::success(serde_json::json!({
                "valid": valid,
                "sender": {
                    "display_name": invite.sender_identity.display_name,
                    "contact_hint": invite.sender_identity.contact_hint,
                    "public_key": invite.sender_pub_key,
                    "fingerprint": invite.fingerprint(),
                },
                "proposed_distance": invite.proposed_distance,
                "created_at": invite.created_at,
            })))
        }
        .await,
    )
}

/// DELETE /api/contacts/{key} — revoke trust and remove contact
pub async fn revoke_contact(path: web::Path<String>, state: web::Data<AppState>) -> impl Responder {
    let public_key = path.into_inner();
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            op.revoke_contact(&public_key)
                .await
                .handler_err("revoke contact")?;
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
    let url = std::env::var("DISCOVERY_SERVICE_URL")
        .map_err(|_| "Discovery service not configured (DISCOVERY_SERVICE_URL)".to_string())?;
    let key_hex = std::env::var("DISCOVERY_MASTER_KEY")
        .map_err(|_| "Discovery master key not configured".to_string())?;
    let key = hex::decode(&key_hex).map_err(|_| "Invalid DISCOVERY_MASTER_KEY hex".to_string())?;

    let token = std::env::var("DISCOVERY_AUTH_TOKEN")
        .or_else(|_| {
            crate::keychain::load_credentials()
                .ok()
                .flatten()
                .map(|c| c.session_token)
                .filter(|t| !t.is_empty())
                .ok_or_else(|| "no token".to_string())
        })
        .map_err(|_| "No auth token available for discovery service".to_string())?;

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
