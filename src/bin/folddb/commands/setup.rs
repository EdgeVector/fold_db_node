use crate::error::CliError;
use base64::Engine;
use dialoguer::{Confirm, Input};
use fold_db::security::{Ed25519KeyPair, KeyUtils, SecurityConfig};
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

/// Sign the canonical CLI-register payload `"{public_key_hex}:{timestamp}"` with the
/// node's base64-encoded Ed25519 private key. Returns the base64 signature.
///
/// Must stay in sync with:
/// - `fold_db_node/src/server/routes/auth.rs::sign_payload`
/// - `exemem-infra/lambdas/auth_service/src/cli/types.rs::verify_ed25519_signature`
pub fn sign_cli_register_payload(
    private_key_b64: &str,
    public_key_hex: &str,
    timestamp: i64,
) -> Result<String, CliError> {
    let key_bytes = base64::engine::general_purpose::STANDARD
        .decode(private_key_b64)
        .map_err(|e| CliError::new(format!("Failed to decode private key: {}", e)))?;
    let key_pair = Ed25519KeyPair::from_secret_key(&key_bytes)
        .map_err(|e| CliError::new(format!("Failed to create key pair: {}", e)))?;
    let payload = format!("{}:{}", public_key_hex, timestamp);
    let signature = key_pair.sign(payload.as_bytes());
    Ok(KeyUtils::signature_to_base64(&signature))
}

/// Register the node's public key with the Exemem API.
///
/// The request is signed with the node's private key so the server can verify
/// key ownership and allow idempotent re-registration.
pub fn register_with_exemem(
    api_url: &str,
    public_key_hex: &str,
    private_key_b64: &str,
) -> Result<ExememRegisterResponse, CliError> {
    register_with_exemem_and_invite(api_url, public_key_hex, private_key_b64, None)
}

/// Register with Exemem, optionally passing an invite code.
///
/// Always sends a signed request — the caller must provide the node's
/// base64-encoded Ed25519 private key so the payload can be signed.
pub fn register_with_exemem_and_invite(
    api_url: &str,
    public_key_hex: &str,
    private_key_b64: &str,
    invite_code: Option<&str>,
) -> Result<ExememRegisterResponse, CliError> {
    let url = format!("{}/api/auth/cli/register", api_url.trim_end_matches('/'));
    let timestamp = chrono::Utc::now().timestamp();
    let signature = sign_cli_register_payload(private_key_b64, public_key_hex, timestamp)?;
    let mut body = serde_json::json!({
        "public_key": public_key_hex,
        "timestamp": timestamp,
        "signature": signature,
    });
    if let Some(code) = invite_code {
        body["invite_code"] = serde_json::Value::String(code.to_string());
    }

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

    // --- Generate or reuse identity ---
    let identity = if identity_file_exists() {
        // Resume from partial setup — reuse existing identity
        eprintln!("Found existing identity. Resuming setup...");
        eprintln!();
        let identity_path = fold_db_node::utils::paths::folddb_home()
            .map(|h| h.join("config").join("node_identity.json"))
            .map_err(|e| CliError::new(format!("Cannot find identity: {}", e)))?;
        let json = fs::read_to_string(&identity_path)
            .map_err(|e| CliError::new(format!("Failed to read identity: {}", e)))?;
        serde_json::from_str::<NodeIdentity>(&json)
            .map_err(|e| CliError::new(format!("Failed to parse identity: {}", e)))?
    } else {
        eprint!("Generating node identity...");
        let keypair = Ed25519KeyPair::generate()
            .map_err(|e| CliError::new(format!("Failed to generate keypair: {}", e)))?;
        eprintln!(" done.");
        eprintln!();
        NodeIdentity {
            private_key: keypair.secret_key_base64(),
            public_key: keypair.public_key_base64(),
        }
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

    // --- AI setup ---
    eprintln!("Configure AI for data ingestion:");
    let ai_providers = &["Anthropic (cloud)", "Ollama (local)", "Skip for now"];
    let ai_idx = dialoguer::Select::new()
        .with_prompt("AI provider")
        .items(ai_providers)
        .default(0)
        .interact()
        .map_err(|e| CliError::new(format!("Input cancelled: {}", e)))?;

    let ai_config = match ai_idx {
        0 => {
            let api_key: String = Input::new()
                .with_prompt("Anthropic API key")
                .interact_text()
                .map_err(|e| CliError::new(format!("Input cancelled: {}", e)))?;
            Some(serde_json::json!({
                "provider": "Anthropic",
                "anthropic": {
                    "api_key": api_key,
                    "model": "claude-sonnet-4-20250514",
                    "base_url": "https://api.anthropic.com"
                }
            }))
        }
        1 => {
            let url: String = Input::new()
                .with_prompt("Ollama URL")
                .default("http://localhost:11434".to_string())
                .interact_text()
                .map_err(|e| CliError::new(format!("Input cancelled: {}", e)))?;
            let model: String = Input::new()
                .with_prompt("Ollama model")
                .default("llama3.2".to_string())
                .interact_text()
                .map_err(|e| CliError::new(format!("Input cancelled: {}", e)))?;
            Some(serde_json::json!({
                "provider": "Ollama",
                "ollama": {
                    "model": model,
                    "base_url": url
                }
            }))
        }
        _ => None,
    };
    eprintln!();

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
        let pub_key_bytes = base64::engine::general_purpose::STANDARD
            .decode(&identity.public_key)
            .map_err(|e| CliError::new(format!("Failed to decode public key: {}", e)))?;
        let public_key_hex = hex_encode(&pub_key_bytes);

        eprintln!();
        eprint!("Registering with Exemem...");
        let resp = register_with_exemem_and_invite(
            &api_url,
            &public_key_hex,
            &identity.private_key,
            Some(&invite_code),
        )?;
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

    // Save AI config if provided
    if let Some(ai) = &ai_config {
        let ai_config_path = config_dir.join("ingestion_config.json");
        let ai_json = serde_json::to_string_pretty(ai)
            .map_err(|e| CliError::new(format!("Failed to serialize AI config: {}", e)))?;
        fs::write(&ai_config_path, ai_json)
            .map_err(|e| CliError::new(format!("Failed to write AI config: {}", e)))?;
        eprintln!("AI config saved.");
    }

    // Mark onboarding complete — must match the path the server checks:
    // FOLDDB_HOME/data/.onboarding_complete (lives in data dir so --empty-db resets it)
    let marker_path = fold_db_node::utils::paths::folddb_home()
        .map(|h| h.join("data").join(".onboarding_complete"))
        .unwrap_or_else(|_| PathBuf::from(".onboarding_complete"));
    if let Some(parent) = marker_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&marker_path, "1");

    eprintln!(
        "Config saved to {}",
        config_dir.join("node_config.json").display()
    );
    eprintln!();

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fold_db::security::KeyUtils;

    #[test]
    fn sign_cli_register_payload_round_trips() {
        // Generate a fresh keypair.
        let key_pair = Ed25519KeyPair::generate().expect("generate keypair");
        let private_key_b64 = key_pair.secret_key_base64();
        let public_key_b64 = key_pair.public_key_base64();

        // Decode public key to bytes, then hex-encode (matching the lambda).
        let pub_key_bytes = base64::engine::general_purpose::STANDARD
            .decode(&public_key_b64)
            .expect("decode pub key");
        let public_key_hex = hex_encode(&pub_key_bytes);

        let timestamp: i64 = 1_700_000_000;
        let sig_b64 = sign_cli_register_payload(&private_key_b64, &public_key_hex, timestamp)
            .expect("sign payload");

        // Decode the base64 signature and verify it against the canonical payload.
        let signature = KeyUtils::signature_from_base64(&sig_b64).expect("signature from base64");

        let payload = format!("{}:{}", public_key_hex, timestamp);
        assert!(
            key_pair.verify(payload.as_bytes(), &signature),
            "signature should verify against canonical payload"
        );

        // Different payload must NOT verify.
        let wrong_payload = format!("{}:{}", public_key_hex, timestamp + 1);
        assert!(
            !key_pair.verify(wrong_payload.as_bytes(), &signature),
            "signature must not verify against altered payload"
        );
    }

    #[test]
    fn sign_cli_register_payload_rejects_invalid_private_key() {
        let err = sign_cli_register_payload("not-valid-base64!!!", "deadbeef", 123);
        assert!(err.is_err());
    }
}
