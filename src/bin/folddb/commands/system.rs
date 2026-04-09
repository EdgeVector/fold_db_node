use crate::commands::CommandOutput;
use crate::error::CliError;

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
