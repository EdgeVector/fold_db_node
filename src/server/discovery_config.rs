//! Discovery service configuration resolution.
//!
//! Discovery is enabled when the node has registered with Exemem and has an
//! identity keypair. This module centralises the resolution logic so it does
//! not depend on mutating process-wide environment variables.
//!
//! Resolution order (first match wins):
//! 1. `DISCOVERY_SERVICE_URL` + `DISCOVERY_MASTER_KEY` env vars (explicit
//!    override for tests and `run.sh`-driven local dev).
//! 2. Read the node's Ed25519 identity from the Sled `node_identity` tree
//!    (via the NodeManager-owned pool) and derive URL + master key. This
//!    is the only real resolution path post-Stage-4 — identity lives in
//!    Sled, not on NodeConfig, and there's no separate "pre-init" fallback
//!    to read from `NodeConfigStore` anymore (that field is never written).

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
        Self::from_identity(node_manager).await
    }

    fn from_env() -> Option<Self> {
        let url = std::env::var("DISCOVERY_SERVICE_URL").ok()?;
        let key_hex = std::env::var("DISCOVERY_MASTER_KEY").ok()?;
        let master_key = hex::decode(&key_hex).ok()?;
        Some(Self { url, master_key })
    }

    async fn from_identity(node_manager: &NodeManager) -> Option<Self> {
        // Identity lives in the Sled `node_identity` tree. Read it via the
        // shared NodeManager-owned pool so every consumer hits the single
        // file-lock holder — a bespoke `SledPool::new` at the same path
        // would race the NodeManager pool on the next `create_node` and
        // surface as a `WouldBlock` 500 to the client.
        let pool = node_manager.get_or_init_sled_pool().await;
        let id = crate::identity::load(pool).ok().flatten()?;
        if id.private_key.is_empty() {
            return None;
        }
        let url = format!("{}/api", crate::endpoints::exemem_api_url());
        let master_key = Sha256::digest(id.private_key.as_bytes()).to_vec();
        Some(Self { url, master_key })
    }
}
