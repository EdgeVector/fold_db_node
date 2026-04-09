use crate::commands::CommandOutput;
use crate::error::CliError;
use dialoguer::{Confirm, Input};
use fold_db_node::fold_node::OperationProcessor;

use super::setup::{derive_recovery_phrase, identity_file_exists, register_with_exemem};

/// Display the 24-word recovery phrase
pub fn recovery_phrase(processor: &OperationProcessor) -> Result<CommandOutput, CliError> {
    let private_key = processor.get_node_private_key();
    let words = derive_recovery_phrase(&private_key)?;

    let mut msg = String::new();
    msg.push_str("\x1b[33m  RECOVERY PHRASE (save these 24 words):\x1b[0m\n\n");
    for (i, word) in words.iter().enumerate() {
        msg.push_str(&format!("  {:2}. {:<12}", i + 1, word));
        if (i + 1) % 4 == 0 {
            msg.push('\n');
        }
    }
    msg.push_str(
        "\n  If you lose this device, these words are the\n  ONLY way to recover your data.\n",
    );

    Ok(CommandOutput::Message(msg))
}

/// Restore node from a 24-word recovery phrase
pub async fn restore() -> Result<CommandOutput, CliError> {
    if identity_file_exists() {
        let overwrite = Confirm::new()
            .with_prompt("An identity already exists. Overwrite it?")
            .default(false)
            .interact()
            .map_err(|e| CliError::new(format!("Input cancelled: {}", e)))?;
        if !overwrite {
            return Ok(CommandOutput::Message("Restore cancelled.".to_string()));
        }
    }

    eprintln!("Enter your 24-word recovery phrase (space-separated):");
    let phrase: String = Input::new()
        .with_prompt("Recovery phrase")
        .interact_text()
        .map_err(|e| CliError::new(format!("Input cancelled: {}", e)))?;

    let mnemonic = bip39::Mnemonic::parse_normalized(&phrase)
        .map_err(|e| CliError::new(format!("Invalid recovery phrase: {}", e)))?;

    let entropy = mnemonic.to_entropy();
    if entropy.len() < 32 {
        return Err(CliError::new("Recovery phrase entropy too short"));
    }

    use ed25519_dalek::SigningKey;
    let signing_key = SigningKey::from_bytes(
        entropy[..32]
            .try_into()
            .map_err(|_| CliError::new("Failed to create signing key from entropy"))?,
    );
    let verifying_key = signing_key.verifying_key();

    use base64::Engine;
    let private_key_b64 = base64::engine::general_purpose::STANDARD.encode(signing_key.to_bytes());
    let public_key_b64 = base64::engine::general_purpose::STANDARD.encode(verifying_key.to_bytes());

    // Save identity
    let config_dir = fold_db_node::utils::paths::folddb_home()
        .map(|h| h.join("config"))
        .unwrap_or_else(|_| std::path::PathBuf::from("config"));
    std::fs::create_dir_all(&config_dir)
        .map_err(|e| CliError::new(format!("Failed to create config dir: {}", e)))?;

    let identity = serde_json::json!({
        "private_key": private_key_b64,
        "public_key": public_key_b64,
    });
    let identity_json = serde_json::to_string_pretty(&identity)
        .map_err(|e| CliError::new(format!("Failed to serialize identity: {}", e)))?;
    std::fs::write(config_dir.join("node_identity.json"), &identity_json)
        .map_err(|e| CliError::new(format!("Failed to write identity: {}", e)))?;

    eprintln!("Identity restored.");
    eprintln!("Public key: {}", public_key_b64);

    // Register with Exemem to get fresh credentials
    let api_url = fold_db_node::endpoints::exemem_api_url();
    let pub_key_hex: String = verifying_key
        .to_bytes()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();

    eprint!("Registering with Exemem...");
    match register_with_exemem(&api_url, &pub_key_hex) {
        Ok(resp) => {
            eprintln!(" done.");
            let api_key = resp.api_key.unwrap_or_default();

            let data_path = fold_db_node::utils::paths::folddb_home()
                .map(|h| h.join("data"))
                .unwrap_or_else(|_| std::path::PathBuf::from("data"));

            let config = fold_db_node::fold_node::config::NodeConfig {
                database: fold_db::storage::DatabaseConfig::with_cloud_sync(
                    data_path.clone(),
                    fold_db::storage::CloudSyncConfig {
                        api_url,
                        api_key,
                        session_token: None,
                        user_hash: resp.user_hash,
                    },
                ),
                storage_path: Some(data_path),
                network_listen_address: "/ip4/0.0.0.0/tcp/0".to_string(),
                security_config: fold_db::security::SecurityConfig::from_env(),
                schema_service_url: Some(fold_db_node::endpoints::schema_service_url()),
                public_key: Some(public_key_b64),
                private_key: Some(private_key_b64),
                config_dir: None,
            };

            let config_json = serde_json::to_string_pretty(&config)
                .map_err(|e| CliError::new(format!("Failed to serialize config: {}", e)))?;
            std::fs::write(config_dir.join("node_config.json"), &config_json)
                .map_err(|e| CliError::new(format!("Failed to write config: {}", e)))?;

            Ok(CommandOutput::Message(
                "Identity restored with cloud backup enabled.\nRun `folddb daemon start` to begin syncing."
                    .to_string(),
            ))
        }
        Err(e) => {
            eprintln!(" failed: {}", e);

            let data_path = fold_db_node::utils::paths::folddb_home()
                .map(|h| h.join("data"))
                .unwrap_or_else(|_| std::path::PathBuf::from("data"));

            let config = fold_db_node::fold_node::config::NodeConfig {
                database: fold_db::storage::DatabaseConfig::local(data_path.clone()),
                storage_path: Some(data_path),
                network_listen_address: "/ip4/0.0.0.0/tcp/0".to_string(),
                security_config: fold_db::security::SecurityConfig::from_env(),
                schema_service_url: Some(fold_db_node::endpoints::schema_service_url()),
                public_key: Some(public_key_b64),
                private_key: Some(private_key_b64),
                config_dir: None,
            };

            let config_json = serde_json::to_string_pretty(&config)
                .map_err(|e| CliError::new(format!("Failed to serialize config: {}", e)))?;
            std::fs::write(config_dir.join("node_config.json"), &config_json)
                .map_err(|e| CliError::new(format!("Failed to write config: {}", e)))?;

            Ok(CommandOutput::Message(
                "Identity restored (local only).\nCloud registration failed — run `folddb cloud enable` to retry."
                    .to_string(),
            ))
        }
    }
}
