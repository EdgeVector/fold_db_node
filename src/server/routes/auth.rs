use actix_web::{web, HttpResponse};
use serde::{Deserialize, Serialize};

use crate::keychain;

const EXEMEM_API_URL: &str = "https://api.exemem.com";

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
    let url = format!("{}/api/auth/magic-link/start", EXEMEM_API_URL);

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
    let url = format!("{}/api/auth/magic-link/verify", EXEMEM_API_URL);

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
