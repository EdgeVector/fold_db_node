//! Shared Auth Handlers
//!
//! Framework-agnostic business logic for Exemem registration, credential
//! refresh, identity restore from BIP39, and bootstrap-from-cloud.
//!
//! Routes in `server/routes/auth.rs` are thin wrappers that extract request
//! data from HTTP, call these handlers, and convert results to `HttpResponse`.
//! No business logic lives in the route layer.

use crate::handlers::response::HandlerError;
use crate::keychain;
use crate::server::node_manager::NodeManager;
use fold_db::{CloudCredentials, NodeConfigStore};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

pub(crate) fn exemem_api_url() -> String {
    crate::endpoints::exemem_api_url()
}

// ============================================================================
// Request/Response types
// ============================================================================

#[derive(Debug, Deserialize, Serialize)]
pub struct StoreCredentialsRequest {
    pub user_hash: String,
    pub session_token: String,
    pub api_key: String,
}

// ============================================================================
// Credential operations
// ============================================================================

/// Check if credentials exist locally and return a JSON response body.
pub fn get_credentials_response() -> Result<serde_json::Value, HandlerError> {
    match keychain::load_credentials() {
        Ok(Some(creds)) => {
            let api_url = exemem_api_url();
            Ok(serde_json::json!({
                "ok": true,
                "has_credentials": true,
                "user_hash": creds.user_hash,
                "session_token": creds.session_token,
                "api_url": api_url,
                "api_key": creds.api_key,
            }))
        }
        Ok(None) => Ok(serde_json::json!({
            "ok": true,
            "has_credentials": false,
        })),
        Err(e) => Err(HandlerError::Internal(format!(
            "Failed to check credentials: {}",
            e
        ))),
    }
}

/// Store credentials locally (called after verify).
pub fn store_credentials(req: StoreCredentialsRequest) -> Result<serde_json::Value, HandlerError> {
    let creds = keychain::ExememCredentials {
        user_hash: req.user_hash,
        session_token: req.session_token,
        api_key: req.api_key,
    };
    keychain::store_credentials(&creds)
        .map_err(|e| HandlerError::Internal(format!("Failed to store credentials: {}", e)))?;
    Ok(serde_json::json!({"ok": true}))
}

/// Delete credentials from local storage (logout).
pub fn delete_credentials() -> Result<serde_json::Value, HandlerError> {
    keychain::delete_credentials()
        .map_err(|e| HandlerError::Internal(format!("Failed to delete credentials: {}", e)))?;
    Ok(serde_json::json!({"ok": true}))
}

/// Return the Exemem config (API URL) for the frontend.
pub fn get_exemem_config() -> serde_json::Value {
    serde_json::json!({
        "ok": true,
        "api_url": exemem_api_url(),
    })
}

// ============================================================================
// Signed registration with Exemem
// ============================================================================

/// Register this node's public key with Exemem using the NodeManager's
/// identity. Idempotent: returns fresh session token + new API key for
/// existing users. Also writes user-hash / api-url into the Sled node config
/// on success.
///
/// Returns the raw JSON response from the Exemem CLI register endpoint with
/// `api_url` added in.
pub async fn register_with_exemem(
    node_manager: &Arc<NodeManager>,
    invite_code: Option<&str>,
) -> Result<serde_json::Value, String> {
    let mut response = signed_register(node_manager, invite_code).await?;
    if let Some(obj) = response.as_object_mut() {
        obj.insert(
            "api_url".to_string(),
            serde_json::Value::String(exemem_api_url()),
        );
    }
    Ok(response)
}

/// Refresh the session token by calling the signed register endpoint.
/// The register endpoint is idempotent: for existing users with a valid
/// signature, it returns a fresh session token + new API key.
pub async fn refresh_session_token(node_manager: &Arc<NodeManager>) -> Result<String, String> {
    let json = signed_register(node_manager, None).await?;
    json.get("session_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "Missing session_token in register response".to_string())
}

/// Core signed register logic shared by `register_with_exemem` and
/// `refresh_session_token`.
///
/// Signs "{public_key_hex}:{timestamp}" with the node's Ed25519 private key,
/// sends to the Exemem CLI register endpoint, and stores credentials.
pub(crate) async fn signed_register(
    node_manager: &Arc<NodeManager>,
    invite_code: Option<&str>,
) -> Result<serde_json::Value, String> {
    // Get the node's keys from identity (works even during onboarding before user context)
    let public_key_b64 = node_manager
        .ensure_default_identity()
        .await
        .map_err(|e| format!("Failed to initialize node identity: {e}"))?;

    let private_key_b64 = node_manager
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
        };
        keychain::store_credentials(&creds).map_err(|e| {
            format!("Registration succeeded but failed to persist credentials locally: {e}")
        })?;

        // Write ONLY api_url and user_hash to Sled (safe to sync across devices).
        // Per-device secrets (api_key, session_token) stay in credentials.json only.
        if let Some(pool) = node_manager.get_sled_pool().await {
            if let Ok(store) = NodeConfigStore::new(pool) {
                let cloud_creds = CloudCredentials {
                    api_url: exemem_api_url(),
                    user_hash: Some(user_hash.to_string()),
                };
                if let Err(e) = store.set_cloud_config(&cloud_creds) {
                    log::warn!("Failed to write cloud config to Sled: {}", e);
                }
            }
        }
    }

    Ok(json)
}

// ============================================================================
// Standalone auth refresh (no NodeManager dependency)
// ============================================================================

/// Refresh Exemem credentials for the sync engine.
///
/// See the module docs on `build_auth_refresh_callback` for the two-branch
/// behaviour (use stored key if newer, else re-register).
async fn refresh_auth_standalone(
    last_returned: Arc<Mutex<Option<String>>>,
) -> Result<fold_db::sync::auth::SyncAuth, String> {
    // Step 1: Try the stored credentials first. If we have a newer key than the
    // one we last handed the sync engine, just hand it that new key — no need
    // to burn a fresh one.
    let stored = crate::keychain::load_credentials()
        .map_err(|e| format!("Auth refresh: failed to load credentials: {e}"))?;

    if let Some(ref creds) = stored {
        let mut guard = last_returned
            .lock()
            .map_err(|e| format!("Auth refresh: last_returned mutex poisoned: {e}"))?;
        let already_returned = guard.as_deref() == Some(creds.api_key.as_str());
        if !already_returned {
            log::info!("Sync auth: returning stored api_key from credentials.json");
            *guard = Some(creds.api_key.clone());
            return Ok(fold_db::sync::auth::SyncAuth::ApiKey(creds.api_key.clone()));
        }
        // Stored key is the same one we already returned → it's stale, fall through.
        log::info!("Sync auth: stored api_key is stale, re-registering with Exemem");
    } else {
        log::info!("Sync auth: no stored credentials, re-registering with Exemem");
    }

    // Step 2: Re-register. This rotates the API key on the Exemem side.
    let new_api_key = reregister_and_store().await?;

    let mut guard = last_returned
        .lock()
        .map_err(|e| format!("Auth refresh: last_returned mutex poisoned: {e}"))?;
    *guard = Some(new_api_key.clone());

    log::info!("Sync auth refreshed successfully via re-registration");

    // The sync engine's presigned-URL endpoint authenticates with X-API-Key,
    // not a bearer token, so we return ApiKey even after re-registration.
    Ok(fold_db::sync::auth::SyncAuth::ApiKey(new_api_key))
}

/// Re-register this node with Exemem using the persisted Ed25519 keypair.
///
/// Standalone: does not depend on `NodeManager`. Loads the node identity from
/// disk, signs a register request, calls the Exemem CLI register endpoint,
/// stores the new credentials locally, and returns the new `api_key`.
async fn reregister_and_store() -> Result<String, String> {
    // 1. Load the node's persisted identity (Ed25519 keypair).
    //    Try the NodeManager identity path first (identity/{hash}.json),
    //    fall back to config/node_identity.json for backward compat.
    let folddb_home = crate::utils::paths::folddb_home()
        .map_err(|e| format!("Cannot resolve FOLDDB_HOME: {e}"))?;
    let identity_path = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"default");
        let hash_hex = format!("{:x}", hasher.finalize());
        let hashed_path = folddb_home
            .join("identity")
            .join(format!("{hash_hex}.json"));
        if hashed_path.exists() {
            hashed_path
        } else {
            // Backward compat: config/node_identity.json
            folddb_home.join("config").join("node_identity.json")
        }
    };

    let identity_bytes = crate::sensitive_io::read_sensitive(&identity_path)
        .map_err(|e| format!("Failed to read node identity for auth refresh: {e}"))?;
    let identity_json = String::from_utf8(identity_bytes)
        .map_err(|e| format!("Node identity is not valid UTF-8: {e}"))?;

    #[derive(serde::Deserialize)]
    struct Identity {
        private_key: String,
        public_key: String,
    }
    let identity: Identity = serde_json::from_str(&identity_json)
        .map_err(|e| format!("Failed to parse node identity: {e}"))?;

    // 2. Decode public key from base64 to hex (CLI register expects hex).
    let public_key_hex = base64_to_hex(&identity.public_key)
        .ok_or_else(|| "Failed to decode public key from base64".to_string())?;

    // 3. Sign "{public_key_hex}:{timestamp}".
    let timestamp = chrono::Utc::now().timestamp();
    let payload = format!("{}:{}", public_key_hex, timestamp);
    let signature_b64 = sign_payload(&identity.private_key, &payload)?;

    // 4. POST to Exemem CLI register endpoint.
    let api_url = exemem_api_url();
    let url = format!("{}/api/auth/cli/register", api_url);
    let register_body = serde_json::json!({
        "public_key": public_key_hex,
        "timestamp": timestamp,
        "signature": signature_b64
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&register_body)
        .send()
        .await
        .map_err(|e| format!("Auth refresh: failed to connect to Exemem API: {e}"))?;

    let text = resp
        .text()
        .await
        .map_err(|e| format!("Auth refresh: failed to read response: {e}"))?;

    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|_| format!("Auth refresh: invalid JSON response: {text}"))?;

    if !json.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        let error = json
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown error");
        return Err(format!("Auth refresh: register failed: {error}"));
    }

    // 5. Extract and store new credentials.
    let session_token = json
        .get("session_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Auth refresh: missing session_token in response".to_string())?;
    let api_key = json
        .get("api_key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Auth refresh: missing api_key in response".to_string())?;
    let user_hash = json
        .get("user_hash")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Auth refresh: missing user_hash in response".to_string())?;

    let creds = crate::keychain::ExememCredentials {
        user_hash: user_hash.to_string(),
        session_token: session_token.to_string(),
        api_key: api_key.to_string(),
    };
    crate::keychain::store_credentials(&creds)
        .map_err(|e| format!("Auth refresh: failed to store credentials: {e}"))?;

    // Per-device secrets (api_key, session_token) are stored ONLY in credentials.json.
    // We do NOT write them to Sled (which syncs across devices) or node_config.json.

    Ok(api_key.to_string())
}

/// Build an `AuthRefreshCallback` for the sync engine.
///
/// The returned callback:
/// 1. First checks `credentials.json` for a newer API key than the one it last
///    returned, and hands that to the sync engine if available. No network
///    call — just catches the engine up to a key that a previous startup task
///    or register call already produced.
/// 2. Only if the stored key is also stale does it re-register with the
///    Exemem API (rotating the key) and return the new one.
///
/// The "last returned" API key is tracked in a mutex captured by the closure
/// so repeated 401s eventually force a real re-registration instead of
/// returning the same stale key forever.
pub fn build_auth_refresh_callback() -> fold_db::sync::AuthRefreshCallback {
    let last_returned: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    Arc::new(move || {
        let last_returned = last_returned.clone();
        Box::pin(refresh_auth_standalone(last_returned))
    })
}

/// Sign a payload with the node's Ed25519 private key.
/// Returns the base64-encoded signature.
pub(crate) fn sign_payload(private_key_b64: &str, payload: &str) -> Result<String, String> {
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
pub(crate) fn base64_to_hex(b64: &str) -> Option<String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(b64))
        .ok()?;
    Some(hex::encode(bytes))
}

// ============================================================================
// Recovery phrase (BIP39 mnemonic for device transfer)
// ============================================================================

/// Generate the 24-word BIP39 recovery phrase for the node's Ed25519 private
/// key. Local-only — the key never leaves the device over the network.
pub async fn get_recovery_phrase(
    node_manager: &Arc<NodeManager>,
) -> Result<Vec<String>, HandlerError> {
    let private_key_b64 = node_manager
        .get_base_config()
        .await
        .private_key
        .ok_or_else(|| HandlerError::Internal("Node private key not available".to_string()))?;

    use base64::Engine;
    let seed_bytes = match base64::engine::general_purpose::STANDARD.decode(&private_key_b64) {
        Ok(bytes) if bytes.len() == 32 => bytes,
        _ => {
            return Err(HandlerError::Internal(
                "Invalid private key format".to_string(),
            ));
        }
    };

    let mnemonic = bip39::Mnemonic::from_entropy(&seed_bytes)
        .map_err(|e| HandlerError::Internal(format!("Failed to generate mnemonic: {}", e)))?;

    Ok(mnemonic.words().map(|w| w.to_string()).collect())
}

/// Resolve the path to `$FOLDDB_HOME/config/node_identity.json`.
pub(crate) fn identity_path() -> std::path::PathBuf {
    crate::utils::paths::folddb_home()
        .map(|h| h.join("config").join("node_identity.json"))
        .unwrap_or_else(|_| std::path::PathBuf::from("config/node_identity.json"))
}

// ============================================================================
// Restore from phrase
// ============================================================================

/// Input for `restore_from_phrase`. The HTTP layer extracts this from the
/// request body.
pub struct RestoreFromPhraseInput {
    pub words: String,
}

/// Restore node identity from a 24-word BIP39 recovery phrase.
///
/// Derives the Ed25519 keypair, writes the identity file, updates the in-memory
/// NodeManager config, registers with Exemem, and spawns background bootstrap
/// from cloud. If any step fails, rolls back the on-disk identity and the
/// in-memory config.
pub async fn restore_from_phrase(
    node_manager: &Arc<NodeManager>,
    input: RestoreFromPhraseInput,
) -> Result<serde_json::Value, HandlerError> {
    // Parse mnemonic
    let mnemonic = bip39::Mnemonic::parse(input.words.trim())
        .map_err(|e| HandlerError::BadRequest(format!("Invalid recovery phrase: {}", e)))?;

    let entropy = mnemonic.to_entropy();
    if entropy.len() != 32 {
        return Err(HandlerError::BadRequest(format!(
            "Recovery phrase must encode 32 bytes, got {}",
            entropy.len()
        )));
    }

    // Derive Ed25519 keypair from seed
    let key_pair = fold_db::security::Ed25519KeyPair::from_secret_key(&entropy)
        .map_err(|e| HandlerError::Internal(format!("Failed to derive keypair: {}", e)))?;

    use base64::Engine;
    let private_key_b64 = base64::engine::general_purpose::STANDARD.encode(&entropy);
    let public_key_b64 =
        base64::engine::general_purpose::STANDARD.encode(key_pair.public_key_bytes());

    // Snapshot pre-restore config so we can roll back the in-memory state
    // if register fails.
    let pre_restore_config = node_manager.get_base_config().await;
    let id_path = identity_path();

    // Write identity to disk ($FOLDDB_HOME/config/node_identity.json)
    let identity_json = serde_json::json!({
        "private_key": private_key_b64,
        "public_key": public_key_b64,
    });

    crate::sensitive_io::write_sensitive(
        &id_path,
        serde_json::to_string_pretty(&identity_json)
            .unwrap()
            .as_bytes(),
    )
    .map_err(|e| HandlerError::Internal(format!("Failed to write identity: {}", e)))?;

    match finalize_restore(
        node_manager,
        &public_key_b64,
        &private_key_b64,
        pre_restore_config.clone(),
    )
    .await
    {
        Ok(response) => Ok(response),
        Err(e) => {
            // Rollback: remove the freshly-written identity file and restore
            // the pre-restore in-memory config so the node doesn't boot with
            // a half-restored identity on next restart.
            log::error!(
                "restore_from_phrase failed, rolling back identity file: {}",
                e
            );
            if let Err(rm_err) = std::fs::remove_file(&id_path) {
                if rm_err.kind() != std::io::ErrorKind::NotFound {
                    log::error!(
                        "restore_from_phrase rollback: failed to delete {:?}: {}",
                        id_path,
                        rm_err
                    );
                }
            }
            node_manager
                .update_config(crate::server::node_manager::NodeManagerConfig {
                    base_config: pre_restore_config,
                })
                .await;
            Err(HandlerError::Internal(e))
        }
    }
}

/// Finalize a restore once the identity file has been written. Updates the
/// NodeManager config with the restored keypair, calls signed_register, and
/// spawns the background bootstrap.
async fn finalize_restore(
    node_manager: &Arc<NodeManager>,
    public_key_b64: &str,
    private_key_b64: &str,
    pre_restore_config: crate::fold_node::config::NodeConfig,
) -> Result<serde_json::Value, String> {
    // Ensure the shared local node exists (opens Sled) before we grab the handle.
    let _ = node_manager.ensure_default_identity().await;

    // Grab the SledPool BEFORE update_config() invalidates all nodes.
    let sled_pool = node_manager.get_sled_pool().await;

    // Update the in-memory config so signed_register uses the restored key.
    let mut base_config = pre_restore_config;
    base_config.public_key = Some(public_key_b64.to_string());
    base_config.private_key = Some(private_key_b64.to_string());
    node_manager
        .update_config(crate::server::node_manager::NodeManagerConfig { base_config })
        .await;

    // Register with Exemem (idempotent — returns fresh token for existing users).
    // Failure here triggers rollback in the caller.
    let mut response = signed_register(node_manager, None).await?;
    if let Some(obj) = response.as_object_mut() {
        obj.insert(
            "api_url".to_string(),
            serde_json::Value::String(exemem_api_url()),
        );
    }

    // Spawn background bootstrap if we got Exemem credentials and a Sled handle.
    if let (Some(api_key), Some(_user_hash), Some(sled_pool)) = (
        response.get("api_key").and_then(|v| v.as_str()),
        response.get("user_hash").and_then(|v| v.as_str()),
        sled_pool,
    ) {
        let api_url = exemem_api_url();
        let api_key = api_key.to_string();
        let node_manager = node_manager.clone();

        tokio::spawn(async move {
            if let Err(e) = bootstrap_from_cloud(&api_url, &api_key, &node_manager, sled_pool).await
            {
                log::error!("Background bootstrap after restore failed: {}", e);
            }
        });
    } else {
        log::warn!(
            "Bootstrap after restore skipped: missing api_key, user_hash, or sled_pool handle"
        );
    }

    Ok(response)
}

// ============================================================================
// Bootstrap status tracking
// ============================================================================

/// State of the most recent / in-flight database bootstrap after identity restore.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BootstrapStatusState {
    InProgress,
    Complete,
    Failed,
}

/// Which phase of the two-phase bootstrap the node is currently executing.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BootstrapPhase {
    Personal,
    Orgs,
}

/// Persisted bootstrap status, written to
/// `$FOLDDB_HOME/data/.bootstrap_status.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapStatus {
    pub status: BootstrapStatusState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<BootstrapPhase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub targets_done: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub targets_total: Option<usize>,
}

impl BootstrapStatus {
    pub fn in_progress() -> Self {
        Self {
            status: BootstrapStatusState::InProgress,
            error: None,
            phase: None,
            targets_done: None,
            targets_total: None,
        }
    }
    /// Phase-aware in-progress status.
    pub fn in_progress_phase(phase: BootstrapPhase, done: usize, total: usize) -> Self {
        Self {
            status: BootstrapStatusState::InProgress,
            error: None,
            phase: Some(phase),
            targets_done: Some(done),
            targets_total: Some(total),
        }
    }
    pub fn complete() -> Self {
        Self {
            status: BootstrapStatusState::Complete,
            error: None,
            phase: None,
            targets_done: None,
            targets_total: None,
        }
    }
    pub fn failed(error: String) -> Self {
        Self {
            status: BootstrapStatusState::Failed,
            error: Some(error),
            phase: None,
            targets_done: None,
            targets_total: None,
        }
    }
}

/// Path to the bootstrap status file.
pub(crate) fn bootstrap_status_path() -> Option<std::path::PathBuf> {
    crate::utils::paths::folddb_home()
        .ok()
        .map(|h| h.join("data").join(".bootstrap_status.json"))
}

/// Write the bootstrap status to disk. Errors are logged loudly (no silent
/// failures) but not returned — callers are inside tokio::spawn and have no
/// way to propagate an error to the original HTTP client.
pub(crate) fn write_bootstrap_status(status: &BootstrapStatus) {
    let path = match bootstrap_status_path() {
        Some(p) => p,
        None => {
            log::error!("write_bootstrap_status: cannot resolve FOLDDB_HOME");
            return;
        }
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::error!(
                "write_bootstrap_status: failed to create {:?}: {}",
                parent,
                e
            );
            return;
        }
    }
    let json = match serde_json::to_string_pretty(status) {
        Ok(s) => s,
        Err(e) => {
            log::error!("write_bootstrap_status: serialize failed: {}", e);
            return;
        }
    };
    if let Err(e) = std::fs::write(&path, json) {
        log::error!("write_bootstrap_status: write {:?} failed: {}", path, e);
    }
}

/// Read the bootstrap status from disk, if the file exists.
pub(crate) fn read_bootstrap_status() -> Option<BootstrapStatus> {
    let path = bootstrap_status_path()?;
    let contents = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Remove the bootstrap status file. Used by tests to reset state between
/// cases.
#[cfg(test)]
pub(crate) fn clear_bootstrap_status() {
    if let Some(path) = bootstrap_status_path() {
        if let Err(e) = std::fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::error!("clear_bootstrap_status: failed to remove {:?}: {}", path, e);
            }
        }
    }
}

/// Compute the current restore bootstrap status, suitable for the
/// `GET /api/auth/restore/status` endpoint. When neither the
/// `.bootstrap_pending` marker nor the `.bootstrap_status.json` file exists,
/// the node is idle and this reports `complete`.
pub fn restore_status() -> BootstrapStatus {
    if let Some(status) = read_bootstrap_status() {
        return status;
    }
    // No status file. If the pending marker is absent too, treat as complete.
    if bootstrap_marker_path().map(|p| p.exists()).unwrap_or(false) {
        return BootstrapStatus::in_progress();
    }
    BootstrapStatus::complete()
}

/// Path to the bootstrap pending marker file.
fn bootstrap_marker_path() -> Option<std::path::PathBuf> {
    crate::utils::paths::folddb_home()
        .ok()
        .map(|h| h.join("data").join(".bootstrap_pending"))
}

/// Write a marker so bootstrap resumes if the app is restarted mid-download.
///
/// Returns an error if the marker file cannot be created — this must NOT
/// fail silently, because a missing marker means the daemon will start with
/// an empty local database and never download the user's data.
pub fn write_bootstrap_marker(api_url: &str, api_key: &str) -> Result<(), String> {
    let path = bootstrap_marker_path()
        .ok_or_else(|| "write_bootstrap_marker: unable to resolve folddb home".to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!(
                "write_bootstrap_marker: failed to create {:?}: {}",
                parent, e
            )
        })?;
    }
    let marker = serde_json::json!({
        "api_url": api_url,
        "api_key": api_key,
    });
    let serialized = serde_json::to_string_pretty(&marker)
        .map_err(|e| format!("write_bootstrap_marker: serialize failed: {}", e))?;
    std::fs::write(&path, serialized)
        .map_err(|e| format!("write_bootstrap_marker: write {:?} failed: {}", path, e))?;
    log::info!("Wrote bootstrap marker at {:?}", path);
    Ok(())
}

/// Remove the marker after successful bootstrap.
fn clear_bootstrap_marker() {
    if let Some(path) = bootstrap_marker_path() {
        let _ = std::fs::remove_file(&path);
        log::info!("Cleared bootstrap marker");
    }
}

/// Check if a bootstrap was interrupted and needs resuming.
/// Returns (api_url, api_key) if a marker exists.
pub fn check_bootstrap_pending() -> Option<(String, String)> {
    let path = bootstrap_marker_path()?;
    let contents = std::fs::read_to_string(&path).ok()?;
    let marker: serde_json::Value = serde_json::from_str(&contents).ok()?;
    let api_url = marker["api_url"].as_str()?.to_string();
    let api_key = marker["api_key"].as_str()?.to_string();
    Some((api_url, api_key))
}

/// Resume a bootstrap from cloud that was interrupted by a previous shutdown.
pub async fn resume_bootstrap(
    api_url: &str,
    api_key: &str,
    node_manager: &Arc<NodeManager>,
    sled_pool: Arc<fold_db::storage::SledPool>,
) -> Result<(), String> {
    bootstrap_from_cloud(api_url, api_key, node_manager, sled_pool).await
}

async fn bootstrap_from_cloud(
    api_url: &str,
    api_key: &str,
    node_manager: &Arc<NodeManager>,
    sled_pool: Arc<fold_db::storage::SledPool>,
) -> Result<(), String> {
    log::info!("Starting database bootstrap from cloud after identity restore");
    write_bootstrap_marker(api_url, api_key)?;
    write_bootstrap_status(&BootstrapStatus::in_progress());

    // Helper so every early-return path records the failure to the status file.
    let run = async { bootstrap_from_cloud_inner(api_url, api_key, node_manager, sled_pool).await };
    match run.await {
        Ok(()) => {
            clear_bootstrap_marker();
            write_bootstrap_status(&BootstrapStatus::complete());
            Ok(())
        }
        Err(e) => {
            write_bootstrap_status(&BootstrapStatus::failed(e.clone()));
            Err(e)
        }
    }
}

async fn bootstrap_from_cloud_inner(
    api_url: &str,
    api_key: &str,
    node_manager: &Arc<NodeManager>,
    sled_pool: Arc<fold_db::storage::SledPool>,
) -> Result<(), String> {
    // Derive E2E encryption keys from the restored identity (unified identity:
    // one Ed25519 key for everything, no separate e2e.key).
    let config = node_manager.get_base_config().await;
    let priv_key = config
        .private_key
        .as_ref()
        .ok_or_else(|| "bootstrap_from_cloud: node private key not set".to_string())?;
    let seed = crate::fold_node::FoldNode::extract_ed25519_seed(priv_key)
        .map_err(|e| format!("Failed to extract seed: {e}"))?;
    let e2e_keys = fold_db::crypto::E2eKeys::from_ed25519_seed(&seed)
        .map_err(|e| format!("Failed to derive E2E keys: {e}"))?;
    let data_dir = config.get_storage_path();
    let data_dir_str = data_dir
        .to_str()
        .ok_or_else(|| "Invalid storage path".to_string())?;

    // Build sync components
    let sync_setup = fold_db::sync::SyncSetup::from_exemem(api_url, api_key, data_dir_str);
    let sync_crypto: Arc<dyn fold_db::crypto::CryptoProvider> = Arc::new(
        fold_db::crypto::LocalCryptoProvider::from_key(e2e_keys.encryption_key()),
    );

    let http = Arc::new(reqwest::Client::new());
    let s3 = fold_db::sync::s3::S3Client::new(http.clone());
    let auth = fold_db::sync::auth::AuthClient::new(http, sync_setup.auth_url, sync_setup.auth);

    // Use the pre-cloned SledPool directly — no sled::open() needed.
    let pool_for_orgs = Arc::clone(&sled_pool);
    let base_store: Arc<dyn fold_db::storage::traits::NamespacedStore> =
        Arc::new(fold_db::storage::SledNamespacedStore::new(sled_pool));

    let engine = Arc::new(fold_db::sync::SyncEngine::new(
        sync_setup.device_id,
        sync_crypto,
        s3,
        auth,
        base_store,
        fold_db::sync::SyncConfig::default(),
    ));

    // --- Phase 1: personal bootstrap ---------------------------------------
    write_bootstrap_status(&BootstrapStatus::in_progress_phase(
        BootstrapPhase::Personal,
        0,
        1,
    ));
    let personal_outcome = engine
        .bootstrap_target(0)
        .await
        .map_err(|e| format!("Bootstrap failed (phase 1: personal): {e}"))?;
    write_bootstrap_status(&BootstrapStatus::in_progress_phase(
        BootstrapPhase::Personal,
        1,
        1,
    ));

    log::info!(
        "Bootstrap phase 1 (personal) complete: last_seq={}, entries_replayed={}",
        personal_outcome.last_seq,
        personal_outcome.entries_replayed
    );

    // --- Phase 1.5: configure org sync targets from the replayed Sled ------
    match crate::fold_node::node::build_org_sync_config_from_sled(&pool_for_orgs)
        .map_err(|e| format!("Bootstrap failed (phase 1.5: load org memberships): {e}"))?
    {
        Some(org_config) => {
            let org_count = org_config.membership_count;
            log::info!(
                "Bootstrap phase 1.5: configuring {} org sync target(s)",
                org_count
            );
            engine
                .configure_org_sync(org_config.partitioner, org_config.targets)
                .await;

            write_bootstrap_status(&BootstrapStatus::in_progress_phase(
                BootstrapPhase::Orgs,
                0,
                org_count,
            ));

            // --- Phase 2: bootstrap all targets (personal + orgs) ----------
            let outcomes = engine
                .bootstrap_all()
                .await
                .map_err(|e| format!("Bootstrap failed (phase 2: org targets): {e}"))?;

            log::info!(
                "Bootstrap phase 2 complete: {} target(s) replayed",
                outcomes.len()
            );
        }
        None => {
            log::info!("Bootstrap phase 1.5: no org memberships, skipping org sync configuration");
        }
    }

    log::info!("Database bootstrap complete after restore");
    Ok(())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes tests that mutate `FOLDDB_HOME` (a process-global env var) so
    /// they don't race each other.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::OnceLock;
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("env lock poisoned")
    }

    /// Set FOLDDB_HOME to a temp dir, write credentials.json containing
    /// `api_key`, and return the temp dir guard.
    fn setup_creds_in_temp_home(api_key: &str) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::env::set_var("FOLDDB_HOME", tmp.path());

        let creds = crate::keychain::ExememCredentials {
            user_hash: "test-user".to_string(),
            session_token: "test-session".to_string(),
            api_key: api_key.to_string(),
        };
        crate::keychain::store_credentials(&creds).expect("store_credentials");
        tmp
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn refresh_auth_returns_stored_api_key_without_network() {
        let _guard = env_lock();
        let _tmp = setup_creds_in_temp_home("api_key_v2");

        let last_returned: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let result = refresh_auth_standalone(last_returned.clone()).await;

        let auth = result.expect("should return stored api_key without hitting network");
        match auth {
            fold_db::sync::auth::SyncAuth::ApiKey(k) => assert_eq!(k, "api_key_v2"),
            fold_db::sync::auth::SyncAuth::BearerToken(_) => {
                panic!("must return ApiKey variant, never BearerToken")
            }
        }
        assert_eq!(
            last_returned.lock().unwrap().as_deref(),
            Some("api_key_v2"),
            "last_returned should be updated to the key we returned"
        );

        std::env::remove_var("FOLDDB_HOME");
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn refresh_auth_returns_newer_stored_key_on_second_call() {
        let _guard = env_lock();
        let tmp = setup_creds_in_temp_home("api_key_v1");

        let last_returned: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let first = refresh_auth_standalone(last_returned.clone())
            .await
            .expect("first call");
        assert!(matches!(
            first,
            fold_db::sync::auth::SyncAuth::ApiKey(ref k) if k == "api_key_v1"
        ));

        // Rotate the stored credential.
        let creds = crate::keychain::ExememCredentials {
            user_hash: "test-user".to_string(),
            session_token: "test-session".to_string(),
            api_key: "api_key_v2".to_string(),
        };
        crate::keychain::store_credentials(&creds).expect("store rotated creds");

        let second = refresh_auth_standalone(last_returned.clone())
            .await
            .expect("second call should see newer stored key");
        match second {
            fold_db::sync::auth::SyncAuth::ApiKey(k) => assert_eq!(k, "api_key_v2"),
            fold_db::sync::auth::SyncAuth::BearerToken(_) => {
                panic!("must return ApiKey variant")
            }
        }
        assert_eq!(last_returned.lock().unwrap().as_deref(), Some("api_key_v2"));

        drop(tmp);
        std::env::remove_var("FOLDDB_HOME");
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn refresh_auth_attempts_reregister_when_stored_key_is_stale() {
        let _guard = env_lock();
        let _tmp = setup_creds_in_temp_home("api_key_stale");

        // Point Exemem API at a non-routable address so reregister fails fast.
        std::env::set_var("EXEMEM_API_URL", "http://127.0.0.1:1");

        let last_returned: Arc<Mutex<Option<String>>> =
            Arc::new(Mutex::new(Some("api_key_stale".to_string())));

        let result = refresh_auth_standalone(last_returned).await;
        let err = result.expect_err("should attempt reregister and fail at HTTP");
        let reached_http = err.contains("failed to connect")
            || err.contains("Failed to read node identity")
            || err.contains("Cannot resolve FOLDDB_HOME");
        assert!(
            reached_http,
            "expected error from reregister path, got: {err}"
        );

        std::env::remove_var("EXEMEM_API_URL");
        std::env::remove_var("FOLDDB_HOME");
    }

    // ------------------------------------------------------------------
    // Bootstrap status file round-trip (G4)
    // ------------------------------------------------------------------

    fn setup_empty_home() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::env::set_var("FOLDDB_HOME", tmp.path());
        std::fs::create_dir_all(tmp.path().join("data")).expect("create data dir");
        tmp
    }

    #[test]
    fn bootstrap_status_write_and_read_round_trip() {
        let _guard = env_lock();
        let _tmp = setup_empty_home();

        write_bootstrap_status(&BootstrapStatus::in_progress());
        let got = read_bootstrap_status().expect("status should exist after write");
        assert_eq!(got.status, BootstrapStatusState::InProgress);
        assert!(got.error.is_none());

        write_bootstrap_status(&BootstrapStatus::failed("boom".to_string()));
        let got = read_bootstrap_status().expect("status should exist");
        assert_eq!(got.status, BootstrapStatusState::Failed);
        assert_eq!(got.error.as_deref(), Some("boom"));

        write_bootstrap_status(&BootstrapStatus::complete());
        let got = read_bootstrap_status().expect("status should exist");
        assert_eq!(got.status, BootstrapStatusState::Complete);
        assert!(got.error.is_none());

        clear_bootstrap_status();
        assert!(
            read_bootstrap_status().is_none(),
            "status file should be gone after clear"
        );

        std::env::remove_var("FOLDDB_HOME");
    }

    #[test]
    fn bootstrap_status_phase_round_trip() {
        let _guard = env_lock();
        let _tmp = setup_empty_home();

        write_bootstrap_status(&BootstrapStatus::in_progress_phase(
            BootstrapPhase::Personal,
            0,
            1,
        ));
        let got = read_bootstrap_status().expect("status should exist");
        assert_eq!(got.status, BootstrapStatusState::InProgress);
        assert_eq!(got.phase, Some(BootstrapPhase::Personal));
        assert_eq!(got.targets_done, Some(0));
        assert_eq!(got.targets_total, Some(1));

        write_bootstrap_status(&BootstrapStatus::in_progress_phase(
            BootstrapPhase::Orgs,
            0,
            5,
        ));
        let got = read_bootstrap_status().expect("status should exist");
        assert_eq!(got.phase, Some(BootstrapPhase::Orgs));
        assert_eq!(got.targets_done, Some(0));
        assert_eq!(got.targets_total, Some(5));

        write_bootstrap_status(&BootstrapStatus::complete());
        let got = read_bootstrap_status().expect("status should exist");
        assert_eq!(got.status, BootstrapStatusState::Complete);
        assert!(got.phase.is_none());
        assert!(got.targets_done.is_none());
        assert!(got.targets_total.is_none());

        clear_bootstrap_status();
        std::env::remove_var("FOLDDB_HOME");
    }

    #[test]
    fn bootstrap_status_old_shape_backward_compatible() {
        let _guard = env_lock();
        let _tmp = setup_empty_home();

        let path = bootstrap_status_path().expect("status path");
        std::fs::write(&path, r#"{"status":"in_progress"}"#).unwrap();
        let got = read_bootstrap_status().expect("old shape must parse");
        assert_eq!(got.status, BootstrapStatusState::InProgress);
        assert!(got.error.is_none());
        assert!(got.phase.is_none());
        assert!(got.targets_done.is_none());
        assert!(got.targets_total.is_none());

        std::fs::write(&path, r#"{"status":"failed","error":"boom"}"#).unwrap();
        let got = read_bootstrap_status().expect("old failed shape must parse");
        assert_eq!(got.status, BootstrapStatusState::Failed);
        assert_eq!(got.error.as_deref(), Some("boom"));
        assert!(got.phase.is_none());

        clear_bootstrap_status();
        std::env::remove_var("FOLDDB_HOME");
    }

    #[test]
    fn bootstrap_phase_serializes_snake_case() {
        let s = serde_json::to_value(BootstrapPhase::Personal).unwrap();
        assert_eq!(s, serde_json::json!("personal"));
        let s = serde_json::to_value(BootstrapPhase::Orgs).unwrap();
        assert_eq!(s, serde_json::json!("orgs"));
    }

    #[test]
    fn bootstrap_status_in_progress_phase_serializes_expected_keys() {
        let status = BootstrapStatus::in_progress_phase(BootstrapPhase::Orgs, 2, 5);
        let v = serde_json::to_value(&status).unwrap();
        assert_eq!(v["status"], "in_progress");
        assert_eq!(v["phase"], "orgs");
        assert_eq!(v["targets_done"], 2);
        assert_eq!(v["targets_total"], 5);
        assert!(v.get("error").is_none(), "error omitted when None");
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn restore_status_reports_each_state() {
        let _guard = env_lock();
        let _tmp = setup_empty_home();

        // Idle (no files): reports complete.
        let status = restore_status();
        assert_eq!(status.status, BootstrapStatusState::Complete);

        // Pending marker present, no status file: reports in_progress.
        let marker = bootstrap_marker_path().expect("marker path");
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(&marker, "{}").unwrap();
        let status = restore_status();
        assert_eq!(status.status, BootstrapStatusState::InProgress);
        std::fs::remove_file(&marker).unwrap();

        write_bootstrap_status(&BootstrapStatus::in_progress());
        assert_eq!(restore_status().status, BootstrapStatusState::InProgress);

        write_bootstrap_status(&BootstrapStatus::failed("network down".to_string()));
        let s = restore_status();
        assert_eq!(s.status, BootstrapStatusState::Failed);
        assert_eq!(s.error.as_deref(), Some("network down"));

        write_bootstrap_status(&BootstrapStatus::complete());
        assert_eq!(restore_status().status, BootstrapStatusState::Complete);

        clear_bootstrap_status();
        std::env::remove_var("FOLDDB_HOME");
    }

    #[test]
    fn bootstrap_marker_roundtrip() {
        let _guard = env_lock();
        let tmp = tempfile::tempdir().expect("tempdir");
        std::env::set_var("FOLDDB_HOME", tmp.path());

        assert!(
            check_bootstrap_pending().is_none(),
            "expected no pending marker in fresh temp home"
        );

        write_bootstrap_marker("https://example.test", "test-api-key")
            .expect("write_bootstrap_marker should succeed");

        let pending = check_bootstrap_pending().expect("marker should be readable");
        assert_eq!(pending.0, "https://example.test");
        assert_eq!(pending.1, "test-api-key");

        let marker_path = tmp.path().join("data").join(".bootstrap_pending");
        assert!(
            marker_path.exists(),
            "marker file missing at {:?}",
            marker_path
        );

        std::env::remove_var("FOLDDB_HOME");
    }
}
