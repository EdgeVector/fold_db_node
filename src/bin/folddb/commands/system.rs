use crate::commands::CommandOutput;
use crate::error::CliError;
use crate::output::OutputMode;
use fold_db_node::fold_node::config::NodeConfig;

/// Handle `folddb config` / `folddb config show` — prints the resolved
/// configuration (mode, data dir, schema service URL, cloud API URL) so users
/// can see what the node will actually use, not just the file path. No daemon
/// required.
pub fn config_show(
    config: &NodeConfig,
    config_path: Option<&str>,
    mode: OutputMode,
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
            let out = serde_json::json!({
                "config_path": resolved_path,
                "mode": mode_label,
                "data_dir": data_dir,
                "schema_service_url": schema_url,
                "schema_service_url_source": schema_url_source,
                "cloud_api_url": cloud_val,
                "network_listen_address": config.network_listen_address,
                "public_key": config.public_key,
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
            let mut msg = String::new();
            msg.push_str(&format!("Config file:        {}\n", resolved_path));
            msg.push_str(&format!("Mode:               {}\n", mode_label));
            msg.push_str(&format!("Data dir:           {}\n", data_dir));
            msg.push_str(&format!("Schema service URL: {}\n", schema_display));
            msg.push_str(&format!("Cloud API URL:      {}", cloud_display));
            Ok(CommandOutput::Message(msg))
        }
    }
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

    #[test]
    fn human_show_local_mode_includes_all_fields() {
        let config = local_config();
        let out = config_show(&config, Some("/tmp/cfg.json"), OutputMode::Human).unwrap();
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
        let out = config_show(&config, Some("/tmp/cfg.json"), OutputMode::Human).unwrap();
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
        let out = config_show(&config, Some("/tmp/cfg.json"), OutputMode::Json).unwrap();
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
        let out = config_show(&config, Some("/tmp/cfg.json"), OutputMode::Json).unwrap();
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
        let out = config_show(&config, Some("/tmp/cfg.json"), OutputMode::Json).unwrap();
        let CommandOutput::RawJson(json) = out else {
            panic!("expected RawJson");
        };
        assert_eq!(json["schema_service_url_source"], "config");
        assert_eq!(json["schema_service_url"], "http://127.0.0.1:9002");
    }
}
