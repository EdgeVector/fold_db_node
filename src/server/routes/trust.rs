//! HTTP routes for trust management, field policies, identity cards,
//! contacts, trust invites, sharing roles, and audit log.
//!
//! All business logic lives in `crate::handlers::trust`. This file is
//! a thin HTTP-extraction layer: it pulls query/body/path data from
//! `HttpRequest`, calls the shared handler, and maps `HandlerResult<T>`
//! into `HttpResponse`.

use crate::handlers::trust as trust_handlers;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_result_to_response, node_or_return};
use actix_web::{web, HttpResponse, Responder};

/// Fix base64 public keys that get mangled by URL path decoding.
/// Actix decodes `%2B` → `+` correctly, but some clients/proxies
/// decode `+` → space. This reverses that corruption.
fn fix_pubkey_from_path(key: &str) -> String {
    key.replace(' ', "+")
}

// ===== Trust management =====

/// POST /api/trust/grant
pub async fn grant_trust(
    body: web::Json<trust_handlers::TrustGrantRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(trust_handlers::grant_trust(&body, &user_hash, &node).await)
}

/// DELETE /api/trust/revoke/{key}
pub async fn revoke_trust(path: web::Path<String>, state: web::Data<AppState>) -> impl Responder {
    let public_key = fix_pubkey_from_path(&path.into_inner());
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(trust_handlers::revoke_trust(&public_key, &user_hash, &node).await)
}

/// GET /api/trust/grants
pub async fn list_trust_grants(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(trust_handlers::list_trust_grants(&user_hash, &node).await)
}

/// GET /api/trust/resolve/{key}
pub async fn resolve_trust(path: web::Path<String>, state: web::Data<AppState>) -> impl Responder {
    let public_key = fix_pubkey_from_path(&path.into_inner());
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(trust_handlers::resolve_trust(public_key, &user_hash, &node).await)
}

// ===== Schema policy =====

/// PUT /api/schema/{name}/field/{field}/policy
pub async fn set_field_policy(
    path: web::Path<(String, String)>,
    body: web::Json<trust_handlers::SetFieldPolicyRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (schema_name, field_name) = path.into_inner();
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(
        trust_handlers::set_field_policy(
            &schema_name,
            &field_name,
            body.into_inner(),
            &user_hash,
            &node,
        )
        .await,
    )
}

/// GET /api/schema/{name}/field/{field}/policy
pub async fn get_field_policy(
    path: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (schema_name, field_name) = path.into_inner();
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(
        trust_handlers::get_field_policy(schema_name, field_name, &user_hash, &node).await,
    )
}

/// GET /api/schema/{name}/policies
pub async fn get_all_field_policies(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let schema_name = path.into_inner();
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(
        trust_handlers::get_all_field_policies(schema_name, &user_hash, &node).await,
    )
}

// ===== Audit log =====

/// GET /api/trust/audit?limit=100
pub async fn get_audit_log(
    query: web::Query<trust_handlers::AuditLogQuery>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let limit = query.limit.unwrap_or(100);
    handler_result_to_response(trust_handlers::get_audit_log(limit, &user_hash, &node).await)
}

// ===== Identity card =====

/// GET /api/identity/card
pub async fn get_identity_card(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(trust_handlers::get_identity_card(&user_hash, &node).await)
}

/// PUT /api/identity/card
pub async fn set_identity_card(
    body: web::Json<trust_handlers::SetIdentityCardRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(
        trust_handlers::set_identity_card(body.into_inner(), &user_hash, &node).await,
    )
}

// ===== Contacts =====

/// GET /api/contacts
pub async fn list_contacts(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(trust_handlers::list_contacts(&user_hash, &node).await)
}

/// GET /api/contacts/{key}
pub async fn get_contact(path: web::Path<String>, state: web::Data<AppState>) -> impl Responder {
    let public_key = fix_pubkey_from_path(&path.into_inner());
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(trust_handlers::get_contact(&public_key, &user_hash, &node).await)
}

/// DELETE /api/contacts/{key}
pub async fn revoke_contact(path: web::Path<String>, state: web::Data<AppState>) -> impl Responder {
    let public_key = fix_pubkey_from_path(&path.into_inner());
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(trust_handlers::revoke_contact(&public_key, &user_hash, &node).await)
}

// ===== Trust invites =====

/// POST /api/trust/invite
pub async fn create_trust_invite(
    body: web::Json<trust_handlers::CreateInviteRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(trust_handlers::create_trust_invite(&body, &user_hash, &node).await)
}

/// POST /api/trust/invite/accept
pub async fn accept_trust_invite(
    body: web::Json<trust_handlers::AcceptInviteRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(
        trust_handlers::accept_trust_invite(body.into_inner(), &user_hash, &node).await,
    )
}

/// POST /api/trust/invite/preview
pub async fn preview_trust_invite(
    body: web::Json<serde_json::Value>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, _node) = node_or_return!(state);
    let token = match body.get("token").and_then(|v| v.as_str()) {
        Some(t) => t.to_string(),
        None => {
            return HttpResponse::BadRequest()
                .json(serde_json::json!({"error": "Missing 'token' field"}));
        }
    };
    handler_result_to_response(trust_handlers::preview_trust_invite(&token).await)
}

// ===== Trust invite relay (via Exemem discovery service) =====

/// Helper: get discovery URL, master key, and auth token from the explicit
/// `AppState` resolver plus env/keychain credentials.
async fn get_discovery_config_and_token(
    state: &AppState,
) -> Result<(String, Vec<u8>, String), String> {
    let cloud_required_msg = "Email invites and link sharing require Exemem cloud backup. \
         Enable cloud backup in Settings to use these features.";
    let cfg = state
        .discovery_config()
        .await
        .ok_or_else(|| cloud_required_msg.to_string())?;
    let url = cfg.url;
    let key = cfg.master_key;

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

/// Build a DiscoveryPublisher from the `AppState` discovery resolver,
/// returning 503 on failure.
async fn require_publisher(
    state: &AppState,
) -> Result<crate::discovery::publisher::DiscoveryPublisher, HttpResponse> {
    let (url, key, token) = get_discovery_config_and_token(state)
        .await
        .map_err(|e| HttpResponse::ServiceUnavailable().json(serde_json::json!({"error": e})))?;
    Ok(crate::discovery::publisher::DiscoveryPublisher::new(
        key, url, token,
    ))
}

/// Extract a required string field from a JSON body, returning 400 on absence.
fn require_json_str<'a>(body: &'a serde_json::Value, field: &str) -> Result<&'a str, HttpResponse> {
    body.get(field).and_then(|v| v.as_str()).ok_or_else(|| {
        HttpResponse::BadRequest()
            .json(serde_json::json!({"error": format!("Missing '{}' field", field)}))
    })
}

/// POST /api/trust/invite/share
pub async fn share_trust_invite(
    body: web::Json<serde_json::Value>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, _node) = node_or_return!(state);
    let publisher = match require_publisher(&state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let invite_token = match require_json_str(&body, "token") {
        Ok(t) => t.to_string(),
        Err(r) => return r,
    };
    handler_result_to_response(
        trust_handlers::share_trust_invite(&publisher, &invite_token, &user_hash).await,
    )
}

/// GET /api/trust/invite/fetch?id=xxx
pub async fn fetch_shared_invite(
    query: web::Query<std::collections::HashMap<String, String>>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, _node) = node_or_return!(state);
    let publisher = match require_publisher(&state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let invite_id = match query.get("id") {
        Some(id) => id.clone(),
        None => {
            return HttpResponse::BadRequest()
                .json(serde_json::json!({"error": "Missing 'id' query parameter"}));
        }
    };
    handler_result_to_response(trust_handlers::fetch_shared_invite(&publisher, &invite_id).await)
}

/// POST /api/trust/invite/send-verified
pub async fn send_verified_invite(
    body: web::Json<serde_json::Value>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, _node) = node_or_return!(state);
    let publisher = match require_publisher(&state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let invite_token = match require_json_str(&body, "token") {
        Ok(t) => t.to_string(),
        Err(r) => return r,
    };
    let recipient_email = match require_json_str(&body, "recipient_email") {
        Ok(e) => e.to_string(),
        Err(r) => return r,
    };
    let sender_name = match require_json_str(&body, "sender_name") {
        Ok(n) => n.to_string(),
        Err(r) => return r,
    };
    handler_result_to_response(
        trust_handlers::send_verified_invite(
            &publisher,
            &invite_token,
            &recipient_email,
            &sender_name,
            &user_hash,
        )
        .await,
    )
}

/// POST /api/trust/invite/verify
pub async fn verify_invite_code(
    body: web::Json<serde_json::Value>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, _node) = node_or_return!(state);
    let publisher = match require_publisher(&state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let invite_id = match require_json_str(&body, "invite_id") {
        Ok(id) => id.to_string(),
        Err(r) => return r,
    };
    let code = match require_json_str(&body, "code") {
        Ok(c) => c.to_string(),
        Err(r) => return r,
    };
    handler_result_to_response(
        trust_handlers::verify_invite_code(&publisher, &invite_id, &code).await,
    )
}

// ===== Sharing roles =====

/// GET /api/sharing/roles
pub async fn list_sharing_roles(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, _node) = node_or_return!(state);
    handler_result_to_response(trust_handlers::list_sharing_roles())
}

/// POST /api/contacts/{key}/role
pub async fn assign_contact_role(
    path: web::Path<String>,
    body: web::Json<trust_handlers::AssignRoleRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let public_key = fix_pubkey_from_path(&path.into_inner());
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(
        trust_handlers::assign_contact_role(&public_key, &body, &user_hash, &node).await,
    )
}

/// DELETE /api/contacts/{key}/role/{domain}
pub async fn remove_contact_role(
    path: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (pk_raw, domain) = path.into_inner();
    let public_key = fix_pubkey_from_path(&pk_raw);
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(
        trust_handlers::remove_contact_role(&public_key, &domain, &user_hash, &node).await,
    )
}

/// GET /api/sharing/audit/{key}
pub async fn sharing_audit(path: web::Path<String>, state: web::Data<AppState>) -> impl Responder {
    let public_key = fix_pubkey_from_path(&path.into_inner());
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(trust_handlers::sharing_audit(&public_key, &user_hash, &node).await)
}

/// GET /api/sharing/posture
pub async fn sharing_posture(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(trust_handlers::sharing_posture(&user_hash, &node).await)
}

/// POST /api/sharing/apply-defaults
pub async fn apply_defaults_all(
    query: web::Query<std::collections::HashMap<String, String>>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let force = query.get("force").map(|v| v == "true").unwrap_or(false);
    handler_result_to_response(trust_handlers::apply_defaults_all(force, &user_hash, &node).await)
}

// ===== Declined invites =====

/// POST /api/trust/invite/decline
pub async fn decline_trust_invite(
    body: web::Json<serde_json::Value>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, _node) = node_or_return!(state);
    let token = match body.get("token").and_then(|v| v.as_str()) {
        Some(t) => t.to_string(),
        None => {
            return HttpResponse::BadRequest()
                .json(serde_json::json!({"error": "Missing 'token' field"}));
        }
    };
    handler_result_to_response(trust_handlers::decline_trust_invite(&token).await)
}

/// GET /api/trust/invite/declined
pub async fn list_declined_invites(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, _node) = node_or_return!(state);
    handler_result_to_response(trust_handlers::list_declined_invites().await)
}

/// DELETE /api/trust/invite/declined/{nonce}
pub async fn undecline_invite(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let nonce = path.into_inner();
    let (_user_hash, _node) = node_or_return!(state);
    handler_result_to_response(trust_handlers::undecline_invite(&nonce).await)
}

// ===== Sent invites =====

/// GET /api/trust/invite/sent
pub async fn list_sent_invites(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, _node) = node_or_return!(state);
    handler_result_to_response(trust_handlers::list_sent_invites().await)
}

/// GET /api/sharing/exemem-status — check Exemem connectivity and token validity
pub async fn exemem_status(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, _node) = node_or_return!(state);

    let config_result = get_discovery_config_and_token(&state).await;
    match config_result {
        Err(msg) => HttpResponse::Ok().json(serde_json::json!({
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

            HttpResponse::Ok().json(serde_json::json!({
                "connected": true,
                "discovery_url": url,
                "token": token_info,
            }))
        }
    }
}
