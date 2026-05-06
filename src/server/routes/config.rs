use crate::server::http_server::AppState;
use crate::utils::crypto::user_hash_from_pubkey;
use actix_web::{web, HttpResponse, Responder};
use fold_db::storage::config::DatabaseConfig;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Path;

/// Database configuration request/response types
#[derive(Deserialize, Serialize, utoipa::ToSchema, Debug, Clone)]
pub struct DatabaseConfigRequest {
    pub database: DatabaseConfigDto,
}

#[derive(Deserialize, Serialize, utoipa::ToSchema, Debug, Clone)]
pub struct DatabaseConfigDto {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud_sync: Option<CloudSyncConfigDto>,
}

#[derive(Deserialize, Serialize, utoipa::ToSchema, Debug, Clone)]
pub struct CloudSyncConfigDto {
    pub api_url: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct DatabaseConfigResponse {
    pub success: bool,
    pub message: String,
    pub requires_restart: bool,
}

/// Get current database configuration
#[utoipa::path(
    get,
    path = "/api/system/database-config",
    tag = "system",
    responses(
        (status = 200, description = "Database configuration", body = DatabaseConfigDto)
    )
)]
pub async fn get_database_config(state: web::Data<AppState>) -> impl Responder {
    // Get the base configuration from NodeManager (not per-user)
    let config = state.node_manager.get_base_config().await;

    let db_config = DatabaseConfigDto {
        path: config.database.path.to_string_lossy().to_string(),
        cloud_sync: config
            .database
            .cloud_sync
            .as_ref()
            .map(|cs| CloudSyncConfigDto {
                api_url: cs.api_url.clone(),
            }),
    };

    HttpResponse::Ok().json(db_config)
}

/// Get the auto-generated identity for local/desktop mode.
///
/// Returns the node's unique public key (from config) as the user identity.
/// Each installation gets its own keypair, so every user has a unique identity.
/// This endpoint does NOT require authentication, allowing the frontend
/// to auto-authenticate without a login step.
#[utoipa::path(
    get,
    path = "/api/system/auto-identity",
    tag = "system",
    responses(
        (status = 200, description = "Default identity for auto-login", body = serde_json::Value)
    )
)]
pub async fn auto_identity(state: web::Data<AppState>) -> impl Responder {
    // Read the node identity (does NOT mint — only the
    // POST /api/setup/bootstrap route may do that). When the tree is
    // empty, surface the canonical 503 so the UI / CLI knows to route
    // the user through bootstrap instead of treating it as a 500.
    let public_key = match state.node_manager.ensure_default_identity().await {
        Ok(pk) => pk,
        Err(crate::server::NodeManagerError::NotProvisioned) => {
            return crate::utils::http_errors::node_not_provisioned_response();
        }
        Err(e) => {
            return HttpResponse::InternalServerError().json(json!({
                "ok": false,
                "error": format!("Failed to initialize node identity: {e}")
            }));
        }
    };

    // Derive user_hash = SHA256(public_key)[0:32] (same algorithm as frontend)
    let user_hash = user_hash_from_pubkey(&public_key);

    HttpResponse::Ok().json(json!({
        "user_id": public_key,
        "user_hash": user_hash,
        "public_key": public_key,
    }))
}

/// Update database configuration
///
/// This endpoint updates the database configuration in the node config file.
/// The server must be restarted for the changes to take effect.
#[utoipa::path(
    post,
    path = "/api/system/database-config",
    tag = "system",
    request_body = DatabaseConfigRequest,
    responses(
        (status = 200, description = "Configuration updated", body = DatabaseConfigResponse),
        (status = 400, description = "Bad request", body = DatabaseConfigResponse),
        (status = 500, description = "Server error", body = DatabaseConfigResponse)
    )
)]
pub async fn update_database_config(
    _state: web::Data<AppState>,
    _req: web::Json<DatabaseConfigRequest>,
) -> impl Responder {
    // NOTE: Dynamic database config updates are not supported in multi-tenant mode
    // The database configuration is set at startup and affects all users.
    // To change the database configuration, update the config file and restart the server.
    HttpResponse::BadRequest().json(DatabaseConfigResponse {
        success: false,
        message: "Dynamic database configuration updates are not supported. Please update the configuration file and restart the server.".to_string(),
        requires_restart: true,
    })
}

/// Request body for system setup (matches CLI setup wizard)
#[derive(Deserialize, Serialize, utoipa::ToSchema, Debug, Clone)]
pub struct SetupRequest {
    /// Storage configuration (optional: only update if provided)
    #[serde(default)]
    pub storage: Option<StorageSetup>,
    /// Schema service URL (optional: only update if provided)
    #[serde(default)]
    pub schema_service_url: Option<String>,
}

/// Storage setup options matching CLI wizard
#[derive(Deserialize, Serialize, utoipa::ToSchema, Debug, Clone)]
#[serde(tag = "type")]
pub enum StorageSetup {
    /// Local Sled storage
    #[serde(rename = "local")]
    Local { path: String },
    /// Exemem cloud storage (local Sled + encrypted S3 sync)
    #[serde(rename = "exemem")]
    Exemem { api_url: String, api_key: String },
}

/// Response for setup endpoint
#[derive(Serialize, utoipa::ToSchema)]
pub struct SetupResponse {
    pub success: bool,
    pub message: String,
}

use crate::fold_node::config::save_node_config as persist_node_config;

/// Apply setup configuration (storage and/or schema service URL)
///
/// This endpoint allows the UI wizard to configure the same settings as the CLI
/// setup wizard. It updates the config, persists it to disk, and invalidates
/// cached nodes so the next request uses the new configuration.
#[utoipa::path(
    post,
    path = "/api/system/setup",
    tag = "system",
    request_body = SetupRequest,
    responses(
        (status = 200, description = "Setup applied successfully", body = SetupResponse),
        (status = 400, description = "Bad request", body = SetupResponse),
        (status = 500, description = "Server error", body = SetupResponse)
    )
)]
pub async fn apply_setup(
    state: web::Data<AppState>,
    req: web::Json<SetupRequest>,
) -> impl Responder {
    // Read current config
    let mut config = state.node_manager.get_base_config().await;

    let mut changes = Vec::new();

    // Apply storage override if provided
    if let Some(ref storage) = req.storage {
        match storage {
            StorageSetup::Local { path } => {
                // Expand `~` / `~/...` so the path doesn't get persisted as a
                // literal tilde and turn into `<cwd>/~/.folddb/data/` at Sled
                // open time (see DatabaseSetupScreen which sends `~/.folddb/data`).
                let resolved = match crate::server::routes::filesystem::expand_tilde(path) {
                    Ok(p) => p,
                    Err(msg) => {
                        return HttpResponse::BadRequest().json(SetupResponse {
                            success: false,
                            message: msg,
                        });
                    }
                };
                config.database = DatabaseConfig::local(resolved);
                changes.push("storage (local)");
            }
            StorageSetup::Exemem { api_url, api_key } => {
                let cloud_sync = fold_db::storage::config::CloudSyncConfig {
                    api_url: api_url.clone(),
                    api_key: api_key.clone(),
                    session_token: None,
                    user_hash: None,
                    p2p_sync: None,
                };
                // Normalize the existing on-disk path too — a previous Local
                // setup may have persisted a literal `~`-prefixed string.
                let existing_path = config.database.path.to_string_lossy().to_string();
                let resolved = match crate::server::routes::filesystem::expand_tilde(&existing_path)
                {
                    Ok(p) => p,
                    Err(msg) => {
                        return HttpResponse::BadRequest().json(SetupResponse {
                            success: false,
                            message: msg,
                        });
                    }
                };
                config.database = DatabaseConfig::with_cloud_sync(resolved, cloud_sync);
                changes.push("storage (exemem)");
                // api_url is persisted via the DatabaseConfig -> node_config.json
                // write below. No separate Sled copy.
            }
        }
    }

    // Apply schema_service_url override if provided
    if let Some(ref url) = req.schema_service_url {
        config.schema_service_url = Some(url.clone());
        changes.push("schema service URL");
    }

    if changes.is_empty() {
        return HttpResponse::BadRequest().json(SetupResponse {
            success: false,
            message: "No configuration changes provided".to_string(),
        });
    }

    // Persist to disk
    if let Err(e) = persist_node_config(&config) {
        tracing::error!(
            target: "fold_node::http_server",
            "Failed to persist setup config: {}",
            e
        );
        return HttpResponse::InternalServerError().json(SetupResponse {
            success: false,
            message: format!("Failed to save configuration: {}", e),
        });
    }

    // Update NodeManager config and invalidate all cached nodes.
    // `update_config` takes the new `NodeConfig` directly — the manager
    // preserves the boot-time `config_dir` / `upload_path` across updates.
    state.node_manager.update_config(config).await;

    let message = format!("Setup applied: {}", changes.join(", "));
    tracing::info!(
            target: "fold_node::http_server", "{}", message);

    HttpResponse::Ok().json(SetupResponse {
        success: true,
        message,
    })
}

/// Response for database status check
#[derive(Serialize, utoipa::ToSchema)]
pub struct DatabaseStatusResponse {
    /// Whether the database has been initialized (a node is active)
    pub initialized: bool,
    /// Whether a saved config file exists on disk (returning user)
    pub has_saved_config: bool,
    /// Whether the onboarding wizard has been completed (marker file in data dir)
    pub onboarding_complete: bool,
}

/// Get database initialization status
///
/// Returns whether the database has been initialized and whether a saved config
/// exists. For returning users with a saved config, this endpoint auto-initializes
/// the node and returns `initialized: true`. For fresh installs, returns
/// `initialized: false` so the frontend can show the setup screen.
///
/// This endpoint does NOT require a node to exist — it's safe to call before
/// the database is initialized.
#[utoipa::path(
    get,
    path = "/api/system/database-status",
    tag = "system",
    responses(
        (status = 200, description = "Database status", body = DatabaseStatusResponse)
    )
)]
pub async fn get_database_status(state: web::Data<AppState>) -> impl Responder {
    let config_path = std::env::var("NODE_CONFIG").unwrap_or_else(|_| {
        crate::utils::paths::folddb_home()
            .map(|h| {
                h.join("config")
                    .join("node_config.json")
                    .to_string_lossy()
                    .to_string()
            })
            .unwrap_or_else(|_| "config/node_config.json".to_string())
    });
    let has_saved_config = Path::new(&config_path).exists();

    let initialized = if state.node_manager.has_active_node().await {
        true
    } else if has_saved_config {
        // For returning users, try to auto-initialize. The public key lives
        // in the Sled `node_identity` tree (not on NodeConfig) — read it
        // via the shared pool.
        let pool = state.node_manager.get_or_init_sled_pool().await;
        match crate::identity::load(pool).ok().flatten() {
            Some(id) if !id.public_key.is_empty() => {
                let user_hash = user_hash_from_pubkey(&id.public_key);
                state
                    .node_manager
                    .get_node(&user_hash)
                    .await
                    .inspect_err(|e| {
                        tracing::warn!(
                        target: "fold_node::http_server",
                                        "Auto-initialization failed for returning user: {}",
                                        e
                                    );
                    })
                    .is_ok()
            }
            _ => false,
        }
    } else {
        false
    };

    // Check for onboarding marker file in the data directory.
    // This file is created when the user completes the onboarding wizard,
    // and is wiped when --empty-db removes the data directory.
    let onboarding_complete = crate::utils::paths::folddb_home()
        .map(|h| h.join("data").join(".onboarding_complete").exists())
        .unwrap_or(false);

    HttpResponse::Ok().json(DatabaseStatusResponse {
        initialized,
        has_saved_config,
        onboarding_complete,
    })
}

/// Mark onboarding as complete by writing a marker file in the data directory.
///
/// This marker lives in `FOLDDB_HOME/data/.onboarding_complete` so that
/// `--empty-db` (which removes the data directory) also resets onboarding state.
#[utoipa::path(
    post,
    path = "/api/system/onboarding-complete",
    tag = "system",
    responses(
        (status = 200, description = "Onboarding marked complete", body = serde_json::Value)
    )
)]
pub async fn mark_onboarding_complete() -> impl Responder {
    let marker_path = match crate::utils::paths::folddb_home() {
        Ok(h) => h.join("data").join(".onboarding_complete"),
        Err(e) => {
            return HttpResponse::InternalServerError().json(json!({
                "ok": false,
                "error": format!("Cannot determine FOLDDB_HOME: {e}")
            }));
        }
    };

    // Routed through sensitive_io: the marker's mere presence reveals that a
    // credentialed setup flow ran (PR #885 reasoning). write_sensitive
    // creates the parent directory itself.
    if let Err(e) = crate::sensitive_io::write_sensitive(&marker_path, b"1") {
        return HttpResponse::InternalServerError().json(json!({
            "ok": false,
            "error": format!("Failed to write onboarding marker: {e}")
        }));
    }

    HttpResponse::Ok().json(json!({ "ok": true }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fold_node::config::NodeConfig;
    use crate::server::http_server::AppState;
    use crate::server::node_manager::{NodeManager, NodeManagerConfig};
    use std::sync::Arc;

    /// Reuses the crate-wide env mutex so tests that mutate `HOME` /
    /// `FOLDDB_HOME` don't race with auth tests doing the same thing.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::secure_store::test_master_key::lock()
    }

    /// Build an `AppState` whose `NodeConfig.source_path` points inside
    /// `tmp` so `persist_node_config` writes there instead of the real
    /// `$FOLDDB_HOME/config/node_config.json`.
    fn test_app_state(tmp: &tempfile::TempDir) -> web::Data<AppState> {
        std::fs::create_dir_all(tmp.path().join("config")).expect("create config dir");
        let source_path = tmp.path().join("config").join("node_config.json");
        let base_config = NodeConfig {
            database: fold_db::storage::DatabaseConfig::local(tmp.path().join("data")),
            storage_path: Some(tmp.path().join("data")),
            network_listen_address: "/ip4/0.0.0.0/tcp/0".to_string(),
            schema_service_url: Some("test://mock".to_string()),
            env: None,
            config_dir: Some(tmp.path().join("config")),
            seed_identity: None,
            source_path: Some(source_path),
        };
        let node_manager = Arc::new(NodeManager::new(NodeManagerConfig {
            base_config,
            config_dir: tmp.path().join("config"),
            upload_path: tmp.path().join("uploads"),
        }));
        web::Data::new(AppState { node_manager })
    }

    /// Regression test for the literal-`~` directory bug: the UI's default
    /// "Local storage" path is `~/.folddb/data`, but `apply_setup` used to
    /// pass that string straight into `PathBuf::from`, so Sled later
    /// created `<cwd>/~/.folddb/data/`. After the fix, the path stored in
    /// `NodeConfig.database.path` must be an absolute path under `$HOME`.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn apply_setup_local_expands_tilde() {
        let _guard = env_lock();
        let tmp = tempfile::tempdir().expect("tempdir");
        // Treat the tempdir as HOME so the assertion is hermetic — we
        // don't want the test's idea of "under $HOME" to depend on the
        // CI runner's real home.
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());

        let state = test_app_state(&tmp);
        let req = web::Json(SetupRequest {
            storage: Some(StorageSetup::Local {
                path: "~/.folddb/data".to_string(),
            }),
            schema_service_url: None,
        });

        let resp = apply_setup(state.clone(), req)
            .await
            .respond_to(&actix_web::test::TestRequest::post().to_http_request());
        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);

        let resolved = state.node_manager.get_base_config().await.database.path;
        assert!(
            resolved.is_absolute(),
            "stored database path must be absolute, got {:?}",
            resolved
        );
        assert!(
            resolved.starts_with(tmp.path()),
            "stored database path must live under $HOME ({:?}), got {:?}",
            tmp.path(),
            resolved
        );
        assert!(
            !resolved.to_string_lossy().contains('~'),
            "stored database path must not contain a literal '~', got {:?}",
            resolved
        );
        assert_eq!(resolved, tmp.path().join(".folddb/data"));

        match original_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    /// Bootstrap-collapse step 2: hitting GET /api/system/auto-identity
    /// before the user has gone through POST /api/setup/bootstrap MUST
    /// return the canonical 503 instead of silently minting a fresh
    /// identity (the old `load_or_generate` behavior). The body shape
    /// is fixed at `{error: "node_not_provisioned", next: "POST /api/setup/bootstrap"}`
    /// — clients pivot off both fields.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn auto_identity_returns_503_when_not_provisioned() {
        let _guard = env_lock();
        let tmp = tempfile::tempdir().expect("tempdir");

        let state = test_app_state(&tmp);
        let resp = auto_identity(state)
            .await
            .respond_to(&actix_web::test::TestRequest::get().to_http_request());

        assert_eq!(
            resp.status(),
            actix_web::http::StatusCode::SERVICE_UNAVAILABLE,
            "auto_identity must surface NotProvisioned as 503"
        );
        let body_bytes = match actix_web::body::to_bytes(resp.into_body()).await {
            Ok(b) => b,
            Err(_) => panic!("failed to read auto_identity response body"),
        };
        let body: serde_json::Value =
            serde_json::from_slice(&body_bytes).expect("response body is JSON");
        assert_eq!(body["error"], "node_not_provisioned");
        assert_eq!(body["next"], "POST /api/setup/bootstrap");
    }
}
