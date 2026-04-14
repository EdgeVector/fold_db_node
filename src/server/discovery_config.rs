//! Discovery service configuration resolution.
//!
//! Discovery is enabled when the node has registered with Exemem and has an
//! identity keypair. This module centralises the resolution logic so it does
//! not depend on mutating process-wide environment variables.
//!
//! Resolution order (first match wins):
//! 1. `DISCOVERY_SERVICE_URL` + `DISCOVERY_MASTER_KEY` env vars (explicit
//!    override for tests and `run.sh`-driven local dev).
//! 2. The node's current `NodeManager` base config — derives the URL from
//!    `endpoints::exemem_api_url()` and the master key from
//!    `SHA256(private_key)`.
//! 3. If no base config is loaded yet, the on-disk Sled `NodeConfigStore`
//!    under `folddb_home/data` (the case where the user registered after
//!    the HTTP server booted).

use sha2::{Digest, Sha256};

use crate::server::node_manager::NodeManager;

/// Resolved discovery configuration — always a fully-populated pair.
#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    pub url: String,
    pub master_key: Vec<u8>,
}

impl DiscoveryConfig {
    /// Attempt to resolve the discovery configuration for the given node
    /// manager. Returns `None` when the node has not been registered /
    /// given an identity yet.
    pub async fn resolve(node_manager: &NodeManager) -> Option<Self> {
        if let Some(cfg) = Self::from_env() {
            return Some(cfg);
        }

        if let Some(cfg) = Self::from_base_config(node_manager).await {
            return Some(cfg);
        }

        Self::from_sled_fallback()
    }

    fn from_env() -> Option<Self> {
        let url = std::env::var("DISCOVERY_SERVICE_URL").ok()?;
        let key_hex = std::env::var("DISCOVERY_MASTER_KEY").ok()?;
        let master_key = hex::decode(&key_hex).ok()?;
        Some(Self { url, master_key })
    }

    async fn from_base_config(node_manager: &NodeManager) -> Option<Self> {
        let base = node_manager.get_base_config().await;
        let priv_key_b64 = base.private_key.as_ref()?;
        if priv_key_b64.is_empty() {
            return None;
        }
        let url = format!("{}/api", crate::endpoints::exemem_api_url());
        let master_key = Sha256::digest(priv_key_b64.as_bytes()).to_vec();
        Some(Self { url, master_key })
    }

    fn from_sled_fallback() -> Option<Self> {
        let data_path = crate::utils::paths::folddb_home()
            .ok()
            .map(|h| h.join("data"))
            .or_else(|| {
                std::env::var("FOLD_STORAGE_PATH")
                    .ok()
                    .map(std::path::PathBuf::from)
            })?;

        let pool = std::sync::Arc::new(fold_db::storage::SledPool::new(data_path));
        let store = fold_db::NodeConfigStore::new(pool).ok()?;
        let cloud = store.get_cloud_config()?;

        // This fallback path does not have access to the E2E key (which is
        // derived from the identity private key we are trying to read). If
        // the stored value is encrypted we cannot decrypt it here — the
        // `NodeManager` base-config path will succeed on the next resolve
        // attempt once the node finishes initialization. Fall through to
        // None so the caller retries.
        let raw_priv = store.raw_identity_private_key()?;
        if raw_priv.starts_with("ENC:") {
            log::debug!("discovery: Sled identity is encrypted; deferring to base-config path");
            return None;
        }
        let url = format!("{}/api", cloud.api_url);
        let master_key = Sha256::digest(raw_priv.as_bytes()).to_vec();
        Some(Self { url, master_key })
    }
}
