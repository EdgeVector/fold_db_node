use crate::commands::CommandOutput;
use crate::error::CliError;
use crate::output::OutputMode;
use fold_db_node::fold_node::config::NodeConfig;

/// Runtime view of the daemon — what port it's configured for, whether a
/// healthy daemon is actually answering on that port, and the list of orgs
/// the daemon reports (empty when the daemon is down or the fetch failed).
/// `config show` only queries the daemon when one is already healthy, so this
/// struct is assembled in `main.rs` and handed in — keeping the formatter
/// itself synchronous and easy to unit-test.
#[derive(Debug, Clone, Default)]
pub struct DaemonInfo {
    pub port: u16,
    pub running: bool,
    pub orgs: Vec<String>,
}

/// Handle `folddb config` / `folddb config show` — prints the resolved
/// configuration (mode, data dir, schema service URL, cloud API URL, daemon
/// port, and org memberships) so users can see what the node will actually
/// use, not just the file path. No daemon required; daemon-dependent fields
/// degrade gracefully when the daemon is not running.
pub fn config_show(
    config: &NodeConfig,
    config_path: Option<&str>,
    mode: OutputMode,
    daemon_info: &DaemonInfo,
) -> Result<CommandOutput, CliError> {
    let resolved_path = resolve_path_for_display(config_path);

    let data_dir = config
        .storage_path
        .as_ref()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| config.database.path.to_string_lossy().to_string());

    let (schema_url, schema_url_source) = match &config.schema_service_url {
        Some(url) => (url.clone(), "config"),
        None => (fold_db_node::endpoints::schema_service_url(), "default"),
    };

    let (mode_label, cloud_api_url) = match &config.database.cloud_sync {
        Some(cs) => ("Exemem cloud sync", cs.api_url.clone()),
        None => ("Local only", String::new()),
    };

    match mode {
        OutputMode::Json => {
            let cloud_val = if cloud_api_url.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::Value::String(cloud_api_url.clone())
            };
            let orgs_val = if daemon_info.running {
                serde_json::Value::Array(
                    daemon_info
                        .orgs
                        .iter()
                        .cloned()
                        .map(serde_json::Value::String)
                        .collect(),
                )
            } else {
                serde_json::Value::Null
            };
            let out = serde_json::json!({
                "config_path": resolved_path,
                "mode": mode_label,
                "data_dir": data_dir,
                "schema_service_url": schema_url,
                "schema_service_url_source": schema_url_source,
                "cloud_api_url": cloud_val,
                "network_listen_address": config.network_listen_address,
                "public_key": config.public_key,
                "daemon_port": daemon_info.port,
                "daemon_running": daemon_info.running,
                "orgs": orgs_val,
            });
            Ok(CommandOutput::RawJson(out))
        }
        OutputMode::Human => {
            let schema_display = if schema_url_source == "default" {
                format!("{} (default)", schema_url)
            } else {
                schema_url
            };
            let cloud_display = if cloud_api_url.is_empty() {
                "(cloud disabled)".to_string()
            } else {
                cloud_api_url
            };
            let daemon_display = if daemon_info.running {
                format!("{} (running)", daemon_info.port)
            } else {
                format!("{} (not running)", daemon_info.port)
            };
            let orgs_display = if !daemon_info.running {
                "(daemon not running)".to_string()
            } else if daemon_info.orgs.is_empty() {
                "none".to_string()
            } else {
                daemon_info.orgs.join(", ")
            };
            let mut msg = String::new();
            msg.push_str(&format!("Config file:        {}\n", resolved_path));
            msg.push_str(&format!("Mode:               {}\n", mode_label));
            msg.push_str(&format!("Data dir:           {}\n", data_dir));
            msg.push_str(&format!("Schema service URL: {}\n", schema_display));
            msg.push_str(&format!("Cloud API URL:      {}\n", cloud_display));
            msg.push_str(&format!("Daemon port:        {}\n", daemon_display));
            msg.push_str(&format!("Orgs:               {}", orgs_display));
            Ok(CommandOutput::Message(msg))
        }
    }
}

/// Collect daemon-side state for `config show`. Uses a short health check so
/// `config show` stays snappy when the daemon is down. Org list is fetched
/// only when the daemon is healthy; fetch failures fall through to an empty
/// list rather than erroring — `config show` is a diagnostic, not a gate.
pub async fn gather_daemon_info(config: &NodeConfig) -> DaemonInfo {
    let port = super::daemon::default_port();
    let running = super::daemon::read_running_pid().is_some()
        && super::daemon::check_daemon_health(port).await;

    let orgs = if running {
        fetch_org_names(config, port).await.unwrap_or_default()
    } else {
        Vec::new()
    };

    DaemonInfo {
        port,
        running,
        orgs,
    }
}

/// Fetch org names from the live daemon's `/api/org` endpoint. Returns `None`
/// when the request fails or the daemon response is shaped unexpectedly —
/// `config show` renders an empty list in that case.
async fn fetch_org_names(config: &NodeConfig, port: u16) -> Option<Vec<String>> {
    let pk = config.public_key.as_ref()?;
    let user_hash = fold_db_node::utils::crypto::user_hash_from_pubkey(pk);
    let client = crate::client::FoldDbClient::new(port, &user_hash);
    let json = client.org_list().await.ok()?;
    let arr = json
        .pointer("/data/orgs")
        .or_else(|| json.get("orgs"))
        .and_then(|v| v.as_array())?;
    Some(
        arr.iter()
            .filter_map(|o| o.get("org_name").and_then(|n| n.as_str()).map(String::from))
            .collect(),
    )
}

/// Resolve the config path for display. Uses the same precedence as
/// `load_node_config` but falls back to a literal `"(not set)"` rather than
/// erroring — `config show` should still succeed when the file is missing so
/// the user sees the defaults the node would run with.
fn resolve_path_for_display(config_path: Option<&str>) -> String {
    config_path
        .map(|p| p.to_string())
        .or_else(|| std::env::var("NODE_CONFIG").ok())
        .or_else(|| {
            fold_db_node::utils::paths::folddb_home().ok().map(|h| {
                h.join("config")
                    .join("node_config.json")
                    .to_string_lossy()
                    .to_string()
            })
        })
        .unwrap_or_else(|| "(not set)".to_string())
}

/// Handle `folddb config set <key> <value>`
pub async fn config_set(
    key: &str,
    value: &str,
    config_path: Option<&str>,
) -> Result<CommandOutput, CliError> {
    match key {
        "env" => {
            match value {
                "dev" | "prod" => {}
                _ => {
                    return Err(CliError::new(format!(
                        "Invalid env value: '{}'. Must be 'dev' or 'prod'",
                        value
                    )));
                }
            }

            // Read existing config, update env field, write back
            let path = resolve_config_path(config_path)?;
            let contents = std::fs::read_to_string(&path)
                .map_err(|e| CliError::new(format!("Failed to read config: {}", e)))?;
            let mut config: serde_json::Value = serde_json::from_str(&contents)
                .map_err(|e| CliError::new(format!("Failed to parse config: {}", e)))?;
            config["env"] = serde_json::Value::String(value.to_string());
            let updated = serde_json::to_string_pretty(&config)
                .map_err(|e| CliError::new(format!("Failed to serialize config: {}", e)))?;
            std::fs::write(&path, updated)
                .map_err(|e| CliError::new(format!("Failed to write config: {}", e)))?;

            let msg = format!("Set env = {}", value);
            // Warn if daemon is running
            if super::daemon::read_running_pid().is_some() {
                Ok(CommandOutput::Message(format!(
                    "{}. Restart daemon for changes to take effect.",
                    msg
                )))
            } else {
                Ok(CommandOutput::Message(msg))
            }
        }
        _ => Err(CliError::new(format!(
            "Unknown config key: '{}'. Supported: env",
            key
        ))),
    }
}

pub fn resolve_config_path(config_path: Option<&str>) -> Result<String, CliError> {
    config_path
        .map(|p| p.to_string())
        .or_else(|| std::env::var("NODE_CONFIG").ok())
        .or_else(|| {
            let home = std::env::var("FOLDDB_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| {
                    dirs::home_dir()
                        .unwrap_or_else(|| std::path::PathBuf::from("."))
                        .join(".folddb")
                });
            let path = home.join("config").join("node_config.json");
            if path.exists() {
                Some(path.to_string_lossy().to_string())
            } else {
                None
            }
        })
        .ok_or_else(|| CliError::new("No config file found"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fold_db_node::fold_node::config::NodeConfig;

    fn local_config() -> NodeConfig {
        let mut c = NodeConfig::new(std::path::PathBuf::from("/tmp/fdb-data"));
        c.public_key = Some("test-pubkey".to_string());
        c
    }

    fn info_down() -> DaemonInfo {
        DaemonInfo {
            port: 9001,
            running: false,
            orgs: Vec::new(),
        }
    }

    fn info_up(orgs: Vec<&str>) -> DaemonInfo {
        DaemonInfo {
            port: 9001,
            running: true,
            orgs: orgs.into_iter().map(String::from).collect(),
        }
    }

    #[test]
    fn human_show_local_mode_includes_all_fields() {
        let config = local_config();
        let out = config_show(
            &config,
            Some("/tmp/cfg.json"),
            OutputMode::Human,
            &info_down(),
        )
        .unwrap();
        let CommandOutput::Message(msg) = out else {
            panic!("expected Message");
        };
        assert!(msg.contains("Config file:"));
        assert!(msg.contains("/tmp/cfg.json"));
        assert!(msg.contains("Mode:"));
        assert!(msg.contains("Local only"));
        assert!(msg.contains("Data dir:"));
        assert!(msg.contains("/tmp/fdb-data"));
        assert!(msg.contains("Schema service URL:"));
        assert!(msg.contains("Cloud API URL:"));
        assert!(msg.contains("(cloud disabled)"));
        assert!(msg.contains("Daemon port:"));
        assert!(msg.contains("9001"));
        assert!(msg.contains("(not running)"));
        assert!(msg.contains("Orgs:"));
        assert!(msg.contains("(daemon not running)"));
    }

    #[test]
    fn human_show_cloud_mode_shows_api_url() {
        let mut config = local_config();
        config.database.cloud_sync = Some(fold_db::storage::config::CloudSyncConfig {
            api_url: "https://example.test".to_string(),
            api_key: "SECRET-KEY".to_string(),
            session_token: Some("SECRET-TOKEN".to_string()),
            user_hash: Some("hash".to_string()),
        });
        let out = config_show(
            &config,
            Some("/tmp/cfg.json"),
            OutputMode::Human,
            &info_down(),
        )
        .unwrap();
        let CommandOutput::Message(msg) = out else {
            panic!("expected Message");
        };
        assert!(msg.contains("Exemem cloud sync"));
        assert!(msg.contains("https://example.test"));
        // Secrets must never appear in `config show` output.
        assert!(!msg.contains("SECRET-KEY"));
        assert!(!msg.contains("SECRET-TOKEN"));
    }

    #[test]
    fn json_show_does_not_leak_secrets() {
        let mut config = local_config();
        config.private_key = Some("PRIVATE-KEY-NEVER-SHOW".to_string());
        config.database.cloud_sync = Some(fold_db::storage::config::CloudSyncConfig {
            api_url: "https://example.test".to_string(),
            api_key: "API-KEY-NEVER-SHOW".to_string(),
            session_token: Some("SESSION-NEVER-SHOW".to_string()),
            user_hash: Some("hash".to_string()),
        });
        let out = config_show(
            &config,
            Some("/tmp/cfg.json"),
            OutputMode::Json,
            &info_down(),
        )
        .unwrap();
        let CommandOutput::RawJson(json) = out else {
            panic!("expected RawJson");
        };
        let s = serde_json::to_string(&json).unwrap();
        assert!(!s.contains("PRIVATE-KEY-NEVER-SHOW"));
        assert!(!s.contains("API-KEY-NEVER-SHOW"));
        assert!(!s.contains("SESSION-NEVER-SHOW"));
        assert_eq!(json["cloud_api_url"], "https://example.test");
        assert_eq!(json["mode"], "Exemem cloud sync");
    }

    #[test]
    fn schema_service_url_falls_back_to_default() {
        let config = local_config();
        assert!(config.schema_service_url.is_none());
        let out = config_show(
            &config,
            Some("/tmp/cfg.json"),
            OutputMode::Json,
            &info_down(),
        )
        .unwrap();
        let CommandOutput::RawJson(json) = out else {
            panic!("expected RawJson");
        };
        assert_eq!(json["schema_service_url_source"], "default");
        assert!(json["schema_service_url"]
            .as_str()
            .unwrap()
            .starts_with("http"));
    }

    #[test]
    fn schema_service_url_from_config_labeled_config() {
        let mut config = local_config();
        config.schema_service_url = Some("http://127.0.0.1:9002".to_string());
        let out = config_show(
            &config,
            Some("/tmp/cfg.json"),
            OutputMode::Json,
            &info_down(),
        )
        .unwrap();
        let CommandOutput::RawJson(json) = out else {
            panic!("expected RawJson");
        };
        assert_eq!(json["schema_service_url_source"], "config");
        assert_eq!(json["schema_service_url"], "http://127.0.0.1:9002");
    }

    #[test]
    fn json_show_includes_daemon_port_and_orgs_when_running() {
        let config = local_config();
        let info = info_up(vec!["alpha", "beta"]);
        let out = config_show(&config, Some("/tmp/cfg.json"), OutputMode::Json, &info).unwrap();
        let CommandOutput::RawJson(json) = out else {
            panic!("expected RawJson");
        };
        assert_eq!(json["daemon_port"], 9001);
        assert_eq!(json["daemon_running"], true);
        assert_eq!(json["orgs"], serde_json::json!(["alpha", "beta"]));
    }

    #[test]
    fn json_show_orgs_null_when_daemon_down() {
        let config = local_config();
        let out = config_show(
            &config,
            Some("/tmp/cfg.json"),
            OutputMode::Json,
            &info_down(),
        )
        .unwrap();
        let CommandOutput::RawJson(json) = out else {
            panic!("expected RawJson");
        };
        assert_eq!(json["daemon_running"], false);
        assert!(json["orgs"].is_null());
    }

    #[test]
    fn human_show_orgs_listed_when_running() {
        let config = local_config();
        let info = info_up(vec!["alpha", "beta"]);
        let out = config_show(&config, Some("/tmp/cfg.json"), OutputMode::Human, &info).unwrap();
        let CommandOutput::Message(msg) = out else {
            panic!("expected Message");
        };
        assert!(msg.contains("(running)"));
        assert!(msg.contains("alpha, beta"));
    }

    #[test]
    fn human_show_orgs_none_when_running_but_empty() {
        let config = local_config();
        let info = info_up(vec![]);
        let out = config_show(&config, Some("/tmp/cfg.json"), OutputMode::Human, &info).unwrap();
        let CommandOutput::Message(msg) = out else {
            panic!("expected Message");
        };
        assert!(msg.contains("Orgs:               none"));
    }
}
