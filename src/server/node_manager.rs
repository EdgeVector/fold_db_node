//! Node Manager
//!
//! Manages the FoldDB node, caching it for reuse.
//! This enables lazy node initialization - the node is only created when
//! a user makes their first request.
//!
//! FoldDB always uses local Sled storage (with optional cloud sync on top).
//! A single shared node is used for all requests to avoid Sled lock conflicts.

use crate::fold_node::config::NodeConfig;
use crate::fold_node::FoldNode;
use crate::utils::crypto::user_hash_from_pubkey;
use base64::Engine as _;
use fold_db::fold_db_core::factory;
use fold_db::security::Ed25519KeyPair;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

/// Persisted node identity keypair
#[derive(Serialize, Deserialize)]
struct NodeIdentity {
    private_key: String,
    public_key: String,
}

/// Error type for node manager operations
#[derive(Debug, thiserror::Error)]
pub enum NodeManagerError {
    #[error("Configuration error: {0}")]
    ConfigurationError(String),
    #[error("Storage error: {0}")]
    StorageError(String),
    #[error("Security error: {0}")]
    SecurityError(String),
    #[error("Node creation error: {0}")]
    NodeCreationError(String),
}

/// Configuration for creating nodes
#[derive(Clone)]
pub struct NodeManagerConfig {
    /// Base node configuration (user_id will be set per-tenant)
    pub base_config: NodeConfig,
}

/// Manages the FoldDB node instance
pub struct NodeManager {
    /// Configuration for creating the node (wrapped in RwLock for live reconfiguration)
    config: RwLock<NodeManagerConfig>,
    /// Shared node (single-tenant, avoids Sled lock conflicts)
    shared_node: Arc<Mutex<Option<Arc<RwLock<FoldNode>>>>>,
}

impl NodeManager {
    /// Create a new NodeManager
    pub fn new(config: NodeManagerConfig) -> Self {
        Self {
            config: RwLock::new(config),
            shared_node: Arc::new(Mutex::new(None)),
        }
    }

    /// Get the node, creating one if it doesn't exist.
    ///
    /// Returns a shared node for all requests. Uses a mutex to ensure only one
    /// node is ever created, avoiding race conditions where multiple concurrent
    /// requests could try to create the node simultaneously.
    pub async fn get_node(&self, user_id: &str) -> Result<Arc<RwLock<FoldNode>>, NodeManagerError> {
        let mut shared = self.shared_node.lock().await;

        if let Some(node) = shared.as_ref() {
            return Ok(node.clone());
        }

        let node = self.create_node(user_id).await?;
        *shared = Some(node.clone());

        Ok(node)
    }

    /// Create a new node instance for a user
    async fn create_node(&self, user_id: &str) -> Result<Arc<RwLock<FoldNode>>, NodeManagerError> {
        // Clone the base config and set user_id
        let mut node_config = self.config.read().await.base_config.clone();

        // DatabaseConfig is always local Sled storage; user isolation is handled differently.
        // No per-user config mutation needed.

        // Use keys from config if already set (from node_config.json or Sled).
        // Only generate a default identity if none is configured.
        if node_config.public_key.is_none() || node_config.private_key.is_none() {
            let keypair = Self::load_or_generate_identity("default").await?;
            node_config = node_config
                .with_identity(&keypair.public_key_base64(), &keypair.secret_key_base64());
        }

        // Load E2E encryption keys — derived from identity if no legacy e2e.key exists
        let e2e_keys = {
            let folddb_home = crate::utils::paths::folddb_home().map_err(|e| {
                NodeManagerError::ConfigurationError(format!("Cannot resolve FOLDDB_HOME: {e}"))
            })?;
            let e2e_key_path = folddb_home.join("e2e.key");

            if e2e_key_path.exists() {
                // Legacy: use existing e2e.key file
                fold_db::crypto::E2eKeys::load_or_generate(&e2e_key_path)
                    .await
                    .map_err(|e| {
                        NodeManagerError::ConfigurationError(format!(
                            "Failed to load E2E keys: {}",
                            e
                        ))
                    })?
            } else if let Some(ref priv_key) = node_config.private_key {
                // Derive from identity — one key for everything
                let seed =
                    crate::fold_node::FoldNode::extract_ed25519_seed(priv_key).map_err(|e| {
                        NodeManagerError::ConfigurationError(format!(
                            "Failed to extract seed: {}",
                            e
                        ))
                    })?;
                fold_db::crypto::E2eKeys::from_ed25519_seed(&seed).map_err(|e| {
                    NodeManagerError::ConfigurationError(format!(
                        "Failed to derive E2E keys: {}",
                        e
                    ))
                })?
            } else {
                // No identity yet — generate random key (pre-signup state)
                fold_db::crypto::E2eKeys::load_or_generate(&e2e_key_path)
                    .await
                    .map_err(|e| {
                        NodeManagerError::ConfigurationError(format!(
                            "Failed to load E2E keys: {}",
                            e
                        ))
                    })?
            }
        };

        // Ensure the Exemem factory can find the correct storage path.
        // The factory reads FOLD_STORAGE_PATH to locate the Sled database;
        // without this, it falls back to a relative "data" path which causes
        // lock conflicts in multi-node setups.
        std::env::set_var("FOLD_STORAGE_PATH", node_config.get_storage_path());

        // Inject per-device credentials from credentials.json into the DatabaseConfig.
        // credentials.json is the single source of truth for api_key and session_token.
        // The config file may have a stale key; Sled must not store per-device secrets.
        if node_config.database.has_cloud_sync() {
            if let Ok(Some(creds)) = crate::keychain::load_credentials() {
                if let Some(ref mut cloud) = node_config.database.cloud_sync {
                    cloud.api_key = creds.api_key;
                    if !creds.session_token.is_empty() {
                        cloud.session_token = Some(creds.session_token);
                    }
                }
            }
        }

        // Build auth-refresh callback for Exemem mode so the sync engine can
        // automatically recover from expired tokens (401) by re-registering.
        let auth_refresh = if node_config.database.has_cloud_sync() {
            Some(crate::server::routes::auth::build_auth_refresh_callback())
        } else {
            None
        };

        // Create FoldDB with user context set
        let db = fold_db::logging::core::run_with_user(user_id, async {
            factory::create_fold_db_with_auth_refresh(
                &node_config.database,
                &e2e_keys,
                auth_refresh,
            )
            .await
        })
        .await
        .map_err(|e| NodeManagerError::StorageError(e.to_string()))?;

        // Create FoldDB node with user context set
        let node = fold_db::logging::core::run_with_user(user_id, async {
            FoldNode::new_with_db(node_config, db).await
        })
        .await
        .map_err(|e| NodeManagerError::NodeCreationError(e.to_string()))?;

        Ok(Arc::new(RwLock::new(node)))
    }

    /// Load an existing identity keypair from disk, or generate a new random one.
    ///
    /// Key file path: `~/.fold_db/identity/{sha256(user_id)}.json`
    /// The SHA-256 hash is used as the filename to avoid path injection from arbitrary user_ids.
    async fn load_or_generate_identity(user_id: &str) -> Result<Ed25519KeyPair, NodeManagerError> {
        // Build the key file path: $FOLDDB_HOME/identity/{hash}.json
        let folddb_home = crate::utils::paths::folddb_home().map_err(|e| {
            NodeManagerError::ConfigurationError(format!("Cannot resolve FOLDDB_HOME: {e}"))
        })?;

        let mut hasher = Sha256::new();
        hasher.update(user_id.as_bytes());
        let hash_hex = format!("{:x}", hasher.finalize());

        let identity_dir = folddb_home.join("identity");
        let identity_path = identity_dir.join(format!("{hash_hex}.json"));

        if identity_path.exists() {
            // Load existing keypair
            let content = tokio::fs::read_to_string(&identity_path)
                .await
                .map_err(|e| {
                    NodeManagerError::SecurityError(format!("Failed to read identity file: {e}"))
                })?;

            let identity: NodeIdentity = serde_json::from_str(&content).map_err(|e| {
                NodeManagerError::SecurityError(format!("Invalid identity file: {e}"))
            })?;

            let secret_bytes = base64::engine::general_purpose::STANDARD
                .decode(&identity.private_key)
                .map_err(|e| {
                    NodeManagerError::SecurityError(format!("Invalid private key encoding: {e}"))
                })?;

            Ed25519KeyPair::from_secret_key(&secret_bytes)
                .map_err(|e| NodeManagerError::SecurityError(e.to_string()))
        } else {
            // Generate a new random keypair
            let keypair = Ed25519KeyPair::generate()
                .map_err(|e| NodeManagerError::SecurityError(e.to_string()))?;

            let identity = NodeIdentity {
                private_key: keypair.secret_key_base64(),
                public_key: keypair.public_key_base64(),
            };

            // Ensure directory exists
            tokio::fs::create_dir_all(&identity_dir)
                .await
                .map_err(|e| {
                    NodeManagerError::SecurityError(format!(
                        "Failed to create identity directory: {e}"
                    ))
                })?;

            // Write the identity file
            let content = serde_json::to_string_pretty(&identity).map_err(|e| {
                NodeManagerError::SecurityError(format!("Failed to serialize identity: {e}"))
            })?;
            tokio::fs::write(&identity_path, &content)
                .await
                .map_err(|e| {
                    NodeManagerError::SecurityError(format!("Failed to write identity file: {e}"))
                })?;

            // Restrict permissions to owner-only (Unix)
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = std::fs::Permissions::from_mode(0o600);
                std::fs::set_permissions(&identity_path, perms).map_err(|e| {
                    NodeManagerError::SecurityError(format!(
                        "Failed to set identity file permissions: {e}"
                    ))
                })?;
            }

            log::info!("Generated new node identity at {}", identity_path.display());

            Ok(keypair)
        }
    }

    /// Invalidate the cached node, forcing recreation on next access.
    /// Used when configuration changes require the node to be recreated.
    pub async fn invalidate_all_nodes(&self) {
        let mut shared = self.shared_node.lock().await;
        *shared = None;
    }

    /// Update the configuration and invalidate the cached node.
    /// The next request will create a fresh node with the new config.
    pub async fn update_config(&self, new_config: NodeManagerConfig) {
        {
            let mut config = self.config.write().await;
            *config = new_config;
        }

        self.invalidate_all_nodes().await;
    }

    /// Set a pre-existing node.
    /// This is useful for embedded scenarios where the node is created externally.
    pub async fn set_node(&self, _user_id: &str, node: FoldNode) {
        let node_arc = Arc::new(RwLock::new(node));
        let mut shared = self.shared_node.lock().await;
        *shared = Some(node_arc);
    }

    /// Get the base configuration (returns a clone since config is behind RwLock)
    pub async fn get_base_config(&self) -> NodeConfig {
        self.config.read().await.base_config.clone()
    }

    /// Ensure a default identity exists and return its public key.
    ///
    /// On first call this generates a keypair (persisted to disk) and eagerly
    /// creates the local-mode node so subsequent authenticated requests are fast.
    /// Returns the base-64 public key string.
    pub async fn ensure_default_identity(&self) -> Result<String, NodeManagerError> {
        // Fast path: identity already populated in the base config
        {
            let config = self.config.read().await;
            if let Some(pk) = &config.base_config.public_key {
                if !pk.is_empty() {
                    return Ok(pk.clone());
                }
            }
        }

        // Generate (or load) an identity for the "default" local user
        let keypair = Self::load_or_generate_identity("default").await?;
        let public_key = keypair.public_key_base64();

        // Persist the identity into the base config so future reads find it
        {
            let mut config = self.config.write().await;
            config.base_config = config
                .base_config
                .clone()
                .with_identity(&public_key, &keypair.secret_key_base64());
        }

        // Eagerly create the node so the first authenticated request doesn't block
        let user_hash = user_hash_from_pubkey(&public_key);
        let _ = self.get_node(&user_hash).await;

        Ok(public_key)
    }

    /// Get a clone of the SledPool from the currently loaded node.
    ///
    /// The pool is `Arc`-wrapped, so cloning shares the same pool instance.
    /// This is used by bootstrap-after-restore to get a Sled handle that
    /// survives node invalidation.
    ///
    /// Returns None if no node is loaded yet.
    pub async fn get_sled_pool(&self) -> Option<std::sync::Arc<fold_db::storage::SledPool>> {
        let shared = self.shared_node.lock().await;
        if let Some(node_arc) = shared.as_ref() {
            let node = node_arc.read().await;
            if let Ok(fold_db) = node.get_fold_db() {
                return fold_db.sled_pool().cloned();
            }
        }
        None
    }

    /// Check if an active node exists.
    pub async fn has_active_node(&self) -> bool {
        let shared = self.shared_node.lock().await;
        shared.is_some()
    }
}
