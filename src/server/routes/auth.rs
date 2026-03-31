use actix_web::{web, HttpResponse};
use serde::{Deserialize, Serialize};

use crate::keychain;
use crate::server::http_server::AppState;
use crate::server::routes::common::require_node;

fn exemem_api_url() -> String {
    std::env::var("EXEMEM_API_URL")
        .unwrap_or_else(|_| "https://ygyu7ritx8.execute-api.us-west-2.amazonaws.com".to_string())
}

// ============================================================================
// Request/Response types
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct MagicLinkStartRequest {
    pub email: String,
}

#[derive(Debug, Deserialize)]
pub struct MagicLinkVerifyRequest {
    pub email: String,
    pub code: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StoreCredentialsRequest {
    pub user_hash: String,
    pub session_token: String,
    pub api_key: String,
    pub encryption_key: String,
}

// ============================================================================
// Proxy routes — forward to Exemem API
// ============================================================================

/// POST /api/auth/magic-link/start
/// Proxy to Exemem auth service to send verification email.
pub async fn magic_link_start(body: web::Json<MagicLinkStartRequest>) -> HttpResponse {
    let client = reqwest::Client::new();
    let url = format!("{}/api/auth/magic-link/start", exemem_api_url());

    match client
        .post(&url)
        .json(&serde_json::json!({ "email": body.email }))
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            match resp.text().await {
                Ok(text) => {
                    let json: serde_json::Value = serde_json::from_str(&text)
                        .unwrap_or(serde_json::json!({"ok": false, "error": text}));
                    HttpResponse::build(
                        actix_web::http::StatusCode::from_u16(status.as_u16())
                            .unwrap_or(actix_web::http::StatusCode::INTERNAL_SERVER_ERROR),
                    )
                    .json(json)
                }
                Err(e) => HttpResponse::BadGateway().json(serde_json::json!({
                    "ok": false,
                    "error": format!("Failed to read response: {}", e)
                })),
            }
        }
        Err(e) => HttpResponse::BadGateway().json(serde_json::json!({
            "ok": false,
            "error": format!("Failed to connect to Exemem API: {}", e)
        })),
    }
}

/// POST /api/auth/magic-link/verify
/// Proxy to Exemem auth service, then store credentials in keychain on success.
pub async fn magic_link_verify(body: web::Json<MagicLinkVerifyRequest>) -> HttpResponse {
    let client = reqwest::Client::new();
    let url = format!("{}/api/auth/magic-link/verify", exemem_api_url());

    match client
        .post(&url)
        .json(&serde_json::json!({
            "email": body.email,
            "code": body.code
        }))
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            match resp.text().await {
                Ok(text) => {
                    let json: serde_json::Value = serde_json::from_str(&text)
                        .unwrap_or(serde_json::json!({"ok": false, "error": text}));

                    // If verification succeeded, store credentials in keychain
                    if json.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                        if let (Some(user_hash), Some(session_token), Some(api_key)) = (
                            json.get("user_hash").and_then(|v| v.as_str()),
                            json.get("session_token").and_then(|v| v.as_str()),
                            json.get("api_key").and_then(|v| v.as_str()),
                        ) {
                            let creds = keychain::ExememCredentials {
                                user_hash: user_hash.to_string(),
                                session_token: session_token.to_string(),
                                api_key: api_key.to_string(),
                                encryption_key: String::new(),
                            };
                            if let Err(e) = keychain::store_credentials(&creds) {
                                log::warn!("Failed to store credentials in keychain: {}", e);
                            }
                        }
                    }

                    HttpResponse::build(
                        actix_web::http::StatusCode::from_u16(status.as_u16())
                            .unwrap_or(actix_web::http::StatusCode::INTERNAL_SERVER_ERROR),
                    )
                    .json(json)
                }
                Err(e) => HttpResponse::BadGateway().json(serde_json::json!({
                    "ok": false,
                    "error": format!("Failed to read response: {}", e)
                })),
            }
        }
        Err(e) => HttpResponse::BadGateway().json(serde_json::json!({
            "ok": false,
            "error": format!("Failed to connect to Exemem API: {}", e)
        })),
    }
}

// ============================================================================
// Keychain routes — local credential management
// ============================================================================

/// GET /api/auth/credentials
/// Check if credentials exist in the keychain.
pub async fn get_credentials() -> HttpResponse {
    match keychain::load_credentials() {
        Ok(Some(creds)) => HttpResponse::Ok().json(serde_json::json!({
            "ok": true,
            "has_credentials": true,
            "user_hash": creds.user_hash,
        })),
        Ok(None) => HttpResponse::Ok().json(serde_json::json!({
            "ok": true,
            "has_credentials": false,
        })),
        Err(e) => HttpResponse::InternalServerError().json(serde_json::json!({
            "ok": false,
            "error": format!("Failed to check credentials: {}", e)
        })),
    }
}

/// POST /api/auth/credentials
/// Store credentials in the keychain (called after verify with encryption key).
pub async fn store_credentials(body: web::Json<StoreCredentialsRequest>) -> HttpResponse {
    let creds = keychain::ExememCredentials {
        user_hash: body.user_hash.clone(),
        session_token: body.session_token.clone(),
        api_key: body.api_key.clone(),
        encryption_key: body.encryption_key.clone(),
    };

    match keychain::store_credentials(&creds) {
        Ok(()) => HttpResponse::Ok().json(serde_json::json!({"ok": true})),
        Err(e) => HttpResponse::InternalServerError().json(serde_json::json!({
            "ok": false,
            "error": format!("Failed to store credentials: {}", e)
        })),
    }
}

/// DELETE /api/auth/credentials
/// Delete credentials from the keychain (logout).
pub async fn delete_credentials() -> HttpResponse {
    match keychain::delete_credentials() {
        Ok(()) => HttpResponse::Ok().json(serde_json::json!({"ok": true})),
        Err(e) => HttpResponse::InternalServerError().json(serde_json::json!({
            "ok": false,
            "error": format!("Failed to delete credentials: {}", e)
        })),
    }
}

// ============================================================================
// Exemem config & registration
// ============================================================================

/// GET /api/auth/exemem-config
/// Return the Exemem API URL so the frontend doesn't need to hardcode it.
pub async fn get_exemem_config() -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({
        "ok": true,
        "api_url": exemem_api_url(),
    }))
}

/// POST /api/auth/register
/// Register this node's public key with Exemem to create a cloud account.
/// Uses the node's Ed25519 public key — no email required.
pub async fn register_with_exemem(data: web::Data<AppState>) -> HttpResponse {
    // Get the node's public key (base64-encoded)
    let (_user_hash, node_arc) = match require_node(&data).await {
        Ok(res) => res,
        Err(response) => return response,
    };
    let node = node_arc.read().await;
    let public_key_b64 = node.get_node_public_key().to_string();
    drop(node);

    // Decode base64 → bytes → hex (CLI register endpoint expects hex)
    let public_key_hex = match base64_to_hex(&public_key_b64) {
        Some(hex) => hex,
        None => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "ok": false,
                "error": "Failed to decode node public key from base64"
            }));
        }
    };

    // Call Exemem CLI register endpoint
    let client = reqwest::Client::new();
    let url = format!("{}/api/auth/cli/register", exemem_api_url());

    match client
        .post(&url)
        .json(&serde_json::json!({ "public_key": public_key_hex }))
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            match resp.text().await {
                Ok(text) => {
                    let json: serde_json::Value = serde_json::from_str(&text)
                        .unwrap_or(serde_json::json!({"ok": false, "error": text}));

                    // Store credentials in keychain on success
                    if json.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                        if let (Some(user_hash), Some(session_token), Some(api_key)) = (
                            json.get("user_hash").and_then(|v| v.as_str()),
                            json.get("session_token").and_then(|v| v.as_str()),
                            json.get("api_key").and_then(|v| v.as_str()),
                        ) {
                            let creds = keychain::ExememCredentials {
                                user_hash: user_hash.to_string(),
                                session_token: session_token.to_string(),
                                api_key: api_key.to_string(),
                                encryption_key: String::new(),
                            };
                            if let Err(e) = keychain::store_credentials(&creds) {
                                log::warn!("Failed to store credentials in keychain: {}", e);
                            }
                        }
                    }

                    // Include the API URL in the response so frontend can use it
                    let mut response = json.clone();
                    if let Some(obj) = response.as_object_mut() {
                        obj.insert(
                            "api_url".to_string(),
                            serde_json::Value::String(exemem_api_url()),
                        );
                    }

                    HttpResponse::build(
                        actix_web::http::StatusCode::from_u16(status.as_u16())
                            .unwrap_or(actix_web::http::StatusCode::INTERNAL_SERVER_ERROR),
                    )
                    .json(response)
                }
                Err(e) => HttpResponse::BadGateway().json(serde_json::json!({
                    "ok": false,
                    "error": format!("Failed to read response: {}", e)
                })),
            }
        }
        Err(e) => HttpResponse::BadGateway().json(serde_json::json!({
            "ok": false,
            "error": format!("Failed to connect to Exemem API: {}", e)
        })),
    }
}

/// Decode a base64 string to hex. Returns None on invalid base64.
fn base64_to_hex(b64: &str) -> Option<String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(b64))
        .ok()?;
    Some(hex::encode(bytes))
}
