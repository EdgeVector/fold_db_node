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

// ============================================================================
// Recovery phrase (BIP39 mnemonic for device transfer)
// ============================================================================

/// GET /api/auth/recovery-phrase
/// Returns the node's Ed25519 private key as 24 BIP39 mnemonic words.
/// Local-only endpoint — the key never leaves the device over the network.
pub async fn get_recovery_phrase(data: web::Data<AppState>) -> HttpResponse {
    let private_key_b64 = match data.node_manager.get_base_config().await.private_key {
        Some(pk) => pk,
        None => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "ok": false,
                "error": "Node private key not available"
            }));
        }
    };

    use base64::Engine;
    let seed_bytes = match base64::engine::general_purpose::STANDARD.decode(&private_key_b64) {
        Ok(bytes) if bytes.len() == 32 => bytes,
        _ => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "ok": false,
                "error": "Invalid private key format"
            }));
        }
    };

    let mnemonic = match bip39::Mnemonic::from_entropy(&seed_bytes) {
        Ok(m) => m,
        Err(e) => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "ok": false,
                "error": format!("Failed to generate mnemonic: {}", e)
            }));
        }
    };

    let words: Vec<&str> = mnemonic.words().collect();

    HttpResponse::Ok().json(serde_json::json!({
        "ok": true,
        "words": words
    }))
}

/// POST /api/auth/restore
/// Restore node identity from a 24-word BIP39 recovery phrase.
/// Derives Ed25519 keypair, writes identity, registers with Exemem.
pub async fn restore_from_phrase(
    data: web::Data<AppState>,
    body: web::Json<serde_json::Value>,
) -> HttpResponse {
    let words = match body.get("words").and_then(|v| v.as_str()) {
        Some(w) => w.trim().to_string(),
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "ok": false,
                "error": "Missing 'words' field"
            }));
        }
    };

    // Parse mnemonic
    let mnemonic = match bip39::Mnemonic::parse(&words) {
        Ok(m) => m,
        Err(e) => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "ok": false,
                "error": format!("Invalid recovery phrase: {}", e)
            }));
        }
    };

    let entropy = mnemonic.to_entropy();
    if entropy.len() != 32 {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "ok": false,
            "error": format!("Recovery phrase must encode 32 bytes, got {}", entropy.len())
        }));
    }

    // Derive Ed25519 keypair from seed
    let key_pair = match fold_db::security::Ed25519KeyPair::from_secret_key(&entropy) {
        Ok(kp) => kp,
        Err(e) => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "ok": false,
                "error": format!("Failed to derive keypair: {}", e)
            }));
        }
    };

    use base64::Engine;
    let private_key_b64 = base64::engine::general_purpose::STANDARD.encode(&entropy);
    let public_key_b64 =
        base64::engine::general_purpose::STANDARD.encode(key_pair.public_key_bytes());

    // Write identity to disk ($FOLDDB_HOME/config/node_identity.json)
    let identity_path = crate::utils::paths::folddb_home()
        .map(|h| h.join("config").join("node_identity.json"))
        .unwrap_or_else(|_| std::path::PathBuf::from("config/node_identity.json"));
    if let Some(parent) = identity_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let identity_json = serde_json::json!({
        "private_key": private_key_b64,
        "public_key": public_key_b64,
    });

    if let Err(e) = std::fs::write(
        &identity_path,
        serde_json::to_string_pretty(&identity_json).unwrap(),
    ) {
        return HttpResponse::InternalServerError().json(serde_json::json!({
            "ok": false,
            "error": format!("Failed to write identity: {}", e)
        }));
    }

    // Grab the Sled database handle BEFORE update_config() invalidates all nodes.
    // Sled's Db is Arc-wrapped internally, so this clone keeps the database open
    // even after invalidation clears the shared node. This avoids the Sled
    // exclusive file lock issue that plagued PRs #196-#200.
    let sled_db = data.node_manager.get_sled_db().await;

    // Update the in-memory config so signed_register uses the restored key.
    // This calls invalidate_all_nodes(), clearing the shared node — but our
    // cloned sled_db handle above keeps the database alive.
    let mut base_config = data.node_manager.get_base_config().await;
    base_config.public_key = Some(public_key_b64.clone());
    base_config.private_key = Some(private_key_b64.clone());
    data.node_manager
        .update_config(crate::server::node_manager::NodeManagerConfig { base_config })
        .await;

    // Register with Exemem (idempotent — returns fresh token for existing users)
    match signed_register(&data, None).await {
        Ok(json) => {
            let mut response = json;
            if let Some(obj) = response.as_object_mut() {
                obj.insert(
                    "api_url".to_string(),
                    serde_json::Value::String(exemem_api_url()),
                );
            }

            // Spawn background bootstrap if we got Exemem credentials and a Sled handle.
            // This downloads the latest snapshot + replays write logs from R2
            // so the restored node has the full database, not just the identity.
            if let (Some(api_key), Some(_user_hash), Some(sled_db)) = (
                response.get("api_key").and_then(|v| v.as_str()),
                response.get("user_hash").and_then(|v| v.as_str()),
                sled_db,
            ) {
                let api_url = exemem_api_url();
                let api_key = api_key.to_string();
                let node_manager = data.node_manager.clone();

                tokio::spawn(async move {
                    if let Err(e) =
                        bootstrap_from_cloud(&api_url, &api_key, &node_manager, sled_db).await
                    {
                        log::error!("Background bootstrap after restore failed: {}", e);
                    }
                });
            }

            HttpResponse::Ok().json(response)
        }
        Err(e) => HttpResponse::InternalServerError().json(serde_json::json!({
            "ok": false,
            "error": e
        })),
    }
}

/// Bootstrap the local database from cloud storage (R2/S3).
///
/// Accepts a pre-cloned Sled `Db` handle from the running server's shared node.
/// Sled's `Db` is internally `Arc`-wrapped, so cloning it shares the same open
/// database — no exclusive file lock conflict. The caller grabs this handle
/// BEFORE `update_config()` / `invalidate_all_nodes()` so it stays alive even
/// after the shared node is cleared.
///
/// Downloads the latest snapshot and replays any write logs after it,
/// restoring the full database state on a new/recovered device.
async fn bootstrap_from_cloud(
    api_url: &str,
    api_key: &str,
    node_manager: &std::sync::Arc<crate::server::node_manager::NodeManager>,
    sled_db: sled::Db,
) -> Result<(), String> {
    log::info!("Starting database bootstrap from cloud after identity restore");

    // Load E2E encryption keys (needed to decrypt the snapshot + logs)
    let folddb_home = crate::utils::paths::folddb_home()
        .map_err(|e| format!("Cannot resolve FOLDDB_HOME: {e}"))?;
    let e2e_key_path = folddb_home.join("e2e.key");
    let e2e_keys = fold_db::crypto::E2eKeys::load_or_generate(&e2e_key_path)
        .await
        .map_err(|e| format!("Failed to load E2E keys: {e}"))?;

    // Resolve the storage path (same logic as factory/node_manager)
    let config = node_manager.get_base_config().await;
    let data_dir = config.get_storage_path();
    let data_dir_str = data_dir
        .to_str()
        .ok_or_else(|| "Invalid storage path".to_string())?;

    // Build sync components
    let sync_setup = fold_db::sync::SyncSetup::from_exemem(api_url, api_key, data_dir_str);
    let sync_crypto: std::sync::Arc<dyn fold_db::crypto::CryptoProvider> = std::sync::Arc::new(
        fold_db::crypto::LocalCryptoProvider::from_key(e2e_keys.encryption_key()),
    );

    let http = std::sync::Arc::new(reqwest::Client::new());
    let s3 = fold_db::sync::s3::S3Client::new(http.clone());
    let auth = fold_db::sync::auth::AuthClient::new(http, sync_setup.auth_url, sync_setup.auth);

    // Use the pre-cloned Sled handle directly — no sled::open() needed.
    // This avoids the exclusive file lock issue since we share the same
    // database instance the server already holds open.
    let base_store: std::sync::Arc<dyn fold_db::storage::traits::NamespacedStore> =
        std::sync::Arc::new(fold_db::storage::SledNamespacedStore::new(sled_db));

    let engine = std::sync::Arc::new(fold_db::sync::SyncEngine::new(
        sync_setup.device_id,
        sync_crypto,
        s3,
        auth,
        base_store,
        fold_db::sync::SyncConfig::default(),
    ));

    // Run bootstrap (download snapshot + replay logs)
    let final_seq = engine
        .bootstrap()
        .await
        .map_err(|e| format!("Bootstrap failed: {e}"))?;

    log::info!(
        "Database bootstrap complete after restore: final sequence = {}",
        final_seq
    );

    Ok(())
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
