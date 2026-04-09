//! Multi-tenant Node Manager
//!
//! Manages FoldDB nodes for different tenants, caching them for reuse.
//! This enables lazy node initialization - nodes are only created when
//! a user makes their first request, avoiding DynamoDB access during startup.
//!
//! # Storage Mode Behavior
//!
//! - **Cloud mode (DynamoDB)**: Creates separate nodes per user with user_id isolation
//! - **Local mode (Sled)**: Shares a single node across all users (single-tenant)
//!   This avoids Sled lock conflicts since only one process can hold the lock.

use crate::fold_node::config::NodeConfig;
use crate::fold_node::FoldNode;
use crate::utils::crypto::user_hash_from_pubkey;
use base64::Engine as _;
use fold_db::fold_db_core::factory;
use fold_db::security::Ed25519KeyPair;
use fold_db::storage::config::DatabaseConfig;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
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

/// Manages FoldDB nodes for different tenants
pub struct NodeManager {
    /// Configuration for creating new nodes (wrapped in RwLock for live reconfiguration)
    config: RwLock<NodeManagerConfig>,
    /// Cache of active nodes (user_id -> Node)
    nodes: Arc<Mutex<HashMap<String, Arc<RwLock<FoldNode>>>>>,
    /// Shared node for local mode (single-tenant)
    /// In local Sled mode, we share one node to avoid lock conflicts
    shared_local_node: Arc<Mutex<Option<Arc<RwLock<FoldNode>>>>>,
    /// Whether we're in local mode (wrapped in RwLock for live reconfiguration)
    is_local_mode: RwLock<bool>,
}

impl NodeManager {
    /// Create a new NodeManager
    pub fn new(config: NodeManagerConfig) -> Self {
        // Both Local and Exemem use local Sled storage (Exemem adds S3 sync on top)
        let is_local_mode = matches!(
            config.base_config.database,
            DatabaseConfig::Local { .. } | DatabaseConfig::Exemem { .. }
        );
        Self {
            config: RwLock::new(config),
            nodes: Arc::new(Mutex::new(HashMap::new())),
            shared_local_node: Arc::new(Mutex::new(None)),
            is_local_mode: RwLock::new(is_local_mode),
        }
    }

    /// Get a node for a specific user, creating one if it doesn't exist
    ///
    /// In local mode (Sled), returns a shared node for all users to avoid lock conflicts.
    /// In cloud mode (DynamoDB), creates/returns a per-user node with user_id isolation.
    pub async fn get_node(&self, user_id: &str) -> Result<Arc<RwLock<FoldNode>>, NodeManagerError> {
        // Local mode: use shared single node to avoid Sled lock conflicts
        if *self.is_local_mode.read().await {
            return self.get_shared_local_node(user_id).await;
        }

        // Cloud mode: per-user nodes with DynamoDB partition isolation
        // Check cache first
        {
            let nodes = self.nodes.lock().await;
            if let Some(node) = nodes.get(user_id) {
                return Ok(node.clone());
            }
        }

        // Create new node
        let node = self.create_node(user_id).await?;

        // Cache it
        {
            let mut nodes = self.nodes.lock().await;
            nodes.insert(user_id.to_string(), node.clone());
        }

        Ok(node)
    }

    /// Get or create the shared local node (for Sled mode)
    ///
    /// Uses a mutex to ensure only one node is ever created, avoiding race conditions
    /// where multiple concurrent requests could try to create the node simultaneously.
    async fn get_shared_local_node(
        &self,
        user_id: &str,
    ) -> Result<Arc<RwLock<FoldNode>>, NodeManagerError> {
        // Hold the lock for the entire check-and-create operation to avoid races
        let mut shared = self.shared_local_node.lock().await;

        // If we already have a shared node, return it
        if let Some(node) = shared.as_ref() {
            return Ok(node.clone());
        }

        // Create the shared node while still holding the lock
        // This ensures only one thread creates the node
        let node = self.create_node(user_id).await?;

        // Store it as the shared node
        *shared = Some(node.clone());

        Ok(node)
    }

    /// Create a new node instance for a user
    async fn create_node(&self, user_id: &str) -> Result<Arc<RwLock<FoldNode>>, NodeManagerError> {
        // Clone the base config and set user_id
        let mut node_config = self.config.read().await.base_config.clone();

        // Set user_id in database config
        match &mut node_config.database {
            #[cfg(feature = "aws-backend")]
            DatabaseConfig::Cloud(ref mut cloud_config) => {
                cloud_config.user_id = Some(user_id.to_string());
            }
            DatabaseConfig::Local { .. } | DatabaseConfig::Exemem { .. } => {
                // Local/Exemem storage doesn't need user_id in config
                // User isolation is handled differently
            }
        }

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

        // Build auth-refresh callback for Exemem mode so the sync engine can
        // automatically recover from expired tokens (401) by re-registering.
        let auth_refresh = if matches!(&node_config.database, DatabaseConfig::Exemem { .. }) {
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

    /// Invalidate (remove) a node from the cache
    /// This forces a reload/recreation on the next access
    pub async fn invalidate_node(&self, user_id: &str) {
        let mut nodes = self.nodes.lock().await;
        nodes.remove(user_id);

        // Also clear the shared local node so it gets re-created
        if *self.is_local_mode.read().await {
            let mut shared = self.shared_local_node.lock().await;
            *shared = None;
        }
    }

    /// Invalidate all cached nodes
    /// Used when configuration changes require all nodes to be recreated
    pub async fn invalidate_all_nodes(&self) {
        let mut nodes = self.nodes.lock().await;
        nodes.clear();

        let mut shared = self.shared_local_node.lock().await;
        *shared = None;
    }

    /// Update the configuration and invalidate all cached nodes
    /// The next request will create fresh nodes with the new config
    pub async fn update_config(&self, new_config: NodeManagerConfig) {
        let new_is_local = matches!(
            new_config.base_config.database,
            DatabaseConfig::Local { .. } | DatabaseConfig::Exemem { .. }
        );

        {
            let mut config = self.config.write().await;
            *config = new_config;
        }
        {
            let mut is_local = self.is_local_mode.write().await;
            *is_local = new_is_local;
        }

        self.invalidate_all_nodes().await;
    }

    /// Set a pre-existing node in the cache
    /// This is useful for embedded scenarios where the node is created externally
    pub async fn set_node(&self, user_id: &str, node: FoldNode) {
        let node_arc = Arc::new(RwLock::new(node));

        // In local mode, also set the shared_local_node so get_node finds it
        if *self.is_local_mode.read().await {
            let mut shared = self.shared_local_node.lock().await;
            *shared = Some(node_arc.clone());
        }

        let mut nodes = self.nodes.lock().await;
        nodes.insert(user_id.to_string(), node_arc);
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

    /// Get a clone of the Sled database handle from the currently loaded shared node.
    ///
    /// Sled's `Db` is internally `Arc`-wrapped, so cloning shares the same open
    /// database with no lock conflict. This is used by bootstrap-after-restore to
    /// get a Sled handle that survives node invalidation.
    ///
    /// Returns None if no node is loaded yet.
    pub async fn get_sled_db(&self) -> Option<sled::Db> {
        let shared = self.shared_local_node.lock().await;
        if let Some(node_arc) = shared.as_ref() {
            let node = node_arc.read().await;
            if let Ok(fold_db) = node.get_fold_db().await {
                return fold_db.sled_db().cloned();
            }
        }
        None
    }

    /// Check if any active node exists in the cache.
    ///
    /// In local mode, checks the shared local node.
    /// In cloud mode, checks if any per-user node has been created.
    pub async fn has_active_node(&self) -> bool {
        if *self.is_local_mode.read().await {
            let shared = self.shared_local_node.lock().await;
            shared.is_some()
        } else {
            let nodes = self.nodes.lock().await;
            !nodes.is_empty()
        }
    }
}
