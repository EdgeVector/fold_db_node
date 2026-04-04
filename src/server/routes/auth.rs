use actix_web::{web, HttpResponse};
use serde::{Deserialize, Serialize};

use crate::keychain;
use crate::server::http_server::AppState;

pub(crate) fn exemem_api_url() -> String {
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
/// Proxy to Exemem auth service, then store credentials locally on success.
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

                    // If verification succeeded, store credentials locally
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
                                log::warn!("Failed to store credentials locally: {}", e);
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
// Credential routes — local credential management
// ============================================================================

/// GET /api/auth/credentials
/// Check if credentials exist locally.
pub async fn get_credentials() -> HttpResponse {
    match keychain::load_credentials() {
        Ok(Some(creds)) => HttpResponse::Ok().json(serde_json::json!({
            "ok": true,
            "has_credentials": true,
            "user_hash": creds.user_hash,
            "session_token": creds.session_token,
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
/// Store credentials locally (called after verify with encryption key).
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
/// Delete credentials from local storage (logout).
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
/// Signs the request with the node's Ed25519 private key to prove key ownership.
/// Idempotent: if already registered, returns a fresh session token.
/// Accepts optional JSON body with `invite_code` for new registrations.
pub async fn register_with_exemem(
    data: web::Data<AppState>,
    body: web::Json<serde_json::Value>,
) -> HttpResponse {
    let invite_code = body
        .get("invite_code")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    match signed_register(&data, invite_code.as_deref()).await {
        Ok(json) => {
            // Include the API URL in the response so frontend can use it
            let mut response = json;
            if let Some(obj) = response.as_object_mut() {
                obj.insert(
                    "api_url".to_string(),
                    serde_json::Value::String(exemem_api_url()),
                );
            }
            HttpResponse::Ok().json(response)
        }
        Err(e) => HttpResponse::InternalServerError().json(serde_json::json!({
            "ok": false,
            "error": e
        })),
    }
}

/// Refresh the session token by calling the signed register endpoint.
/// The register endpoint is idempotent: for existing users with a valid
/// signature, it returns a fresh session token + new API key.
///
/// Returns the new session token on success.
pub async fn refresh_session_token(data: &web::Data<AppState>) -> Result<String, String> {
    let json = signed_register(data, None).await?;

    json.get("session_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "Missing session_token in register response".to_string())
}

/// Core signed register logic shared by register_with_exemem and refresh_session_token.
///
/// Signs "{public_key_hex}:{timestamp}" with the node's Ed25519 private key,
/// sends to the Exemem CLI register endpoint, and stores credentials.
async fn signed_register(
    data: &web::Data<AppState>,
    invite_code: Option<&str>,
) -> Result<serde_json::Value, String> {
    // Get the node's keys from identity (works even during onboarding before user context)
    let public_key_b64 = data
        .node_manager
        .ensure_default_identity()
        .await
        .map_err(|e| format!("Failed to initialize node identity: {e}"))?;

    let private_key_b64 = data
        .node_manager
        .get_base_config()
        .await
        .private_key
        .ok_or("Node private key not available".to_string())?;

    // Decode base64 → hex (CLI register endpoint expects hex)
    let public_key_hex =
        base64_to_hex(&public_key_b64).ok_or("Failed to decode node public key from base64")?;

    // Sign: "{public_key_hex}:{timestamp}"
    // Must match auth_service/src/cli/types.rs::verify_ed25519_signature()
    let timestamp = chrono::Utc::now().timestamp();
    let payload = format!("{}:{}", public_key_hex, timestamp);
    let signature_b64 = sign_payload(&private_key_b64, &payload)?;

    // Call Exemem CLI register endpoint with signature
    let client = reqwest::Client::new();
    let url = format!("{}/api/auth/cli/register", exemem_api_url());

    let mut register_body = serde_json::json!({
        "public_key": public_key_hex,
        "timestamp": timestamp,
        "signature": signature_b64
    });
    if let Some(code) = invite_code {
        register_body["invite_code"] = serde_json::Value::String(code.to_string());
    }

    let resp = client
        .post(&url)
        .json(&register_body)
        .send()
        .await
        .map_err(|e| format!("Failed to connect to Exemem API: {}", e))?;

    let text = resp
        .text()
        .await
        .map_err(|e| format!("Failed to read response: {}", e))?;

    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| format!("Invalid JSON response: {}", text))?;

    if !json.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        let error = json
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown error");
        return Err(format!("Register failed: {}", error));
    }

    // Store credentials on success
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
            log::warn!("Failed to store credentials locally: {}", e);
        }
    }

    Ok(json)
}

/// Sign a payload with the node's Ed25519 private key.
/// Returns the base64-encoded signature.
fn sign_payload(private_key_b64: &str, payload: &str) -> Result<String, String> {
    use base64::Engine;
    let key_bytes = base64::engine::general_purpose::STANDARD
        .decode(private_key_b64)
        .map_err(|e| format!("Failed to decode private key: {}", e))?;
    let key_pair = fold_db::security::Ed25519KeyPair::from_secret_key(&key_bytes)
        .map_err(|e| format!("Failed to create key pair: {}", e))?;
    let signature = key_pair.sign(payload.as_bytes());
    Ok(fold_db::security::KeyUtils::signature_to_base64(&signature))
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
