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
use crate::identity;
use crate::utils::crypto::user_hash_from_pubkey;
use fold_db::fold_db_core::factory;
use fold_db::storage::SledPool;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

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

    /// Create a new node instance for a user.
    ///
    /// Identity handling lives entirely inside [`FoldNode::new_with_db`] —
    /// it reads / generates the keypair from the `node_identity` Sled
    /// tree using the same pool we hand to the FoldDB factory. No
    /// separate identity files, no pre-load here.
    async fn create_node(&self, user_id: &str) -> Result<Arc<FoldNode>, NodeManagerError> {
        let mut node_config = self.config.read().await.base_config.clone();

        // Ensure the Exemem factory can find the correct storage path.
        // The factory reads FOLD_STORAGE_PATH to locate the Sled database;
        // without this, it falls back to a relative "data" path which causes
        // lock conflicts in multi-node setups.
        std::env::set_var("FOLD_STORAGE_PATH", node_config.get_storage_path());

        // Inject per-device credentials from credentials.json into the
        // DatabaseConfig. credentials.json is the single source of truth for
        // api_key and session_token. Sled must not store per-device secrets.
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

        // Shared pool that survives node invalidations at the same path —
        // prevents the WouldBlock race when a node is recreated after a
        // cloud-sync activation mid-register.
        let pool = self.get_or_create_pool(&node_config.database.path).await;

        // Resolve identity up-front against the shared pool so we have the
        // E2E seed available for FoldDB initialization. This is the single
        // identity read per create_node — the same pool threads through to
        // FoldDB below, so no double file-lock holder.
        let id = Arc::new(
            identity::load_or_generate(Arc::clone(&pool))
                .map_err(NodeManagerError::SecurityError)?,
        );
        let seed = FoldNode::extract_ed25519_seed(&id.private_key).map_err(|e| {
            NodeManagerError::ConfigurationError(format!("Failed to extract seed: {}", e))
        })?;
        let e2e_keys = fold_db::crypto::E2eKeys::from_ed25519_seed(&seed).map_err(|e| {
            NodeManagerError::ConfigurationError(format!("Failed to derive E2E keys: {}", e))
        })?;

        // The auth-refresh callback captures the identity Arc so the sync
        // engine can re-sign register requests on 401 without re-opening
        // the Sled identity tree (we're holding its pool for writes).
        let auth_refresh =
            crate::handlers::auth::auth_refresh_for(&node_config.database, Arc::clone(&id));

        // Load the persistent signing keypair from the node identity so
        // molecule signatures match the node's public key — see the
        // factory docs for why this must come from the persistent identity.
        let signer =
            identity::signer_from_identity(&id).map_err(NodeManagerError::SecurityError)?;

        let db = fold_db::logging::core::run_with_user(user_id, async {
            factory::create_fold_db_with_pool_and_auth_refresh(
                &node_config.database,
                &e2e_keys,
                signer,
                auth_refresh,
                Some(pool),
            )
            .await
        })
        .await
        .map_err(|e| NodeManagerError::StorageError(e.to_string()))?;

        let node = fold_db::logging::core::run_with_user(user_id, async {
            FoldNode::new_with_db(node_config, db).await
        })
        .await
        .map_err(|e| NodeManagerError::NodeCreationError(e.to_string()))?;

        Ok(Arc::new(node))
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
    ///
    /// The `shared_node` write lock is held across the config update and the
    /// invalidation so no concurrent `get_node` fast-path can observe the new
    /// config while still returning a node that was built from the old config.
    /// Any reader blocks until both updates are visible together.
    pub async fn update_config(&self, new_config: NodeManagerConfig) {
        let new_path = new_config.base_config.database.path.clone();

        // Acquire the node slot first so fast-path readers block until config
        // AND the cached node are updated atomically together.
        let mut shared = self.shared_node.write().await;

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

        *shared = None;
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
    /// Reads (or creates + persists) the keypair from the Sled identity
    /// tree, then eagerly builds the node so (a) the first authenticated
    /// request doesn't block on Sled open, and (b) `get_sled_pool()`
    /// immediately returns the live pool — callers like `restore_from_phrase`
    /// rely on the pool being `Some` right after this call.
    pub async fn ensure_default_identity(&self) -> Result<String, NodeManagerError> {
        let pool = self.get_or_init_sled_pool().await;
        let id = identity::load_or_generate(pool).map_err(NodeManagerError::SecurityError)?;
        let user_hash = user_hash_from_pubkey(&id.public_key);
        let _ = self.get_node(&user_hash).await;
        Ok(id.public_key)
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

    /// Get the shared SledPool, lazily creating it at the base config's
    /// `database.path` if one does not yet exist.
    ///
    /// This is the only supported way for non-FoldNode code (discovery
    /// resolver, identity-card writer, snapshot uploader, etc.) to touch
    /// Sled. It guarantees a **single** `sled::Db` file-lock holder per
    /// database path for the lifetime of the process — bespoke
    /// `SledPool::new(path)` call sites at the same path race this pool
    /// for the OS file lock and produce `WouldBlock` errors during the
    /// register → org-create window.
    pub async fn get_or_init_sled_pool(&self) -> Arc<SledPool> {
        let path = self.config.read().await.base_config.database.path.clone();
        self.get_or_create_pool(&path).await
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
            .with_seed_identity(crate::identity::identity_from_keypair(&keypair));
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

    /// Regression test for the register → org-create Sled lock race
    /// (fd7c5): `get_or_init_sled_pool` must return **the same pool**
    /// that a subsequent `create_node` would pick up, so helper code
    /// (discovery resolver, identity-card writer, snapshot uploader)
    /// cannot open a second `sled::Db` at the same path.
    ///
    /// Before the fix, `DiscoveryConfig::from_sled_fallback`,
    /// `set_identity_card`, and `migrate_to_cloud` all called
    /// `SledPool::new(data_path)` directly. Any one of those pools holding
    /// the OS flock caused the next `create_node` (triggered by the first
    /// `POST /api/org` after `POST /api/auth/register`) to fail with
    /// `WouldBlock` inside `NodeConfigStore::with_crypto_key`.
    #[tokio::test]
    async fn get_or_init_sled_pool_returns_cached_pool() {
        let tmp = tempfile::TempDir::new().unwrap();
        let manager = NodeManager::new(test_config(tmp.path()));

        // Helper lazily creates and caches the pool.
        let pool_helper = manager.get_or_init_sled_pool().await;

        // `create_node` must see the exact same Arc — not a fresh pool at
        // the same path (which would race for the file lock).
        let _node = manager.get_node("test_user").await.unwrap();
        let pool_after_node = manager.get_sled_pool().await.expect("pool must exist");

        assert!(
            Arc::ptr_eq(&pool_helper, &pool_after_node),
            "helper pool and node pool must be the same Arc — otherwise \
             two sled::Db instances fight for the OS flock"
        );
    }

    /// Regression test for fd7c5: simulate the register → org-create
    /// race. A first `get_node` boots the node (mirroring register's
    /// `ensure_default_identity`). A helper then borrows the shared pool
    /// (mirroring `DiscoveryConfig::from_sled_fallback` or any other
    /// caller that previously opened a bespoke pool). `invalidate_all`
    /// runs. The second `get_node` (mirroring org-create) must succeed
    /// — the shared pool is the only flock holder.
    #[tokio::test]
    async fn register_then_org_create_race_does_not_deadlock() {
        let tmp = tempfile::TempDir::new().unwrap();
        let manager = NodeManager::new(test_config(tmp.path()));

        // Register path: first get_node creates node + pool.
        let first = manager.get_node("test_user").await.unwrap();

        // Helper path (discovery / identity card / snapshot): grab the
        // shared pool via the sanctioned API and hold it open — mirrors
        // the DiscoveryConfig::resolve fallback firing during the
        // register→org-create window.
        let helper_pool = manager.get_or_init_sled_pool().await;
        let _helper_guard = helper_pool
            .acquire_arc()
            .expect("helper must be able to acquire the shared pool");

        // Simulate the `enable_cloud_sync_in_config` invalidation.
        manager.invalidate_all_nodes().await;

        // Org-create path: recreating the node while the helper still
        // holds a guard on the shared pool must succeed — no WouldBlock
        // 500 to the client.
        let second = manager
            .get_node("test_user")
            .await
            .expect("create_node must not fail on Sled flock race");

        assert!(!Arc::ptr_eq(&first, &second));
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
