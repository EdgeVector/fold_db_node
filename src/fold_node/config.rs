use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub use fold_db::storage::config::DatabaseConfig;

use crate::identity::NodeIdentity;

/// Configuration for a FoldNode instance.
///
/// Note: node identity (Ed25519 keypair) lives in the `node_identity`
/// Sled tree via [`crate::identity::IdentityStore`], not on this config.
/// Setup/restore/test paths may pre-seed a keypair for first boot via
/// [`Self::with_seed_identity`], which writes into the store before
/// [`crate::fold_node::FoldNode::new`] reads from it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    /// Database storage configuration
    #[serde(default)]
    pub database: DatabaseConfig,

    /// Explicit storage path override. Used by Exemem and Cloud modes where the
    /// database config doesn't carry a local path. `run.sh` writes this from
    /// `$FOLDDB_HOME/data` so multi-node setups each get their own Sled directory.
    #[serde(default)]
    pub storage_path: Option<PathBuf>,

    /// Network listening address
    #[serde(default = "default_network_listen_address")]
    pub network_listen_address: String,
    /// URL of the schema service (optional, if not provided will load from local directories)
    #[serde(default)]
    pub schema_service_url: Option<String>,
    /// Explicit config directory override.
    /// When set, trust modules (contact book, sharing roles, etc.) use this
    /// instead of resolving `$FOLDDB_HOME`. This eliminates env-var races in
    /// parallel tests.
    #[serde(default)]
    pub config_dir: Option<PathBuf>,

    /// Transient bootstrap hatch: if set, `FoldNode::new` writes this
    /// keypair into the `node_identity` Sled tree before resolving
    /// identity. Only written on first boot (when the tree is empty);
    /// subsequent boots ignore it and use the persisted value. Used by
    /// setup / restore / test flows that generate a keypair before the
    /// daemon starts. Never serialized — `node_config.json` must not
    /// carry secrets.
    #[serde(skip)]
    pub seed_identity: Option<NodeIdentity>,
}

fn default_network_listen_address() -> String {
    "/ip4/0.0.0.0/tcp/0".to_string()
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            database: DatabaseConfig::default(),
            storage_path: None,
            network_listen_address: default_network_listen_address(),
            schema_service_url: None,
            config_dir: None,
            seed_identity: None,
        }
    }
}

impl NodeConfig {
    /// Create a new node configuration with the specified storage path
    pub fn new(storage_path: PathBuf) -> Self {
        Self {
            database: DatabaseConfig::local(storage_path.clone()),
            storage_path: Some(storage_path),
            network_listen_address: default_network_listen_address(),
            schema_service_url: None,
            config_dir: None,
            seed_identity: None,
        }
    }

    /// Get the effective storage path.
    ///
    /// For Local mode the path is embedded in the database config.
    /// For Exemem/Cloud modes we use the explicit `storage_path` field
    /// (written by `run.sh` from `$FOLDDB_HOME/data`) so that each node
    /// instance gets its own Sled directory. Falls back to `"data"` for
    /// backwards compatibility when `storage_path` is absent.
    pub fn get_storage_path(&self) -> PathBuf {
        self.storage_path
            .clone()
            .unwrap_or_else(|| self.database.path.clone())
    }

    /// Set the network listening address
    pub fn with_network_listen_address(mut self, address: &str) -> Self {
        self.network_listen_address = address.to_string();
        self
    }

    /// Set the schema service URL
    pub fn with_schema_service_url(mut self, url: &str) -> Self {
        self.schema_service_url = Some(url.to_string());
        self
    }

    /// Pre-seed the node identity into the Sled identity store on first
    /// boot. Used by setup / restore / test flows that generate a keypair
    /// outside the live daemon and want the node to adopt it instead of
    /// auto-generating a fresh one. See the `seed_identity` field docs.
    pub fn with_seed_identity(mut self, identity: NodeIdentity) -> Self {
        self.seed_identity = Some(identity);
        self
    }

    /// Set an explicit config directory. Trust modules (contact book, sharing
    /// roles, classification defaults) will read/write files here instead of
    /// resolving `$FOLDDB_HOME`.
    pub fn with_config_dir(mut self, dir: PathBuf) -> Self {
        self.config_dir = Some(dir);
        self
    }

    /// Resolve the config directory.
    ///
    /// Priority:
    /// 1. Explicit `config_dir` on this config (set via `with_config_dir`)
    /// 2. `$FOLDDB_HOME/config`
    /// 3. `~/.folddb/config`
    pub fn get_config_dir(&self) -> Result<PathBuf, String> {
        if let Some(dir) = &self.config_dir {
            return Ok(dir.clone());
        }
        Ok(crate::utils::paths::folddb_home()?.join("config"))
    }
}

/// Load a node configuration from the given path or from the `NODE_CONFIG`
/// environment variable.
///
/// If the file does not exist, a default [`NodeConfig`] is returned. When a
/// `port` is provided in this case, the returned config will have its
/// `network_listen_address` set to `"/ip4/0.0.0.0/tcp/<port>"`.
pub fn load_node_config(
    path: Option<&str>,
    port: Option<u16>,
) -> Result<NodeConfig, std::io::Error> {
    use std::fs;

    let config_path = path
        .map(|p| p.to_string())
        .or_else(|| std::env::var("NODE_CONFIG").ok())
        .unwrap_or_else(|| {
            crate::utils::paths::folddb_home()
                .map(|h| {
                    h.join("config")
                        .join("node_config.json")
                        .to_string_lossy()
                        .to_string()
                })
                .unwrap_or_else(|_| "config/node_config.json".to_string())
        });

    if let Ok(config_str) = fs::read_to_string(&config_path) {
        match serde_json::from_str::<NodeConfig>(&config_str) {
            Ok(cfg) => Ok(cfg),
            Err(e) => {
                tracing::error!(
                target: "fold_node::http_server",
                        "Failed to parse node configuration: {}",
                        e
                    );
                Err(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
            }
        }
    } else {
        let mut config = NodeConfig::default();

        if let Some(p) = port {
            config.network_listen_address = format!("/ip4/0.0.0.0/tcp/{}", p);
        }
        Ok(config)
    }
}

/// Persist a [`NodeConfig`] to the same path [`load_node_config`] reads from.
///
/// Path resolution: `NODE_CONFIG` env var, else `$FOLDDB_HOME/config/node_config.json`,
/// else `config/node_config.json`. Creates the parent directory if missing.
pub fn save_node_config(config: &NodeConfig) -> Result<(), String> {
    use std::fs;

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

    if let Some(parent) = std::path::Path::new(&config_path).parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create config directory: {}", e))?;
    }

    let config_json = serde_json::to_string_pretty(config)
        .map_err(|e| format!("Failed to serialize config: {}", e))?;

    fs::write(&config_path, config_json)
        .map_err(|e| format!("Failed to write config file: {}", e))?;

    Ok(())
}
