//! Background poller for the messaging bulletin board.
//!
//! Without this, encrypted messages posted to the bulletin board (connection
//! requests, query requests/responses, schema list requests/responses, data
//! shares, referrals) are only fetched when the user opens the Connections
//! screen. Async inter-node queries therefore only complete "when a human
//! looks at them", which violates the product requirement that responses
//! arrive autonomously.
//!
//! This poller runs on a fixed cadence (15s), resolves discovery config from
//! the shared node, and delegates to `handlers::discovery::poll_and_dispatch`.
//! If discovery is not configured yet (no invite code / pre-signup), each
//! tick is a no-op until config becomes available.

use crate::handlers::discovery as discovery_handlers;
use crate::server::node_manager::NodeManager;
use std::sync::Arc;
use std::time::Duration;

/// How often the poller runs. Matches the cadence used elsewhere for bulletin
/// board polling — short enough to feel responsive, long enough not to DOS
/// the messaging service.
const POLL_INTERVAL: Duration = Duration::from_secs(15);

/// Spawn the background poller task. Returns the JoinHandle so the caller
/// can choose to await it on shutdown; in practice the server just lets it
/// die with the tokio runtime.
pub fn spawn(node_manager: Arc<NodeManager>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(POLL_INTERVAL);
        // Skip the immediate tick so we don't race server startup.
        interval.tick().await;

        loop {
            interval.tick().await;

            if let Err(e) = tick(&node_manager).await {
                log::warn!("Background poller tick failed: {e}");
            }
        }
    })
}

/// One iteration of the poller: resolve config, lock the node, dispatch.
///
/// Returns `Ok(())` on both "success" and "config not available" (a pre-signup
/// node shouldn't spam logs — we silently skip until it has discovery config).
async fn tick(node_manager: &Arc<NodeManager>) -> Result<(), String> {
    if !node_manager.has_active_node().await {
        // No node yet — nothing to poll against.
        return Ok(());
    }

    // Resolve discovery config the same way the UI routes do: env vars or
    // derived from Sled config store.
    let (discovery_url, master_key) = match resolve_discovery_config() {
        Some(c) => c,
        None => return Ok(()), // Discovery not configured — skip silently
    };

    let auth_token = match resolve_auth_token() {
        Some(t) => t,
        None => return Ok(()), // No auth token yet — skip silently
    };

    // Grab the shared node. NodeManager is single-tenant and already has a
    // cached shared_node at this point (has_active_node() check above); any
    // user_id string routes to the same instance, but we use the public key
    // hash so the lazy path matches what auth routes do.
    let base_config = node_manager.get_base_config().await;
    let public_key = match base_config.public_key.as_ref() {
        Some(pk) if !pk.is_empty() => pk.clone(),
        _ => return Ok(()), // Pre-signup — nothing to poll
    };
    let user_hash = crate::utils::crypto::user_hash_from_pubkey(&public_key);

    let node_arc = node_manager
        .get_node(&user_hash)
        .await
        .map_err(|e| format!("Failed to get node: {e}"))?;
    let node = node_arc.read().await;

    discovery_handlers::poll_and_dispatch(&node, &discovery_url, &auth_token, &master_key)
        .await
        .map_err(|e| format!("poll_and_dispatch failed: {e}"))?;

    Ok(())
}

/// Resolve discovery URL + master key from env vars or the Sled config store.
/// Returns None if discovery is not configured — the caller treats this as a
/// skippable no-op tick rather than an error.
fn resolve_discovery_config() -> Option<(String, Vec<u8>)> {
    use sha2::{Digest, Sha256};

    // Env vars (set by run.sh and by http_server.rs startup).
    if let (Ok(url), Ok(key_hex)) = (
        std::env::var("DISCOVERY_SERVICE_URL"),
        std::env::var("DISCOVERY_MASTER_KEY"),
    ) {
        let key = hex::decode(&key_hex).ok()?;
        return Some((url, key));
    }

    // Fallback: derive from Sled NodeConfigStore.
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
    let identity = store.get_identity()?;

    let url = format!("{}/api", cloud.api_url);
    let key = Sha256::digest(identity.private_key.as_bytes()).to_vec();
    Some((url, key))
}

/// Resolve an auth token from env or the local credential store.
/// Returns None if none is available.
fn resolve_auth_token() -> Option<String> {
    if let Ok(token) = std::env::var("DISCOVERY_AUTH_TOKEN") {
        return Some(token);
    }
    crate::keychain::load_credentials()
        .ok()
        .flatten()
        .filter(|c| !c.session_token.is_empty())
        .map(|c| c.session_token)
}
