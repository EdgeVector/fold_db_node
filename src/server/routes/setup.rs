//! `POST /api/setup/bootstrap` — the canonical first-launch path.
//!
//! Fresh installs and recovery-phrase restores both flow through this one
//! handler. It mints (or derives) a node identity, persists it under the
//! keychain master key, optionally registers with Exemem, writes the
//! `node_config.json`, and finally drops the `.onboarding_complete`
//! marker that gates the whole flow.
//!
//! Auth model — single-user Tauri (see CLAUDE.md "Trust boundary: loopback
//! owner context"): the handler self-disables once the marker file exists.
//! No tokens, no signed payloads. The whole defense rests on `folddb_server`
//! binding 127.0.0.1 only — any future shared/multi-user distribution must
//! gate this on a verified caller identity.
//!
//! TODO: when shared/multi-user lands, gate on signed token here.

use std::path::PathBuf;
use std::sync::Arc;

use actix_web::{web, HttpResponse, Responder};
use fold_db::storage::{CloudSyncConfig, DatabaseConfig};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::fold_node::config::save_node_config;
use crate::handlers::setup::{
    derive_recovery_phrase, identity_from_phrase, register_with_exemem_and_invite,
};
use crate::identity;
use crate::server::http_server::AppState;
use crate::server::startup::ConfigDir;
use crate::trust::identity_card::IdentityCard;
use crate::utils::crypto::user_hash_from_pubkey;

/// Body of `POST /api/setup/bootstrap`. All fields except `name` are optional.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct BootstrapRequest {
    /// Display name for the identity card. Required.
    pub name: String,
    /// Optional contact hint stored on the identity card.
    #[serde(default)]
    pub email: Option<String>,
    /// Optional birthday (`MM-DD`).
    #[serde(default)]
    pub birthday: Option<String>,
    /// `"anthropic"`, `"ollama"`, or omitted to skip AI ingestion config.
    #[serde(default)]
    pub ai_provider: Option<String>,
    /// Anthropic API key — saved to the encrypted key store when
    /// `ai_provider == "anthropic"`.
    #[serde(default)]
    pub anthropic_api_key: Option<String>,
    /// Ollama base URL when `ai_provider == "ollama"`.
    #[serde(default)]
    pub ollama_url: Option<String>,
    /// Ollama model name when `ai_provider == "ollama"`.
    #[serde(default)]
    pub ollama_model: Option<String>,
    /// Register with Exemem cloud backup.
    #[serde(default)]
    pub enable_cloud: bool,
    /// Exemem invite code; required when `enable_cloud == true`.
    #[serde(default)]
    pub invite_code: Option<String>,
    /// 24-word BIP39 recovery phrase. When set, the handler derives the
    /// identity from the phrase instead of minting a fresh one.
    #[serde(default)]
    pub recovery_phrase: Option<String>,
}

/// Successful response from `POST /api/setup/bootstrap`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BootstrapResponse {
    /// Base64-encoded Ed25519 public key.
    pub public_key: String,
    /// SHA-256 of the public key, hex-encoded — the canonical `user_hash`.
    pub user_hash: String,
    /// 24-word recovery phrase. `None` when the request supplied a
    /// `recovery_phrase` (restore path); `Some` on fresh-mint so the UI
    /// can prompt the user to write it down.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_phrase: Option<Vec<String>>,
    /// Set when cloud registration succeeded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cloud: Option<CloudInfo>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CloudInfo {
    /// Always `true` if `cloud` is present in the response.
    pub enabled: bool,
    /// User hash returned by the Exemem registration.
    pub exemem_user_hash: String,
}

/// `POST /api/setup/bootstrap`. See module docs for the auth model.
#[utoipa::path(
    post,
    path = "/api/setup/bootstrap",
    tag = "system",
    request_body = BootstrapRequest,
    responses(
        (status = 200, description = "Bootstrap succeeded", body = BootstrapResponse),
        (status = 400, description = "Invalid request body"),
        (status = 409, description = "Cloud registration conflict"),
        (status = 410, description = "Onboarding already completed; endpoint disabled"),
        (status = 500, description = "Server error"),
    )
)]
pub async fn bootstrap(
    state: web::Data<AppState>,
    config_dir: web::Data<ConfigDir>,
    req: web::Json<BootstrapRequest>,
) -> impl Responder {
    let marker_path = match marker_path() {
        Ok(p) => p,
        Err(e) => {
            return HttpResponse::InternalServerError().json(json!({
                "ok": false,
                "error": format!("Cannot resolve FOLDDB_HOME: {e}")
            }));
        }
    };

    // D1: self-disable once the marker exists. The check happens at the
    // very top of the handler so subsequent re-runs of /bootstrap (CSRF,
    // accidental re-POST) cannot rotate the identity.
    if marker_path.exists() {
        return HttpResponse::Gone().json(json!({
            "ok": false,
            "error": "onboarding_already_complete",
            "message": "This node has already been bootstrapped. POST /api/auth/restore to restore from a recovery phrase."
        }));
    }

    if req.enable_cloud && req.invite_code.as_deref().unwrap_or("").is_empty() {
        return HttpResponse::BadRequest().json(json!({
            "ok": false,
            "error": "invite_code_required",
            "message": "enable_cloud=true requires a non-empty invite_code"
        }));
    }

    if let Some(birthday) = req.birthday.as_deref() {
        if let Err(msg) = IdentityCard::validate_birthday(birthday) {
            return HttpResponse::BadRequest().json(json!({
                "ok": false,
                "error": "invalid_birthday",
                "message": msg,
            }));
        }
    }

    let req = req.into_inner();
    match run_bootstrap(state.get_ref(), config_dir.get_ref(), &marker_path, req).await {
        Ok(resp) => HttpResponse::Ok().json(resp),
        Err(BootstrapError::Conflict(msg)) => HttpResponse::Conflict().json(json!({
            "ok": false,
            "error": "cloud_conflict",
            "message": msg,
        })),
        Err(BootstrapError::Internal(msg)) => HttpResponse::InternalServerError().json(json!({
            "ok": false,
            "error": "bootstrap_failed",
            "message": msg,
        })),
    }
}

#[derive(Debug)]
enum BootstrapError {
    /// Recovery phrase + invite code disagreed with Exemem's existing
    /// registration for that key. Surfaced as HTTP 409.
    Conflict(String),
    /// Anything else — registration timeout, disk error, etc.
    Internal(String),
}

impl From<String> for BootstrapError {
    fn from(s: String) -> Self {
        Self::Internal(s)
    }
}

/// Perform the full bootstrap. Pulled out of the route so the rollback
/// path runs whatever we partially persisted, regardless of which step
/// failed.
async fn run_bootstrap(
    state: &AppState,
    config_dir: &ConfigDir,
    marker_path: &std::path::Path,
    req: BootstrapRequest,
) -> Result<BootstrapResponse, BootstrapError> {
    // ---- (2-3) Identity: derive from phrase or generate fresh, then
    //            persist ENC:-prefixed under the keychain master key when
    //            `os-keychain` is on. Both branches MUST mint the master
    //            key before the first identity write — without that, the
    //            initial blob is plaintext and only re-encrypted on the
    //            next daemon restart, which in the Tauri model is hours
    //            away (the daemon lives the user's whole session).
    //            From here on, any failure must clear the identity tree.
    let is_fresh_mint = req.recovery_phrase.is_none();
    let pool = state.node_manager.get_or_init_sled_pool().await;
    let id = match req.recovery_phrase.as_deref() {
        Some(words) => {
            // Restore path: derive deterministically from the phrase, then
            // mint the keychain master key (idempotent — returns the
            // existing key if one is already present) before saving so the
            // very first on-disk write is `ENC:`-prefixed.
            let id = identity_from_phrase(words).map_err(BootstrapError::Internal)?;
            #[cfg(feature = "os-keychain")]
            crate::secure_store::initialize_master_key().map_err(|e| {
                BootstrapError::Internal(format!("Failed to initialize master key: {e}"))
            })?;
            identity::save(Arc::clone(&pool), &id).map_err(|e| {
                BootstrapError::Internal(format!("Failed to persist identity: {e}"))
            })?;
            id
        }
        None => {
            // Fresh-mint path: `provision` is the bootstrap-only mint+save
            // that calls `secure_store::initialize_master_key` before
            // writing, so the first persisted blob is encrypted from the
            // start. Precondition: the 410-on-marker check at the top of
            // the route guarantees no prior bootstrap completed; the marker
            // is written after every persisted artifact, so an empty
            // identity tree is the expected state here.
            debug_assert!(
                identity::peek_raw_identity_value(&pool)
                    .ok()
                    .flatten()
                    .is_none(),
                "bootstrap precondition: missing .onboarding_complete marker implies empty identity tree"
            );
            identity::provision(Arc::clone(&pool)).map_err(|e| {
                BootstrapError::Internal(format!("Failed to provision identity: {e}"))
            })?
        }
    };

    // Helper closure for rollback. Captures everything the route persisted
    // so the route can call it from any failure point.
    let pool_for_rollback = Arc::clone(&pool);
    let marker_path_for_rollback = marker_path.to_path_buf();
    let config_dir_for_rollback = config_dir.as_path().to_path_buf();
    let rollback = move || {
        // Identity tree — primary persisted artifact.
        if let Ok(store) = identity::open(Arc::clone(&pool_for_rollback)) {
            let _ = store.clear();
        }
        // Marker — only present if the very last step succeeded; harmless
        // to attempt removal otherwise.
        let _ = std::fs::remove_file(&marker_path_for_rollback);
        // Credentials.
        let _ = crate::keychain::delete_credentials();
        // Anthropic key.
        let _ = crate::ingestion::anthropic_key_store::delete(&config_dir_for_rollback);
        // Per-server config files.
        let _ = std::fs::remove_file(config_dir_for_rollback.join("ingestion_config.json"));
        let _ = std::fs::remove_file(config_dir_for_rollback.join("node_config.json"));
    };

    // ---- (4-onwards): wrap in async block so we can roll back on any error. ----
    let result = run_bootstrap_post_identity(state, config_dir, marker_path, &req, &id).await;
    let (cloud_info, recovery_phrase_for_response) = match result {
        Ok(v) => v,
        Err(e) => {
            rollback();
            return Err(e);
        }
    };

    // ---- (9) Build the response. ----
    let user_hash = user_hash_from_pubkey(&id.public_key);
    let recovery_phrase = if is_fresh_mint {
        Some(recovery_phrase_for_response)
    } else {
        None
    };

    Ok(BootstrapResponse {
        public_key: id.public_key.clone(),
        user_hash,
        recovery_phrase,
        cloud: cloud_info,
    })
}

/// Steps 4–8: identity card, optional cloud registration, AI config, and
/// marker write. Pulled out so the rollback path in [`run_bootstrap`]
/// stays linear.
async fn run_bootstrap_post_identity(
    state: &AppState,
    config_dir: &ConfigDir,
    marker_path: &std::path::Path,
    req: &BootstrapRequest,
    id: &identity::NodeIdentity,
) -> Result<(Option<CloudInfo>, Vec<String>), BootstrapError> {
    let user_hash = user_hash_from_pubkey(&id.public_key);

    // Recovery phrase — derived from the same private key whether we
    // minted or restored. Returned in the response only on fresh-mint.
    let recovery_phrase = derive_recovery_phrase(&id.private_key)
        .map_err(|e| BootstrapError::Internal(format!("Failed to derive recovery phrase: {e}")))?;

    // ---- (5) Cloud registration (optional) ----
    let mut cloud_sync_config: Option<CloudSyncConfig> = None;
    let mut cloud_info: Option<CloudInfo> = None;
    if req.enable_cloud {
        let invite = req
            .invite_code
            .as_deref()
            .ok_or_else(|| BootstrapError::Internal("invite_code missing".into()))?;
        let api_url = crate::endpoints::exemem_api_url();
        use base64::Engine;
        let pub_key_bytes = base64::engine::general_purpose::STANDARD
            .decode(&id.public_key)
            .map_err(|e| BootstrapError::Internal(format!("Failed to decode public key: {e}")))?;
        let public_key_hex = crate::handlers::setup::hex_encode(&pub_key_bytes);

        let private_key_b64 = id.private_key.clone();
        let api_url_for_call = api_url.clone();
        let invite_owned = invite.to_string();
        let resp = tokio::task::spawn_blocking(move || {
            register_with_exemem_and_invite(
                &api_url_for_call,
                &public_key_hex,
                &private_key_b64,
                Some(&invite_owned),
            )
        })
        .await
        .map_err(|e| BootstrapError::Internal(format!("register thread join failed: {e}")))?
        .map_err(|e| {
            // Exemem 4xx with an "already registered with different identity"
            // message is the canonical conflict between a recovery phrase
            // and an invite code that belongs to someone else. Surface as
            // 409 so the client can recover cleanly.
            if e.contains("HTTP 409") || e.to_lowercase().contains("conflict") {
                BootstrapError::Conflict(e)
            } else {
                BootstrapError::Internal(format!("Exemem registration failed: {e}"))
            }
        })?;

        let api_key = resp.api_key.ok_or_else(|| {
            BootstrapError::Internal("Registration response missing api_key".to_string())
        })?;
        let exemem_user_hash = resp.user_hash.clone().unwrap_or_else(|| user_hash.clone());

        // Persist credentials — must succeed for cloud-sync to function.
        let creds = crate::keychain::ExememCredentials {
            user_hash: exemem_user_hash.clone(),
            session_token: String::new(),
            api_key: api_key.clone(),
        };
        crate::keychain::store_credentials(&creds)
            .map_err(|e| BootstrapError::Internal(format!("Failed to store credentials: {e}")))?;

        cloud_sync_config = Some(CloudSyncConfig {
            api_url,
            api_key,
            session_token: None,
            user_hash: resp.user_hash,
            p2p_sync: None,
        });
        cloud_info = Some(CloudInfo {
            enabled: true,
            exemem_user_hash,
        });
    }

    // ---- (6) Anthropic key + (7-equivalent) ingestion_config.json ----
    if let Some(provider) = req.ai_provider.as_deref() {
        match provider {
            "anthropic" => {
                let key = req
                    .anthropic_api_key
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        BootstrapError::Internal(
                            "ai_provider=anthropic requires a non-empty anthropic_api_key"
                                .to_string(),
                        )
                    })?;
                crate::ingestion::anthropic_key_store::save(config_dir.as_path(), key).map_err(
                    |e| BootstrapError::Internal(format!("Failed to save Anthropic key: {e}")),
                )?;
                let cfg = serde_json::json!({
                    "provider": "Anthropic",
                    "anthropic": {
                        "model": "claude-haiku-4-5-20251001",
                        "base_url": "https://api.anthropic.com"
                    }
                });
                write_ingestion_config(config_dir.as_path(), &cfg)?;
            }
            "ollama" => {
                let url = req
                    .ollama_url
                    .clone()
                    .unwrap_or_else(|| "http://localhost:11434".to_string());
                let model = req
                    .ollama_model
                    .clone()
                    .unwrap_or_else(|| "llama3.2".to_string());
                let cfg = serde_json::json!({
                    "provider": "Ollama",
                    "ollama": {
                        "model": model,
                        "base_url": url,
                    }
                });
                write_ingestion_config(config_dir.as_path(), &cfg)?;
            }
            "" | "skip" | "none" => {
                // Persist the user's explicit opt-out as a saved file with
                // `enabled: false`. Without this, a stale `ANTHROPIC_API_KEY`
                // shell export plus the default `INGESTION_ENABLED=true`
                // env-var fallback in `IngestionConfig::load` would silently
                // re-enable ingestion on the next boot, defeating the user's
                // explicit choice. See `IngestionConfig::load` for the
                // precedence rule (saved enabled wins over env var).
                let cfg = serde_json::json!({
                    "provider": "Anthropic",
                    "enabled": false,
                });
                write_ingestion_config(config_dir.as_path(), &cfg)?;
            }
            other => {
                return Err(BootstrapError::Internal(format!(
                    "Unknown ai_provider {other:?}; expected anthropic, ollama, or skip"
                )));
            }
        }
    }

    // ---- (7) NodeConfig — atomic write via save_node_config ----
    let mut config = state.node_manager.get_base_config().await;
    let database_path = config.database.path.clone();
    config.database = match cloud_sync_config.clone() {
        Some(cs) => DatabaseConfig::with_cloud_sync(database_path, cs),
        None => DatabaseConfig::local(database_path),
    };
    save_node_config(&config)
        .map_err(|e| BootstrapError::Internal(format!("Failed to save node config: {e}")))?;

    // Push the new config into NodeManager so the next get_node uses it.
    // Must run BEFORE we ask for the FoldNode handle below — otherwise a
    // cloud-sync activation would not be visible to the freshly-built node.
    state.node_manager.update_config(config).await;

    // ---- (4) Save IdentityCard via the live FoldNode ----
    // Driving this through node_manager.get_node ensures we go through the
    // shared SledPool — no short-lived FoldNode hack, no second flock holder.
    let card = IdentityCard::new(
        req.name.trim().to_string(),
        req.email.clone().filter(|s| !s.is_empty()),
        req.birthday.clone().filter(|s| !s.is_empty()),
    );
    let node = state
        .node_manager
        .get_node(&user_hash)
        .await
        .map_err(|e| BootstrapError::Internal(format!("Failed to acquire node: {e}")))?;
    let db = node
        .get_fold_db()
        .map_err(|e| BootstrapError::Internal(format!("Failed to access FoldDB: {e}")))?;
    card.save(&db)
        .await
        .map_err(|e| BootstrapError::Internal(format!("Failed to save identity card: {e}")))?;

    // ---- (8) Marker — last write so a partial failure leaves the user in
    // the onboarding flow rather than locked out. ----
    crate::sensitive_io::write_sensitive(marker_path, b"1")
        .map_err(|e| BootstrapError::Internal(format!("Failed to write onboarding marker: {e}")))?;

    Ok((cloud_info, recovery_phrase))
}

fn write_ingestion_config(
    config_dir: &std::path::Path,
    cfg: &serde_json::Value,
) -> Result<(), BootstrapError> {
    if !config_dir.exists() {
        std::fs::create_dir_all(config_dir)
            .map_err(|e| BootstrapError::Internal(format!("Failed to create config dir: {e}")))?;
    }
    let path = config_dir.join("ingestion_config.json");
    let json = serde_json::to_string_pretty(cfg)
        .map_err(|e| BootstrapError::Internal(format!("Failed to serialize AI config: {e}")))?;
    crate::sensitive_io::write_atomic_0600(&path, json.as_bytes())
        .map_err(|e| BootstrapError::Internal(format!("Failed to write AI config: {e}")))
}

fn marker_path() -> Result<PathBuf, String> {
    Ok(crate::utils::paths::folddb_home()?
        .join("data")
        .join(".onboarding_complete"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fold_node::config::NodeConfig;
    use crate::server::node_manager::{NodeManager, NodeManagerConfig};

    /// Aliased to [`crate::secure_store::test_master_key::lock`] so the
    /// FOLDDB_HOME-touching tests below serialize with every other
    /// env-mutating test in the crate.
    fn home_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::secure_store::test_master_key::lock()
    }

    fn build_app_state(home: &std::path::Path) -> (web::Data<AppState>, web::Data<ConfigDir>) {
        let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
        let cfg = NodeManagerConfig {
            base_config: NodeConfig::new(home.join("data"))
                .with_schema_service_url("test://mock")
                .with_seed_identity(crate::identity::identity_from_keypair(&keypair)),
            config_dir: home.join("config"),
            upload_path: home.join("uploads"),
        };
        let manager = Arc::new(NodeManager::new(cfg));
        let state = web::Data::new(AppState {
            node_manager: manager,
        });
        let config_dir = web::Data::new(ConfigDir(home.join("config")));
        (state, config_dir)
    }

    /// 410 when the marker is already present — bootstrap is one-shot and
    /// must not rotate identity once the user has finished onboarding.
    #[actix_web::test]
    #[allow(clippy::await_holding_lock)]
    async fn returns_410_when_marker_exists() {
        let _g = home_lock();
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("FOLDDB_HOME", tmp.path());

        let data_dir = tmp.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(data_dir.join(".onboarding_complete"), "1").unwrap();

        let (state, config_dir) = build_app_state(tmp.path());
        let body = web::Json(BootstrapRequest {
            name: "test".into(),
            email: None,
            birthday: None,
            ai_provider: None,
            anthropic_api_key: None,
            ollama_url: None,
            ollama_model: None,
            enable_cloud: false,
            invite_code: None,
            recovery_phrase: None,
        });
        let resp = bootstrap(state, config_dir, body)
            .await
            .respond_to(&actix_web::test::TestRequest::default().to_http_request());
        assert_eq!(resp.status(), 410);

        std::env::remove_var("FOLDDB_HOME");
    }

    /// Cloud=true without invite_code is a 400.
    #[actix_web::test]
    #[allow(clippy::await_holding_lock)]
    async fn requires_invite_code_when_cloud_enabled() {
        let _g = home_lock();
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("FOLDDB_HOME", tmp.path());

        let (state, config_dir) = build_app_state(tmp.path());
        let body = web::Json(BootstrapRequest {
            name: "test".into(),
            email: None,
            birthday: None,
            ai_provider: None,
            anthropic_api_key: None,
            ollama_url: None,
            ollama_model: None,
            enable_cloud: true,
            invite_code: None,
            recovery_phrase: None,
        });
        let resp = bootstrap(state, config_dir, body)
            .await
            .respond_to(&actix_web::test::TestRequest::default().to_http_request());
        assert_eq!(resp.status(), 400);

        std::env::remove_var("FOLDDB_HOME");
    }

    /// Bad birthday format is a 400 — caught up-front so we don't half-write
    /// the identity before noticing.
    #[actix_web::test]
    #[allow(clippy::await_holding_lock)]
    async fn rejects_invalid_birthday() {
        let _g = home_lock();
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("FOLDDB_HOME", tmp.path());

        let (state, config_dir) = build_app_state(tmp.path());
        let body = web::Json(BootstrapRequest {
            name: "test".into(),
            email: None,
            birthday: Some("99-99".to_string()),
            ai_provider: None,
            anthropic_api_key: None,
            ollama_url: None,
            ollama_model: None,
            enable_cloud: false,
            invite_code: None,
            recovery_phrase: None,
        });
        let resp = bootstrap(state, config_dir, body)
            .await
            .respond_to(&actix_web::test::TestRequest::default().to_http_request());
        assert_eq!(resp.status(), 400);

        std::env::remove_var("FOLDDB_HOME");
    }

    /// Marker resolution honours `FOLDDB_HOME`. Ensures `data/.onboarding_complete`
    /// is the path checked at the top of the handler.
    #[test]
    fn marker_path_uses_folddb_home() {
        let _g = home_lock();
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("FOLDDB_HOME", tmp.path());

        let resolved = marker_path().expect("marker path");
        assert_eq!(
            resolved,
            tmp.path().join("data").join(".onboarding_complete")
        );

        std::env::remove_var("FOLDDB_HOME");
    }

    /// Regression for the P0 fresh-install plaintext-at-rest bug. On a fresh
    /// install with `os-keychain` enabled, the bootstrap handler MUST persist
    /// the node identity `ENC:`-prefixed on the very first write — relying on
    /// the boot-time legacy migration to re-encrypt on the next daemon restart
    /// leaves hours of plaintext-at-rest in the Tauri single-session model,
    /// defeating the whole keychain feature.
    ///
    /// Gated on `os-keychain` because the encrypted-vs-plaintext distinction
    /// only exists in that build. `FOLDDB_MASTER_KEY` is the documented
    /// keychain-free escape hatch (see `secure_store::test_master_key`) so
    /// this can run in CI without prompting Apple's Keychain.
    #[cfg(feature = "os-keychain")]
    #[actix_web::test]
    #[allow(clippy::await_holding_lock)]
    async fn bootstrap_persists_encrypted_identity_fresh_mint() {
        let _g = crate::secure_store::test_master_key::with_set();
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("FOLDDB_HOME", tmp.path());

        let (state, config_dir) = build_app_state(tmp.path());
        let body = web::Json(BootstrapRequest {
            name: "test".into(),
            email: None,
            birthday: None,
            ai_provider: None,
            anthropic_api_key: None,
            ollama_url: None,
            ollama_model: None,
            enable_cloud: false,
            invite_code: None,
            recovery_phrase: None,
        });
        let resp = bootstrap(state.clone(), config_dir, body)
            .await
            .respond_to(&actix_web::test::TestRequest::default().to_http_request());
        assert_eq!(resp.status(), 200, "bootstrap must succeed");

        // Read the raw on-disk blob from the same pool the handler used.
        let pool = state.node_manager.get_or_init_sled_pool().await;
        let raw = crate::identity::peek_raw_identity_value(&pool)
            .expect("peek raw identity")
            .expect("identity must be persisted after bootstrap");
        assert!(
            raw.starts_with("ENC:"),
            "fresh-mint bootstrap must persist encrypted identity on first write; \
             got prefix: {:?}",
            raw.chars().take(10).collect::<String>()
        );

        std::env::remove_var("FOLDDB_HOME");
    }

    /// Same property for the recovery-phrase restore branch. Deriving
    /// deterministically from a BIP39 phrase must still produce an
    /// `ENC:`-prefixed on-disk blob on the first write — restored
    /// identities sit on disk for the user's whole session and shouldn't
    /// be plaintext until the next reboot.
    #[cfg(feature = "os-keychain")]
    #[actix_web::test]
    #[allow(clippy::await_holding_lock)]
    async fn bootstrap_persists_encrypted_identity_from_recovery_phrase() {
        let _g = crate::secure_store::test_master_key::with_set();
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("FOLDDB_HOME", tmp.path());

        // Generate a phrase by round-tripping through derive_recovery_phrase
        // so the test doesn't hardcode any 24-word sequence.
        let kp = fold_db::security::Ed25519KeyPair::generate().unwrap();
        let id = crate::identity::identity_from_keypair(&kp);
        let phrase = derive_recovery_phrase(&id.private_key)
            .expect("derive phrase")
            .join(" ");

        let (state, config_dir) = build_app_state(tmp.path());
        let body = web::Json(BootstrapRequest {
            name: "test".into(),
            email: None,
            birthday: None,
            ai_provider: None,
            anthropic_api_key: None,
            ollama_url: None,
            ollama_model: None,
            enable_cloud: false,
            invite_code: None,
            recovery_phrase: Some(phrase),
        });
        let resp = bootstrap(state.clone(), config_dir, body)
            .await
            .respond_to(&actix_web::test::TestRequest::default().to_http_request());
        assert_eq!(resp.status(), 200, "bootstrap must succeed");

        let pool = state.node_manager.get_or_init_sled_pool().await;
        let raw = crate::identity::peek_raw_identity_value(&pool)
            .expect("peek raw identity")
            .expect("identity must be persisted after bootstrap");
        assert!(
            raw.starts_with("ENC:"),
            "restore-from-phrase bootstrap must persist encrypted identity on first write; \
             got prefix: {:?}",
            raw.chars().take(10).collect::<String>()
        );

        std::env::remove_var("FOLDDB_HOME");
    }

    /// Privacy/consent regression: when the user picks "skip" in the AI
    /// provider step of onboarding, the skip arm of bootstrap MUST persist a
    /// saved `ingestion_config.json` with an explicit `enabled: false`.
    /// Without that, `IngestionConfig::load` falls through to the
    /// `INGESTION_ENABLED` env-var default of `true` and the user's explicit
    /// opt-out is silently overridden — see the matching precedence tests
    /// in `ingestion::config`. This test exercises the route end-to-end
    /// through `bootstrap` so the wiring (route → run_bootstrap →
    /// run_bootstrap_post_identity → skip arm → write_ingestion_config)
    /// stays in lockstep.
    #[actix_web::test]
    #[allow(clippy::await_holding_lock)]
    async fn bootstrap_skip_writes_disabled_ingestion_config() {
        let _g = crate::secure_store::test_master_key::with_set();
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("FOLDDB_HOME", tmp.path());

        let (state, config_dir) = build_app_state(tmp.path());
        let body = web::Json(BootstrapRequest {
            name: "test".into(),
            email: None,
            birthday: None,
            ai_provider: Some("skip".to_string()),
            anthropic_api_key: None,
            ollama_url: None,
            ollama_model: None,
            enable_cloud: false,
            invite_code: None,
            recovery_phrase: None,
        });
        let resp = bootstrap(state, config_dir.clone(), body)
            .await
            .respond_to(&actix_web::test::TestRequest::default().to_http_request());
        assert_eq!(resp.status(), 200, "bootstrap with skip must succeed");

        // The saved config must encode the user's explicit opt-out so the
        // env-var fallback in `IngestionConfig::load` doesn't override it.
        let cfg_path = config_dir.as_path().join("ingestion_config.json");
        assert!(
            cfg_path.exists(),
            "skip arm must persist ingestion_config.json (got missing file)"
        );
        let saved: crate::ingestion::config::SavedConfig =
            serde_json::from_slice(&std::fs::read(&cfg_path).expect("read ingestion_config.json"))
                .expect("parse SavedConfig");
        assert_eq!(
            saved.enabled,
            Some(false),
            "skip arm must persist enabled=false; got {:?}",
            saved.enabled
        );

        // End-to-end: even with `ANTHROPIC_API_KEY` set, the load result
        // must report disabled+unconfigured. This mirrors the real-world
        // shell where a developer's `~/.zshrc` exports the var.
        std::env::set_var("ANTHROPIC_API_KEY", "sk-ant-stale-shell-export");
        let loaded =
            crate::ingestion::config::IngestionConfig::load(config_dir.as_path()).expect("load");
        std::env::remove_var("ANTHROPIC_API_KEY");
        assert!(
            !loaded.enabled,
            "post-skip load must report enabled=false even with ANTHROPIC_API_KEY set"
        );
        assert!(
            !loaded.is_ready(),
            "post-skip load must report not-configured (is_ready=false)"
        );
        assert_eq!(
            loaded.anthropic.api_key, "",
            "post-skip load must not pre-populate the api_key from env"
        );

        std::env::remove_var("FOLDDB_HOME");
    }

    /// `write_ingestion_config` must route through the atomic-write helper so
    /// a power loss between serialize and rename can't leave a half-written
    /// `ingestion_config.json` that blocks the next boot. This test calls the
    /// function directly with a tempdir and asserts the file exists with the
    /// expected JSON. The atomicity invariants (no `.tmp` leftover, mode
    /// 0o600) are covered by the `sensitive_io::write_atomic_0600` tests.
    #[test]
    fn write_ingestion_config_persists_expected_json() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = serde_json::json!({
            "providers": [
                { "name": "anthropic", "api_key_ref": "exemem-credentials" }
            ],
            "default_provider": "anthropic"
        });

        write_ingestion_config(tmp.path(), &cfg).expect("write_ingestion_config");

        let path = tmp.path().join("ingestion_config.json");
        assert!(
            path.exists(),
            "ingestion_config.json must exist after write"
        );
        let on_disk: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("read ingestion_config.json"))
                .expect("parse ingestion_config.json");
        assert_eq!(on_disk, cfg, "round-trip JSON must match input");
    }
}
