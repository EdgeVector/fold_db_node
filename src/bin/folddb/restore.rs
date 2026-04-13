//! Restore identity from a 24-word BIP39 recovery phrase.
//!
//! Atomic: if any step fails after identity is written, the partial
//! identity file is removed so the user can retry cleanly.

use crate::commands::CommandOutput;
use crate::error::CliError;
use dialoguer::{Confirm, Input};

/// Check whether a persisted node identity exists.
fn identity_file_exists() -> bool {
    fold_db_node::utils::paths::folddb_home()
        .map(|h| h.join("config").join("node_identity.json").exists())
        .unwrap_or(false)
}

/// Run the interactive restore flow.
pub async fn run_restore() -> Result<CommandOutput, CliError> {
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

    // Parse and validate
    let mnemonic = bip39::Mnemonic::parse_normalized(&phrase)
        .map_err(|e| CliError::new(format!("Invalid recovery phrase: {}", e)))?;

    let entropy = mnemonic.to_entropy();
    if entropy.len() < 32 {
        return Err(CliError::new("Recovery phrase entropy too short"));
    }

    // Derive Ed25519 keypair
    use ed25519_dalek::SigningKey;
    let signing_key = SigningKey::from_bytes(
        entropy[..32]
            .try_into()
            .map_err(|_| CliError::new("Failed to create signing key"))?,
    );
    let verifying_key = signing_key.verifying_key();

    use base64::Engine;
    let private_key_b64 = base64::engine::general_purpose::STANDARD.encode(signing_key.to_bytes());
    let public_key_b64 = base64::engine::general_purpose::STANDARD.encode(verifying_key.to_bytes());

    // Save identity (this is the point of no return — rollback if anything after fails)
    let config_dir = fold_db_node::utils::paths::folddb_home()
        .map(|h| h.join("config"))
        .unwrap_or_else(|_| std::path::PathBuf::from("config"));
    std::fs::create_dir_all(&config_dir)
        .map_err(|e| CliError::new(format!("Failed to create config dir: {}", e)))?;

    let identity_path = config_dir.join("node_identity.json");
    let config_path = config_dir.join("node_config.json");

    let identity_json = serde_json::to_string_pretty(&serde_json::json!({
        "private_key": private_key_b64,
        "public_key": public_key_b64,
    }))
    .map_err(|e| CliError::new(format!("Failed to serialize identity: {}", e)))?;

    std::fs::write(&identity_path, &identity_json)
        .map_err(|e| CliError::new(format!("Failed to write identity: {}", e)))?;

    eprintln!("Public key: {}", public_key_b64);

    // Try to register with Exemem — rollback identity if this fails catastrophically
    let result = try_register_and_configure(
        &public_key_b64,
        &private_key_b64,
        &verifying_key,
        &config_path,
    );

    match result {
        Ok(msg) => Ok(CommandOutput::Message(msg)),
        Err(e) => {
            // Rollback: remove partial identity so user can retry
            eprintln!("Rolling back identity file due to error...");
            let _ = std::fs::remove_file(&identity_path);
            let _ = std::fs::remove_file(&config_path);
            Err(e)
        }
    }
}

fn try_register_and_configure(
    public_key_b64: &str,
    private_key_b64: &str,
    verifying_key: &ed25519_dalek::VerifyingKey,
    config_path: &std::path::Path,
) -> Result<String, CliError> {
    let api_url = fold_db_node::endpoints::exemem_api_url();
    let pub_key_hex: String = verifying_key
        .to_bytes()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();

    eprint!("Registering with Exemem...");

    // Use setup's register function
    match crate::commands::setup::register_with_exemem(&api_url, &pub_key_hex, private_key_b64) {
        Ok(resp) => {
            eprintln!(" done.");
            let api_key = match resp.api_key {
                Some(k) if !k.is_empty() => k,
                _ => {
                    return Err(CliError::new(
                        "Registration succeeded but no API key returned. Contact support.",
                    ));
                }
            };

            let data_path = fold_db_node::utils::paths::folddb_home()
                .map(|h| h.join("data"))
                .unwrap_or_else(|_| std::path::PathBuf::from("data"));

            let marker_api_url = api_url.clone();
            let marker_api_key = api_key.clone();

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
                public_key: Some(public_key_b64.to_string()),
                private_key: Some(private_key_b64.to_string()),
                config_dir: None,
            };

            let config_json = serde_json::to_string_pretty(&config)
                .map_err(|e| CliError::new(format!("Failed to serialize config: {}", e)))?;
            std::fs::write(config_path, config_json)
                .map_err(|e| CliError::new(format!("Failed to write config: {}", e)))?;

            // Write the bootstrap marker so the daemon resumes the cloud
            // database download on next start. Without this, the user ends
            // up with a registered-but-empty local DB (silent data loss).
            // Match the HTTP `/api/auth/restore` path exactly — same marker
            // shape, same location — via the shared helper in server::routes::auth.
            fold_db_node::server::routes::auth::write_bootstrap_marker(
                &marker_api_url,
                &marker_api_key,
            )
            .map_err(|e| CliError::new(format!("Failed to write bootstrap marker: {}", e)))?;

            mark_onboarding_complete();

            eprintln!("Identity restored with cloud backup enabled.");

            let msg = "Identity restored.\n\
                Run `folddb daemon start` — your database will be downloaded \
                and restored on first sync."
                .to_string();
            Ok(msg)
        }
        Err(e) => {
            eprintln!(" failed: {} (will use local-only mode).", e);

            let data_path = fold_db_node::utils::paths::folddb_home()
                .map(|h| h.join("data"))
                .unwrap_or_else(|_| std::path::PathBuf::from("data"));

            let config = fold_db_node::fold_node::config::NodeConfig {
                database: fold_db::storage::DatabaseConfig::local(data_path.clone()),
                storage_path: Some(data_path),
                network_listen_address: "/ip4/0.0.0.0/tcp/0".to_string(),
                security_config: fold_db::security::SecurityConfig::from_env(),
                schema_service_url: Some(fold_db_node::endpoints::schema_service_url()),
                public_key: Some(public_key_b64.to_string()),
                private_key: Some(private_key_b64.to_string()),
                config_dir: None,
            };

            let config_json = serde_json::to_string_pretty(&config)
                .map_err(|e| CliError::new(format!("Failed to serialize config: {}", e)))?;
            std::fs::write(config_path, config_json)
                .map_err(|e| CliError::new(format!("Failed to write config: {}", e)))?;

            mark_onboarding_complete();

            Ok("Identity restored (local only).\nCloud registration failed — run `folddb cloud enable` to retry.".to_string())
        }
    }
}

/// Write the onboarding_complete marker so the UI doesn't re-prompt for setup.
fn mark_onboarding_complete() {
    if let Ok(home) = fold_db_node::utils::paths::folddb_home() {
        let marker = home.join("data").join(".onboarding_complete");
        if let Some(parent) = marker.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&marker, "1");
    }
}
