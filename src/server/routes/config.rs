use crate::fold_node::config::NodeConfig;
use crate::server::http_server::AppState;
use crate::server::node_manager::NodeManagerConfig;
use crate::utils::crypto::user_hash_from_pubkey;
use actix_web::{web, HttpResponse, Responder};
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
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
#[serde(tag = "type")]
pub enum DatabaseConfigDto {
    #[serde(rename = "local")]
    Local { path: String },
    #[cfg(feature = "aws-backend")]
    #[serde(rename = "cloud", alias = "dynamodb")]
    Cloud(Box<CloudConfigDto>),
    #[serde(rename = "exemem")]
    Exemem { api_url: String },
}

/// DTO for ExplicitTables
#[derive(Deserialize, Serialize, utoipa::ToSchema, Debug, Clone, Default)]
pub struct ExplicitTablesDto {
    pub main: String,
    pub metadata: String,
    pub permissions: String,
    pub schema_states: String,
    pub schemas: String,
    pub public_keys: String,
    pub native_index: String,
    pub process: String,
    pub logs: String,
    pub idempotency: String,
}

/// DTO for CloudConfig (formerly DynamoDbConfig)
#[cfg(feature = "aws-backend")]
#[derive(Deserialize, Serialize, utoipa::ToSchema, Debug, Clone)]
pub struct CloudConfigDto {
    pub region: String,
    /// Explicit table names for all required namespaces
    pub tables: ExplicitTablesDto,
    pub auto_create: bool,
    pub user_id: Option<String>,
    pub file_storage_bucket: Option<String>,
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

    let db_config = match &config.database {
        DatabaseConfig::Local { path } => DatabaseConfigDto::Local {
            path: path.to_string_lossy().to_string(),
        },
        #[cfg(feature = "aws-backend")]
        DatabaseConfig::Cloud(config) => DatabaseConfigDto::Cloud(Box::new(CloudConfigDto {
            region: config.region.clone(),
            auto_create: config.auto_create,
            user_id: config.user_id.clone(),
            file_storage_bucket: config.file_storage_bucket.clone(),
            tables: ExplicitTablesDto {
                main: config.tables.main.clone(),
                metadata: config.tables.metadata.clone(),
                permissions: config.tables.permissions.clone(),
                schema_states: config.tables.schema_states.clone(),
                schemas: config.tables.schemas.clone(),
                public_keys: config.tables.public_keys.clone(),
                native_index: config.tables.native_index.clone(),
                process: config.tables.process.clone(),
                logs: config.tables.logs.clone(),
                idempotency: config.tables.idempotency.clone(),
            },
        })),
        DatabaseConfig::Exemem { api_url, .. } => DatabaseConfigDto::Exemem {
            api_url: api_url.clone(),
        },
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
    // Use the node's unique public key from config (set per-installation)
    let config = state.node_manager.get_base_config().await;

    let public_key = match &config.public_key {
        Some(pk) if !pk.is_empty() => pk.clone(),
        _ => {
            return HttpResponse::InternalServerError().json(json!({
                "ok": false,
                "error": "No node public key configured. The server identity has not been initialized."
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

/// Persist a NodeConfig to disk (same path the server loaded from)
fn persist_node_config(config: &NodeConfig) -> Result<(), String> {
    let config_path =
        std::env::var("NODE_CONFIG").unwrap_or_else(|_| "config/node_config.json".to_string());

    // Ensure config directory exists
    if let Some(parent) = std::path::Path::new(&config_path).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create config directory: {}", e))?;
    }

    let config_json = serde_json::to_string_pretty(config)
        .map_err(|e| format!("Failed to serialize config: {}", e))?;

    std::fs::write(&config_path, config_json)
        .map_err(|e| format!("Failed to write config file: {}", e))?;

    Ok(())
}

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
                config.database = DatabaseConfig::Local {
                    path: std::path::PathBuf::from(path),
                };
                changes.push("storage (local)");
            }
            StorageSetup::Exemem { api_url, api_key } => {
                config.database = DatabaseConfig::Exemem {
                    api_url: api_url.clone(),
                    api_key: api_key.clone(),
                    session_token: None,
                    user_hash: None,
                };
                changes.push("storage (exemem)");
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
        log_feature!(
            LogFeature::HttpServer,
            error,
            "Failed to persist setup config: {}",
            e
        );
        return HttpResponse::InternalServerError().json(SetupResponse {
            success: false,
            message: format!("Failed to save configuration: {}", e),
        });
    }

    // Update NodeManager config and invalidate all cached nodes
    let new_manager_config = NodeManagerConfig {
        base_config: config,
    };
    state.node_manager.update_config(new_manager_config).await;

    let message = format!("Setup applied: {}", changes.join(", "));
    log_feature!(LogFeature::HttpServer, info, "{}", message);

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
    let config_path =
        std::env::var("NODE_CONFIG").unwrap_or_else(|_| "config/node_config.json".to_string());
    let has_saved_config = Path::new(&config_path).exists();

    let initialized = if state.node_manager.has_active_node().await {
        true
    } else if has_saved_config {
        // For returning users, try to auto-initialize from saved config
        let config = state.node_manager.get_base_config().await;
        match &config.public_key {
            Some(pk) if !pk.is_empty() => {
                let user_hash = user_hash_from_pubkey(pk);
                state
                    .node_manager
                    .get_node(&user_hash)
                    .await
                    .inspect_err(|e| {
                        log_feature!(
                            LogFeature::HttpServer,
                            warn,
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

    HttpResponse::Ok().json(DatabaseStatusResponse {
        initialized,
        has_saved_config,
    })
}
