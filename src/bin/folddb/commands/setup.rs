use crate::error::CliError;
use dialoguer::{Confirm, Input};
use fold_db::security::{Ed25519KeyPair, SecurityConfig};
use fold_db::storage::{CloudSyncConfig, DatabaseConfig};
use fold_db_node::fold_node::config::NodeConfig;
use fold_db_node::trust::identity_card::IdentityCard;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

fn default_schema_service_url() -> String {
    fold_db_node::endpoints::schema_service_url()
}

#[derive(Serialize, Deserialize)]
struct NodeIdentity {
    private_key: String,
    public_key: String,
}

/// Response from the Exemem CLI registration endpoint.
#[derive(Deserialize)]
pub struct ExememRegisterResponse {
    pub ok: bool,
    #[serde(default)]
    pub user_hash: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

/// Check whether a persisted node identity exists at `$FOLDDB_HOME/config/node_identity.json`.
pub fn identity_file_exists() -> bool {
    let path = fold_db_node::utils::paths::folddb_home()
        .map(|h| h.join("config").join("node_identity.json"))
        .unwrap_or_else(|_| PathBuf::from("config/node_identity.json"));
    path.exists()
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Register the node's public key with the Exemem API.
pub fn register_with_exemem(
    api_url: &str,
    public_key_hex: &str,
) -> Result<ExememRegisterResponse, CliError> {
    let url = format!("{}/api/auth/cli/register", api_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "public_key": public_key_hex,
    });

    let result = std::thread::spawn(move || {
        let client = reqwest::blocking::Client::new();
        client
            .post(&url)
            .json(&body)
            .timeout(std::time::Duration::from_secs(30))
            .send()
    })
    .join()
    .map_err(|_| CliError::new("Registration request thread panicked".to_string()))?
    .map_err(|e| CliError::new(format!("Failed to reach Exemem API: {}", e)))?;

    let status = result.status();
    let body_text = result
        .text()
        .map_err(|e| CliError::new(format!("Failed to read response body: {}", e)))?;

    if !status.is_success() {
        let msg = serde_json::from_str::<serde_json::Value>(&body_text)
            .ok()
            .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(String::from))
            .unwrap_or(body_text);
        return Err(CliError::new(format!(
            "Exemem registration failed (HTTP {}): {}",
            status, msg
        )));
    }

    let resp: ExememRegisterResponse = serde_json::from_str(&body_text)
        .map_err(|e| CliError::new(format!("Failed to parse registration response: {}", e)))?;

    if !resp.ok {
        let msg = resp.message.unwrap_or_else(|| "Unknown error".to_string());
        return Err(CliError::new(format!(
            "Exemem registration failed: {}",
            msg
        )));
    }

    Ok(resp)
}

/// Derive BIP39 recovery phrase from an Ed25519 private key.
pub fn derive_recovery_phrase(private_key_base64: &str) -> Result<Vec<String>, CliError> {
    use base64::Engine;
    let key_bytes = base64::engine::general_purpose::STANDARD
        .decode(private_key_base64)
        .map_err(|e| CliError::new(format!("Failed to decode private key: {}", e)))?;

    // Use first 32 bytes as entropy for BIP39 (24 words = 256 bits)
    let entropy = if key_bytes.len() >= 32 {
        &key_bytes[..32]
    } else {
        return Err(CliError::new("Private key too short for recovery phrase"));
    };

    let mnemonic = bip39::Mnemonic::from_entropy(entropy)
        .map_err(|e| CliError::new(format!("Failed to generate mnemonic: {}", e)))?;

    Ok(mnemonic.words().map(|w| w.to_string()).collect())
}

/// Run the interactive setup wizard.
///
/// Returns a fully populated `NodeConfig` with identity keys embedded.
pub fn run_setup_wizard() -> Result<NodeConfig, CliError> {
    eprintln!();
    eprintln!("Welcome to FoldDB!");
    eprintln!();

    // --- Generate identity ---
    eprint!("Generating node identity...");
    let keypair = Ed25519KeyPair::generate()
        .map_err(|e| CliError::new(format!("Failed to generate keypair: {}", e)))?;
    eprintln!(" done.");
    eprintln!();

    let identity = NodeIdentity {
        private_key: keypair.secret_key_base64(),
        public_key: keypair.public_key_base64(),
    };

    // --- Identity card: name, email, birthday ---
    let name: String = Input::new()
        .with_prompt("Your name")
        .interact_text()
        .map_err(|e| CliError::new(format!("Input cancelled: {}", e)))?;

    let email: String = Input::new()
        .with_prompt("Contact email (optional, press Enter to skip)")
        .default(String::new())
        .interact_text()
        .map_err(|e| CliError::new(format!("Input cancelled: {}", e)))?;
    let email = if email.is_empty() { None } else { Some(email) };

    let birthday: String = Input::new()
        .with_prompt("Birthday MM-DD (optional, press Enter to skip)")
        .default(String::new())
        .validate_with(|input: &String| {
            if input.is_empty() {
                Ok(())
            } else {
                IdentityCard::validate_birthday(input)
            }
        })
        .interact_text()
        .map_err(|e| CliError::new(format!("Input cancelled: {}", e)))?;
    let birthday = if birthday.is_empty() {
        None
    } else {
        Some(birthday)
    };

    eprintln!();
    eprintln!("This info stays on your device. It's only shared when");
    eprintln!("YOU invite someone to connect — never uploaded to any");
    eprintln!("cloud service.");
    eprintln!();

    // Save identity card
    let card = IdentityCard::new(name, email, birthday);
    card.save()
        .map_err(|e| CliError::new(format!("Failed to save identity card: {}", e)))?;

    // --- Cloud backup ---
    let enable_cloud = Confirm::new()
        .with_prompt("Enable cloud backup?")
        .default(false)
        .interact()
        .map_err(|e| CliError::new(format!("Input cancelled: {}", e)))?;

    let default_path = fold_db_node::utils::paths::folddb_home()
        .map(|h| h.join("data"))
        .unwrap_or_else(|_| PathBuf::from("data"));

    let database = if enable_cloud {
        let invite_code: String = Input::new()
            .with_prompt("Invite code")
            .interact_text()
            .map_err(|e| CliError::new(format!("Input cancelled: {}", e)))?;

        let api_url = fold_db_node::endpoints::exemem_api_url();
        let public_key_hex = hex_encode(&keypair.public_key_bytes());

        eprintln!();
        eprint!("Registering with Exemem...");
        // TODO: pass invite_code to registration endpoint
        let _ = &invite_code; // Will be used when API supports it
        let resp = register_with_exemem(&api_url, &public_key_hex)?;
        eprintln!(" done.");

        let api_key = resp
            .api_key
            .ok_or_else(|| CliError::new("Registration response missing api_key".to_string()))?;

        // Show recovery phrase
        eprintln!();
        eprintln!("Cloud backup enabled!");
        eprintln!();

        match derive_recovery_phrase(&identity.private_key) {
            Ok(words) => {
                eprintln!("\x1b[33m  RECOVERY PHRASE (save these 24 words):\x1b[0m");
                eprintln!();
                for (i, word) in words.iter().enumerate() {
                    eprint!("  {:2}. {:<12}", i + 1, word);
                    if (i + 1) % 4 == 0 {
                        eprintln!();
                    }
                }
                eprintln!();
                eprintln!("  If you lose this device, these words are the");
                eprintln!("  ONLY way to recover your data.");
                eprintln!();
            }
            Err(e) => {
                eprintln!("Warning: Could not generate recovery phrase: {}", e);
            }
        }

        DatabaseConfig::with_cloud_sync(
            default_path,
            CloudSyncConfig {
                api_url,
                api_key,
                session_token: None,
                user_hash: resp.user_hash,
            },
        )
    } else {
        DatabaseConfig::local(default_path)
    };

    // --- Persist identity ---
    let config_dir = fold_db_node::utils::paths::folddb_home()
        .map(|h| h.join("config"))
        .unwrap_or_else(|_| PathBuf::from("config"));
    if !config_dir.exists() {
        fs::create_dir_all(&config_dir)
            .map_err(|e| CliError::new(format!("Failed to create config dir: {}", e)))?;
    }
    let identity_json = serde_json::to_string_pretty(&identity)
        .map_err(|e| CliError::new(format!("Failed to serialize identity: {}", e)))?;
    fs::write(config_dir.join("node_identity.json"), &identity_json)
        .map_err(|e| CliError::new(format!("Failed to write node_identity.json: {}", e)))?;

    // --- Build NodeConfig ---
    let storage_path = Some(database.path.clone());
    let config = NodeConfig {
        database,
        storage_path,
        network_listen_address: "/ip4/0.0.0.0/tcp/0".to_string(),
        security_config: SecurityConfig::from_env(),
        schema_service_url: Some(default_schema_service_url()),
        public_key: Some(identity.public_key),
        private_key: Some(identity.private_key),
        config_dir: None,
    };

    // Persist config
    let config_json = serde_json::to_string_pretty(&config)
        .map_err(|e| CliError::new(format!("Failed to serialize config: {}", e)))?;
    fs::write(config_dir.join("node_config.json"), &config_json)
        .map_err(|e| CliError::new(format!("Failed to write node_config.json: {}", e)))?;

    eprintln!(
        "Config saved to {}",
        config_dir.join("node_config.json").display()
    );
    eprintln!();

    Ok(config)
}
