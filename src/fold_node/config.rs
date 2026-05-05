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

    /// Path the config was loaded from. Populated by [`load_node_config`]
    /// so [`save_node_config`] writes back to the same file the daemon was
    /// launched against (e.g. `--config /custom/path.json`). Never
    /// serialized — it's a runtime hand-off, not a persisted setting.
    #[serde(skip)]
    pub source_path: Option<PathBuf>,
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
            source_path: None,
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
            source_path: None,
        }
    }

    /// Get the effective storage path.
    ///
    /// Prefers the explicit `storage_path` field (written by `run.sh` from
    /// `$FOLDDB_HOME/data` so each node instance gets its own Sled
    /// directory) and falls back to `database.path`. The earlier
    /// `"data"`-string fallback was retired with the env-var hand-off —
    /// `database.path` is always populated by the loader (it defaults to
    /// `PathBuf::from("data")` itself if no config supplies one).
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

    let mut config = if let Ok(config_str) = fs::read_to_string(&config_path) {
        match serde_json::from_str::<NodeConfig>(&config_str) {
            Ok(cfg) => cfg,
            Err(e) => {
                tracing::error!(
                target: "fold_node::http_server",
                        "Failed to parse node configuration: {}",
                        e
                    );
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, e));
            }
        }
    } else {
        let mut config = NodeConfig::default();

        if let Some(p) = port {
            config.network_listen_address = format!("/ip4/0.0.0.0/tcp/{}", p);
        }
        config
    };

    config.source_path = Some(PathBuf::from(&config_path));
    Ok(config)
}

/// Persist a [`NodeConfig`] to the same path [`load_node_config`] read from.
///
/// Path resolution: `config.source_path` (set by [`load_node_config`] so a
/// daemon launched with `--config /custom/path.json` round-trips back to
/// the same file), else `NODE_CONFIG` env var, else
/// `$FOLDDB_HOME/config/node_config.json`, else `config/node_config.json`.
/// Creates the parent directory if missing.
pub fn save_node_config(config: &NodeConfig) -> Result<(), String> {
    use std::fs;

    let config_path = config
        .source_path
        .as_ref()
        .map(|p| p.to_string_lossy().to_string())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `NODE_CONFIG` is process-wide; tests that mutate it serialize on
    /// this lock. `FOLDDB_HOME` is intentionally NOT touched here — it has
    /// its own per-module locks (`server::startup`, `handlers::auth`) that
    /// can't be reached across module-private boundaries, and contention
    /// with those tests was the cause of an earlier flake. The
    /// explicit-path arm exercises the bug without touching FOLDDB_HOME at
    /// all (source_path wins regardless), and the no-arg arm uses
    /// NODE_CONFIG (next priority below the explicit path) to anchor the
    /// resolution.
    static NODE_CONFIG_LOCK: Mutex<()> = Mutex::new(());

    struct NodeConfigEnvGuard {
        prev: Option<String>,
    }

    impl NodeConfigEnvGuard {
        fn capture() -> Self {
            Self {
                prev: std::env::var("NODE_CONFIG").ok(),
            }
        }
    }

    impl Drop for NodeConfigEnvGuard {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(v) => std::env::set_var("NODE_CONFIG", v),
                None => std::env::remove_var("NODE_CONFIG"),
            }
        }
    }

    /// The bug this PR fixes: a daemon launched with `--config /custom.json`
    /// would have its UI-driven saves silently routed to
    /// `$FOLDDB_HOME/config/node_config.json` instead. This test seeds an
    /// explicit path, mutates the loaded config, saves, and asserts the
    /// explicit file actually changed — pre-fix it would have stayed in its
    /// seeded state.
    #[test]
    fn save_round_trips_to_explicit_load_path() {
        let _guard = NODE_CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _env = NodeConfigEnvGuard::capture();
        // Clear NODE_CONFIG so the only viable resolution is `source_path`
        // — if that field is ignored, save would fall through to the
        // FOLDDB_HOME/cwd chain and the explicit file would stay unchanged.
        std::env::remove_var("NODE_CONFIG");

        let custom_dir = tempfile::tempdir().expect("custom config tempdir");
        let custom_path = custom_dir.path().join("daemon.json");

        std::fs::write(
            &custom_path,
            serde_json::to_string_pretty(&NodeConfig::default()).unwrap(),
        )
        .expect("seed custom config");

        let mut cfg = load_node_config(Some(custom_path.to_str().unwrap()), None)
            .expect("load from explicit path");
        assert_eq!(
            cfg.source_path.as_deref(),
            Some(custom_path.as_path()),
            "load must remember the explicit path",
        );

        cfg.network_listen_address = "/ip4/0.0.0.0/tcp/4242".to_string();
        save_node_config(&cfg).expect("save");

        let written = std::fs::read_to_string(&custom_path).expect("read back custom");
        let reread: NodeConfig = serde_json::from_str(&written).expect("parse custom");
        assert_eq!(
            reread.network_listen_address, "/ip4/0.0.0.0/tcp/4242",
            "save must persist back to the path we loaded from",
        );
    }

    /// Regression for the no-arg load path: `load_node_config(None, ...)`
    /// must populate `source_path` from whatever the resolution chain
    /// picked, and `save_node_config` must round-trip to that same file.
    /// Anchored on `NODE_CONFIG` (the next-priority arm) so the test
    /// doesn't need to mutate `FOLDDB_HOME`.
    #[test]
    fn save_round_trips_to_default_load_path() {
        let _guard = NODE_CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _env = NodeConfigEnvGuard::capture();

        let dir = tempfile::tempdir().expect("config tempdir");
        let path = dir.path().join("node_config.json");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&NodeConfig::default()).unwrap(),
        )
        .expect("seed default config");
        std::env::set_var("NODE_CONFIG", &path);

        let mut cfg = load_node_config(None, None).expect("load via NODE_CONFIG");
        assert_eq!(
            cfg.source_path.as_deref(),
            Some(path.as_path()),
            "no-arg load must capture the resolved path on source_path",
        );

        cfg.network_listen_address = "/ip4/0.0.0.0/tcp/9999".to_string();
        save_node_config(&cfg).expect("save");

        let written = std::fs::read_to_string(&path).expect("read back default");
        let reread: NodeConfig = serde_json::from_str(&written).expect("parse default");
        assert_eq!(reread.network_listen_address, "/ip4/0.0.0.0/tcp/9999");
    }
}
