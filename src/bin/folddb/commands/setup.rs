use crate::error::CliError;
use dialoguer::{Input, Select};
use fold_db::security::{Ed25519KeyPair, SecurityConfig};
use fold_db::storage::DatabaseConfig;
use fold_db_node::fold_node::config::NodeConfig;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_SCHEMA_SERVICE_URL: &str = "https://y0q3m6vk75.execute-api.us-west-2.amazonaws.com";

const DEFAULT_EXEMEM_API_URL: &str = "https://api.exemem.com";

#[derive(Serialize, Deserialize)]
struct NodeIdentity {
    private_key: String,
    public_key: String,
}

/// Response from the Exemem CLI registration endpoint.
#[derive(Deserialize)]
struct ExememRegisterResponse {
    ok: bool,
    #[serde(default)]
    user_hash: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    deposit_address: Option<String>,
    #[serde(default)]
    network: Option<String>,
    #[serde(default)]
    chain_id: Option<u64>,
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

/// Check whether a persisted node identity exists at `config/node_identity.json`.
pub fn identity_file_exists() -> bool {
    Path::new("config/node_identity.json").exists()
}

/// Hex-encode a byte slice.
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Register the node's public key with the Exemem API.
///
/// Runs the blocking HTTP call on a dedicated thread to avoid panicking
/// inside the tokio runtime.
fn register_with_exemem(
    api_url: &str,
    public_key_hex: &str,
) -> Result<ExememRegisterResponse, CliError> {
    let url = format!("{}/api/auth/cli/register", api_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "public_key": public_key_hex,
    });

    // Spawn on a separate OS thread so reqwest::blocking works even when a
    // tokio runtime is active on the current thread.
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
        // Try to extract a message from the JSON body, fall back to raw text
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

/// Run the interactive setup wizard.
///
/// Returns a fully populated `NodeConfig` with identity keys embedded.
pub fn run_setup_wizard() -> Result<NodeConfig, CliError> {
    eprintln!();
    eprintln!("Welcome to FoldDB setup!");
    eprintln!();

    // --- Generate identity first (needed for Exemem registration) ---
    eprint!("Generating node identity...");
    let keypair = Ed25519KeyPair::generate()
        .map_err(|e| CliError::new(format!("Failed to generate keypair: {}", e)))?;
    eprintln!(" done.");
    eprintln!();

    let identity = NodeIdentity {
        private_key: keypair.secret_key_base64(),
        public_key: keypair.public_key_base64(),
    };

    // --- Backend selection ---
    let backends = &[
        "Local (Sled - embedded, runs on this machine)",
        "Exemem Cloud (local Sled + encrypted S3 sync)",
    ];
    let backend_idx = Select::new()
        .with_prompt("Storage backend")
        .items(backends)
        .default(0)
        .interact()
        .map_err(|e| CliError::new(format!("Selection cancelled: {}", e)))?;

    let database = match backend_idx {
        0 => {
            let default_path = dirs::home_dir()
                .map(|h| h.join(".folddb").join("data"))
                .unwrap_or_else(|| PathBuf::from("data"));

            let data_dir: String = Input::new()
                .with_prompt("Data directory")
                .default(default_path.to_string_lossy().to_string())
                .interact_text()
                .map_err(|e| CliError::new(format!("Input cancelled: {}", e)))?;

            DatabaseConfig::Local {
                path: PathBuf::from(data_dir),
            }
        }
        1 => {
            let api_url: String = Input::new()
                .with_prompt("Exemem API URL")
                .default(DEFAULT_EXEMEM_API_URL.to_string())
                .interact_text()
                .map_err(|e| CliError::new(format!("Input cancelled: {}", e)))?;

            let public_key_hex = hex_encode(&keypair.public_key_bytes());

            eprintln!();
            eprint!("Registering with Exemem...");
            let resp = register_with_exemem(&api_url, &public_key_hex)?;
            eprintln!(" done.");
            eprintln!();

            let api_key = resp.api_key.ok_or_else(|| {
                CliError::new("Registration response missing api_key".to_string())
            })?;
            let user_hash = resp.user_hash.unwrap_or_default();
            let deposit_address = resp.deposit_address.unwrap_or_default();
            let network = resp.network.unwrap_or_default();
            let chain_id = resp.chain_id.unwrap_or(0);
            let token = resp.token.unwrap_or_default();

            eprintln!("Account created successfully!");
            eprintln!("  User hash:        {}", user_hash);
            eprintln!("  Network:          {} (chain {})", network, chain_id);
            eprintln!("  Payment token:    {}", token);
            eprintln!();
            eprintln!("Fund your account by sending {} on {} to:", token, network);
            eprintln!("  {}", deposit_address);
            eprintln!();

            DatabaseConfig::Exemem {
                api_url,
                api_key,
                session_token: None,
                user_hash: None,
            }
        }
        _ => unreachable!(),
    };

    // --- Schema service URL ---
    let schema_url: String = Input::new()
        .with_prompt("Schema service URL")
        .default(DEFAULT_SCHEMA_SERVICE_URL.to_string())
        .interact_text()
        .map_err(|e| CliError::new(format!("Input cancelled: {}", e)))?;

    // --- Persist identity ---
    let config_dir = Path::new("config");
    if !config_dir.exists() {
        fs::create_dir_all(config_dir)
            .map_err(|e| CliError::new(format!("Failed to create config dir: {}", e)))?;
    }
    let identity_json = serde_json::to_string_pretty(&identity)
        .map_err(|e| CliError::new(format!("Failed to serialize identity: {}", e)))?;
    fs::write(config_dir.join("node_identity.json"), &identity_json)
        .map_err(|e| CliError::new(format!("Failed to write node_identity.json: {}", e)))?;

    // --- Build NodeConfig ---
    let config = NodeConfig {
        database,
        network_listen_address: "/ip4/0.0.0.0/tcp/0".to_string(),
        security_config: SecurityConfig::from_env(),
        schema_service_url: Some(schema_url),
        public_key: Some(identity.public_key),
        private_key: Some(identity.private_key),
    };

    // Persist config
    let config_json = serde_json::to_string_pretty(&config)
        .map_err(|e| CliError::new(format!("Failed to serialize config: {}", e)))?;
    fs::write(config_dir.join("node_config.json"), &config_json)
        .map_err(|e| CliError::new(format!("Failed to write node_config.json: {}", e)))?;

    eprintln!("Config saved to config/node_config.json");
    eprintln!();

    Ok(config)
}
