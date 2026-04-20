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
use fold_db::storage::SledPool;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

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
///
/// FoldNode is `Sync` (all mutation happens through interior mutability in its
/// own sub-locks). The outer wrapper only needs to (a) lazily create the node
/// on first access and (b) allow invalidation for reconfiguration. A single
/// `RwLock<Option<Arc<FoldNode>>>` covers both: concurrent reads on the hot
/// path, writes only for init/invalidate. No per-request `RwLock<FoldNode>`.
pub struct NodeManager {
    /// Configuration for creating the node (wrapped in RwLock for live reconfiguration)
    config: RwLock<NodeManagerConfig>,
    /// Shared node slot. `None` until first request creates it.
    shared_node: RwLock<Option<Arc<FoldNode>>>,
    /// Shared SledPool that survives node invalidations at the same storage path.
    /// Two pools pointing at the same path cannot both hold Sled's file lock at
    /// once, so reusing one pool across `create_node`/`invalidate_all_nodes`
    /// cycles prevents a `WouldBlock` race when a client hits the server
    /// immediately after `update_config` (e.g. `/api/sync/status` right after
    /// `/api/auth/register` activates cloud sync).
    shared_pool: RwLock<Option<(PathBuf, Arc<SledPool>)>>,
}

impl NodeManager {
    /// Create a new NodeManager
    pub fn new(config: NodeManagerConfig) -> Self {
        Self {
            config: RwLock::new(config),
            shared_node: RwLock::new(None),
            shared_pool: RwLock::new(None),
        }
    }

    /// Get the node, creating one if it doesn't exist.
    ///
    /// Fast path: concurrent read lock clones the cached `Arc`.
    /// Slow path: upgrades to a write lock to create the node. The
    /// double-check under the write lock prevents racing creators from
    /// building two nodes.
    pub async fn get_node(&self, user_id: &str) -> Result<Arc<FoldNode>, NodeManagerError> {
        // Fast path: concurrent readers, uncontended once cached.
        if let Some(node) = self.shared_node.read().await.as_ref() {
            return Ok(node.clone());
        }

        // Slow path: serialize creators.
        let mut slot = self.shared_node.write().await;
        if let Some(node) = slot.as_ref() {
            // Another creator won the race.
            return Ok(node.clone());
        }

        let node = self.create_node(user_id).await?;
        *slot = Some(node.clone());
        Ok(node)
    }

    /// Create a new node instance for a user
    async fn create_node(&self, user_id: &str) -> Result<Arc<FoldNode>, NodeManagerError> {
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

        // E2E encryption keys — unified identity: derived from the node's
        // Ed25519 seed. In the pre-signup state (no identity yet) we use an
        // ephemeral in-memory random key that will be replaced once the user
        // creates an identity and the node is re-initialized.
        let e2e_keys = if let Some(ref priv_key) = node_config.private_key {
            let seed = crate::fold_node::FoldNode::extract_ed25519_seed(priv_key).map_err(|e| {
                NodeManagerError::ConfigurationError(format!("Failed to extract seed: {}", e))
            })?;
            fold_db::crypto::E2eKeys::from_ed25519_seed(&seed).map_err(|e| {
                NodeManagerError::ConfigurationError(format!("Failed to derive E2E keys: {}", e))
            })?
        } else {
            // Pre-signup: ephemeral, in-memory only — never persisted.
            let mut secret = [0u8; 32];
            rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut secret);
            fold_db::crypto::E2eKeys::from_secret(&secret)
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

        // Reuse the cached SledPool if one exists for the same path. This is
        // what prevents a WouldBlock race when a node is recreated at the same
        // path (e.g. after cloud-sync activation during /api/auth/register).
        let pool = self.get_or_create_pool(&node_config.database.path).await;

        // Create FoldDB with user context set
        let db = fold_db::logging::core::run_with_user(user_id, async {
            factory::create_fold_db_with_pool_and_auth_refresh(
                &node_config.database,
                &e2e_keys,
                auth_refresh,
                Some(pool),
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

        Ok(Arc::new(node))
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
    ///
    /// The shared `SledPool` is intentionally preserved so that the next
    /// `create_node` reopens Sled through the same pool instead of racing the
    /// old pool for the file lock.
    pub async fn invalidate_all_nodes(&self) {
        let mut shared = self.shared_node.write().await;
        *shared = None;
    }

    /// Update the configuration and invalidate the cached node.
    /// The next request will create a fresh node with the new config.
    ///
    /// If the storage path changed, the cached pool is dropped too so the new
    /// path gets its own pool. Otherwise the pool survives for the reason in
    /// [`Self::invalidate_all_nodes`].
    pub async fn update_config(&self, new_config: NodeManagerConfig) {
        let new_path = new_config.base_config.database.path.clone();
        {
            let mut config = self.config.write().await;
            *config = new_config;
        }

        {
            let mut pool_slot = self.shared_pool.write().await;
            if let Some((cached_path, _)) = pool_slot.as_ref() {
                if cached_path != &new_path {
                    *pool_slot = None;
                }
            }
        }

        self.invalidate_all_nodes().await;
    }

    /// Get the cached SledPool for `path`, creating and storing a new one if
    /// none exists (or if a pool for a different path was cached).
    async fn get_or_create_pool(&self, path: &std::path::Path) -> Arc<SledPool> {
        {
            let slot = self.shared_pool.read().await;
            if let Some((cached_path, pool)) = slot.as_ref() {
                if cached_path == path {
                    return Arc::clone(pool);
                }
            }
        }

        let mut slot = self.shared_pool.write().await;
        if let Some((cached_path, pool)) = slot.as_ref() {
            if cached_path == path {
                return Arc::clone(pool);
            }
        }
        let pool = Arc::new(SledPool::new(path.to_path_buf()));
        pool.start_idle_reaper(std::time::Duration::from_secs(30));
        *slot = Some((path.to_path_buf(), Arc::clone(&pool)));
        pool
    }

    /// Set a pre-existing node.
    /// This is useful for embedded scenarios where the node is created externally.
    pub async fn set_node(&self, _user_id: &str, node: FoldNode) {
        let node_arc = Arc::new(node);
        let mut shared = self.shared_node.write().await;
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

    /// Get a clone of the shared SledPool.
    ///
    /// Returns the NodeManager-owned pool first (which survives node
    /// invalidations and reuses the same Sled file-lock holder). Falls back to
    /// the loaded node's pool for legacy paths that set the node directly via
    /// [`Self::set_node`] (tests / embedded scenarios).
    ///
    /// Returns None if no pool has been created yet.
    pub async fn get_sled_pool(&self) -> Option<std::sync::Arc<fold_db::storage::SledPool>> {
        {
            let slot = self.shared_pool.read().await;
            if let Some((_, pool)) = slot.as_ref() {
                return Some(Arc::clone(pool));
            }
        }
        let shared = self.shared_node.read().await;
        if let Some(node) = shared.as_ref() {
            if let Ok(fold_db) = node.get_fold_db() {
                return fold_db.sled_pool().cloned();
            }
        }
        None
    }

    /// Check if an active node exists.
    pub async fn has_active_node(&self) -> bool {
        let shared = self.shared_node.read().await;
        shared.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fold_node::config::NodeConfig;

    fn test_config(path: &std::path::Path) -> NodeManagerConfig {
        let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
        let base_config = NodeConfig::new(path.to_path_buf())
            .with_schema_service_url("test://mock")
            .with_identity(&keypair.public_key_base64(), &keypair.secret_key_base64());
        NodeManagerConfig { base_config }
    }

    /// Regression test for the register -> sync-status Sled lock race.
    ///
    /// Before the fix, `invalidate_all_nodes` dropped the NodeManager's
    /// reference to the node (and thus its SledPool). The next `get_node`
    /// would call the factory which created a fresh `SledPool`, and Sled
    /// would fail with `WouldBlock` because the previous pool still held
    /// the OS file lock (e.g. via a pending background bootstrap task).
    ///
    /// After the fix, NodeManager owns the pool and passes it into the
    /// factory on every `create_node`, so the same Sled file-lock holder
    /// is reused and the second `get_node` succeeds immediately.
    #[tokio::test]
    async fn node_recreation_after_invalidate_reuses_pool() {
        let tmp = tempfile::TempDir::new().unwrap();
        let manager = NodeManager::new(test_config(tmp.path()));

        let user_hash = "test_user";

        let first = manager.get_node(user_hash).await.unwrap();
        let pool_before = manager.get_sled_pool().await.expect("pool must exist");

        // Simulate the register flow: invalidate while something else still
        // holds a reference to the node (and its SledPool).
        let _holder = first.clone();
        manager.invalidate_all_nodes().await;

        // Immediately recreate the node — this must succeed without a
        // WouldBlock / "could not acquire lock" error.
        let second = manager
            .get_node(user_hash)
            .await
            .expect("recreation must not fail on Sled file lock");
        let pool_after = manager.get_sled_pool().await.expect("pool must exist");

        // Same pool instance — reused, not recreated.
        assert!(Arc::ptr_eq(&pool_before, &pool_after));
        // Different FoldNode instances — invalidation did replace the node.
        assert!(!Arc::ptr_eq(&first, &second));
    }

    /// Rapid-fire invalidate/recreate cycles must all succeed. Mirrors the
    /// register flow where a background bootstrap keeps the pool busy while
    /// the foreground path serves follow-up requests.
    #[tokio::test]
    async fn rapid_invalidate_recreate_cycles_succeed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let manager = NodeManager::new(test_config(tmp.path()));
        let user_hash = "test_user";

        let mut holders = Vec::new();
        for _ in 0..5 {
            let node = manager.get_node(user_hash).await.expect("get_node");
            holders.push(node);
            manager.invalidate_all_nodes().await;
        }

        // One pool across the entire cycle.
        assert!(manager.get_sled_pool().await.is_some());
    }

    /// Changing the storage path must drop the cached pool so the new path
    /// gets its own lock holder.
    #[tokio::test]
    async fn update_config_with_new_path_drops_pool() {
        let tmp = tempfile::TempDir::new().unwrap();
        let manager = NodeManager::new(test_config(&tmp.path().join("a")));
        let user_hash = "test_user";

        let _ = manager.get_node(user_hash).await.unwrap();
        let pool_a = manager.get_sled_pool().await.unwrap();

        // Update config to a different path.
        manager
            .update_config(test_config(&tmp.path().join("b")))
            .await;

        let _ = manager.get_node(user_hash).await.unwrap();
        let pool_b = manager.get_sled_pool().await.unwrap();

        assert!(!Arc::ptr_eq(&pool_a, &pool_b));
    }
}
