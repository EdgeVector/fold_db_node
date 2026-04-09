use crate::cli::CloudCommand;
use crate::commands::CommandOutput;
use crate::error::CliError;
use dialoguer::{Confirm, Input};
use fold_db_node::fold_node::OperationProcessor;

use super::setup::{derive_recovery_phrase, register_with_exemem};

/// Run cloud subcommands
pub async fn run(
    action: &CloudCommand,
    processor: &OperationProcessor,
    config_path: Option<&str>,
) -> Result<CommandOutput, CliError> {
    match action {
        CloudCommand::Enable => enable(processor, config_path).await,
        CloudCommand::Disable => disable(config_path).await,
        CloudCommand::Status => status(processor),
    }
}

async fn enable(
    processor: &OperationProcessor,
    config_path: Option<&str>,
) -> Result<CommandOutput, CliError> {
    let db_config = processor.get_database_config();
    if db_config.has_cloud_sync() {
        return Ok(CommandOutput::Message(
            "Cloud backup is already enabled.".to_string(),
        ));
    }

    let invite_code: String = Input::new()
        .with_prompt("Invite code")
        .interact_text()
        .map_err(|e| CliError::new(format!("Input cancelled: {}", e)))?;

    let api_url = fold_db_node::endpoints::exemem_api_url();
    let pub_key = processor.get_node_public_key();
    let pub_key_hex: String = pub_key
        .as_bytes()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();

    eprintln!();
    eprint!("Registering with Exemem...");
    let _ = &invite_code; // TODO: pass to API when supported
    let resp = register_with_exemem(&api_url, &pub_key_hex)?;
    eprintln!(" done.");

    let api_key = resp
        .api_key
        .ok_or_else(|| CliError::new("Registration response missing api_key".to_string()))?;

    // Update config file
    let path = resolve_config_path(config_path)?;
    let contents = std::fs::read_to_string(&path)
        .map_err(|e| CliError::new(format!("Failed to read config: {}", e)))?;
    let mut config: serde_json::Value = serde_json::from_str(&contents)
        .map_err(|e| CliError::new(format!("Failed to parse config: {}", e)))?;

    config["database"]["cloud_sync"] = serde_json::json!({
        "api_url": api_url,
        "api_key": api_key,
        "user_hash": resp.user_hash,
    });

    let updated = serde_json::to_string_pretty(&config)
        .map_err(|e| CliError::new(format!("Failed to serialize config: {}", e)))?;
    std::fs::write(&path, updated)
        .map_err(|e| CliError::new(format!("Failed to write config: {}", e)))?;

    // Show recovery phrase
    let private_key = processor.get_node_private_key();
    let mut msg = "Cloud backup enabled!\n".to_string();

    match derive_recovery_phrase(&private_key) {
        Ok(words) => {
            msg.push_str("\n\x1b[33m  RECOVERY PHRASE (save these 24 words):\x1b[0m\n\n");
            for (i, word) in words.iter().enumerate() {
                msg.push_str(&format!("  {:2}. {:<12}", i + 1, word));
                if (i + 1) % 4 == 0 {
                    msg.push('\n');
                }
            }
            msg.push_str(
                "\n  If you lose this device, these words are the\n  ONLY way to recover your data.\n",
            );
        }
        Err(e) => {
            msg.push_str(&format!(
                "\nWarning: Could not generate recovery phrase: {}\n",
                e
            ));
        }
    }

    if super::daemon::read_running_pid().is_some() {
        msg.push_str("\nRestart daemon for changes to take effect: folddb daemon stop && folddb daemon start");
    }

    Ok(CommandOutput::Message(msg))
}

async fn disable(config_path: Option<&str>) -> Result<CommandOutput, CliError> {
    let confirmed = Confirm::new()
        .with_prompt("Disable cloud backup? Your data remains on this device")
        .default(false)
        .interact()
        .map_err(|e| CliError::new(format!("Input cancelled: {}", e)))?;

    if !confirmed {
        return Ok(CommandOutput::Message("Cancelled.".to_string()));
    }

    let path = resolve_config_path(config_path)?;
    let contents = std::fs::read_to_string(&path)
        .map_err(|e| CliError::new(format!("Failed to read config: {}", e)))?;
    let mut config: serde_json::Value = serde_json::from_str(&contents)
        .map_err(|e| CliError::new(format!("Failed to parse config: {}", e)))?;

    if let Some(db) = config.get_mut("database") {
        if let Some(obj) = db.as_object_mut() {
            obj.remove("cloud_sync");
        }
    }

    let updated = serde_json::to_string_pretty(&config)
        .map_err(|e| CliError::new(format!("Failed to serialize config: {}", e)))?;
    std::fs::write(&path, updated)
        .map_err(|e| CliError::new(format!("Failed to write config: {}", e)))?;

    let mut msg = "Cloud backup disabled. Your data remains on this device.".to_string();
    if super::daemon::read_running_pid().is_some() {
        msg.push_str("\nRestart daemon for changes to take effect: folddb daemon stop && folddb daemon start");
    }

    Ok(CommandOutput::Message(msg))
}

fn status(processor: &OperationProcessor) -> Result<CommandOutput, CliError> {
    let db_config = processor.get_database_config();
    let msg = if let Some(cloud) = &db_config.cloud_sync {
        format!(
            "Cloud sync: enabled\nEndpoint: {}\nLocal path: {}",
            cloud.api_url,
            db_config.path.display()
        )
    } else {
        format!(
            "Cloud sync: disabled\nLocal path: {}",
            db_config.path.display()
        )
    };
    Ok(CommandOutput::Message(msg))
}

fn resolve_config_path(config_path: Option<&str>) -> Result<String, CliError> {
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
