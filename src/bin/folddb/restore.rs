//! Restore identity from a 24-word BIP39 recovery phrase.
//!
//! Atomic: if any step fails after identity is written, the Sled
//! identity tree is cleared so the user can retry cleanly.

use crate::commands::CommandOutput;
use crate::error::CliError;
use dialoguer::{Confirm, Input};
use fold_db_node::identity::{self, NodeIdentity};
use std::path::{Path, PathBuf};

fn data_path() -> PathBuf {
    fold_db_node::utils::paths::folddb_home()
        .map(|h| h.join("data"))
        .unwrap_or_else(|_| PathBuf::from("data"))
}

/// Check whether an identity is already persisted in the Sled
/// `node_identity` tree.
fn identity_exists() -> bool {
    identity::load_standalone(&data_path())
        .ok()
        .flatten()
        .is_some()
}

/// Run the interactive restore flow.
pub async fn run_restore() -> Result<CommandOutput, CliError> {
    if identity_exists() {
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

    let restored = NodeIdentity {
        private_key: private_key_b64.clone(),
        public_key: public_key_b64.clone(),
    };

    // Save into the Sled identity tree. This is the point of no return
    // — if anything below fails we clear the tree so the user can retry.
    let data_path = data_path();
    identity::save_standalone(&data_path, &restored)
        .map_err(|e| CliError::new(format!("Failed to write identity: {}", e)))?;

    let config_dir = fold_db_node::utils::paths::folddb_home()
        .map(|h| h.join("config"))
        .unwrap_or_else(|_| PathBuf::from("config"));
    std::fs::create_dir_all(&config_dir)
        .map_err(|e| CliError::new(format!("Failed to create config dir: {}", e)))?;
    let config_path = config_dir.join("node_config.json");

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
            eprintln!("Rolling back identity due to error...");
            // Clear the Sled identity tree — symmetrical with save_standalone.
            if let Ok((pool, store)) = identity::open_standalone(&data_path) {
                let _ = store.clear();
                drop(pool);
            }
            let _ = std::fs::remove_file(&config_path);
            Err(e)
        }
    }
}

fn try_register_and_configure(
    public_key_b64: &str,
    private_key_b64: &str,
    verifying_key: &ed25519_dalek::VerifyingKey,
    config_path: &Path,
) -> Result<String, CliError> {
    let api_url = fold_db_node::endpoints::exemem_api_url();
    let pub_key_hex: String = verifying_key
        .to_bytes()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();

    eprint!("Registering with Exemem...");

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

            save_restored_cloud_config(config_path, &api_url, &api_key, resp.user_hash)?;

            fold_db_node::server::routes::auth::write_bootstrap_marker(&api_url, &api_key)
                .map_err(|e| CliError::new(format!("Failed to write bootstrap marker: {}", e)))?;

            // Silence unused-var warning: `public_key_b64` is used above for the
            // eprintln in the caller; kept as a parameter for clarity there.
            let _ = public_key_b64;

            mark_onboarding_complete();

            eprintln!("Identity restored.");
            Ok("Identity restored with cloud backup enabled.\n\
                Your database will be downloaded from the cloud on next daemon start.\n\
                Run `folddb daemon start` to begin."
                .to_string())
        }
        Err(e) => {
            eprintln!(" failed: {} (will use local-only mode).", e);

            save_restored_local_config(config_path)?;

            let _ = public_key_b64;

            mark_onboarding_complete();

            eprintln!("Identity restored.");
            Ok("Identity restored (local only).\nCloud registration failed — run `folddb cloud enable` to retry.".to_string())
        }
    }
}

/// Build the cloud-backed `NodeConfig` for a successful restore + register
/// flow and persist it via [`save_node_config`]. Extracted so the file
/// write is reachable from tests without going through `register_with_exemem`.
///
/// [`save_node_config`]: fold_db_node::fold_node::config::save_node_config
fn save_restored_cloud_config(
    config_path: &Path,
    api_url: &str,
    api_key: &str,
    user_hash: Option<String>,
) -> Result<(), CliError> {
    let data_path = data_path();
    let config = fold_db_node::fold_node::config::NodeConfig {
        database: fold_db::storage::DatabaseConfig::with_cloud_sync(
            data_path.clone(),
            fold_db::storage::CloudSyncConfig {
                api_url: api_url.to_string(),
                api_key: api_key.to_string(),
                session_token: None,
                user_hash,
                p2p_sync: None,
            },
        ),
        storage_path: Some(data_path),
        network_listen_address: "/ip4/0.0.0.0/tcp/0".to_string(),
        schema_service_url: Some(fold_db_node::endpoints::schema_service_url()),
        env: None,
        config_dir: None,
        seed_identity: None,
        source_path: Some(config_path.to_path_buf()),
    };

    fold_db_node::fold_node::config::save_node_config(&config)
        .map_err(|e| CliError::new(format!("Failed to write config: {}", e)))
}

/// Build the local-only `NodeConfig` for the registration-failed fallback
/// branch and persist it via [`save_node_config`]. Same testability rationale
/// as [`save_restored_cloud_config`].
///
/// [`save_node_config`]: fold_db_node::fold_node::config::save_node_config
fn save_restored_local_config(config_path: &Path) -> Result<(), CliError> {
    let data_path = data_path();
    let config = fold_db_node::fold_node::config::NodeConfig {
        database: fold_db::storage::DatabaseConfig::local(data_path.clone()),
        storage_path: Some(data_path),
        network_listen_address: "/ip4/0.0.0.0/tcp/0".to_string(),
        schema_service_url: Some(fold_db_node::endpoints::schema_service_url()),
        env: None,
        config_dir: None,
        seed_identity: None,
        source_path: Some(config_path.to_path_buf()),
    };

    fold_db_node::fold_node::config::save_node_config(&config)
        .map_err(|e| CliError::new(format!("Failed to write config: {}", e)))
}

/// Write the onboarding_complete marker so the UI doesn't re-prompt for setup.
///
/// Routed through sensitive_io because the marker's presence reveals that a
/// credentialed restore flow ran (PR #885 reasoning).
fn mark_onboarding_complete() {
    if let Ok(home) = fold_db_node::utils::paths::folddb_home() {
        let marker = home.join("data").join(".onboarding_complete");
        let _ = fold_db_node::sensitive_io::write_sensitive(&marker, b"1");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `save_restored_cloud_config` is the post-registration save in the
    /// successful-cloud branch of `try_register_and_configure`. The atomic
    /// write happens via `save_node_config` -> `write_atomic_0600`; this
    /// test exercises the helper directly so a regression where the
    /// helper bypasses `save_node_config` would fail.
    #[test]
    fn save_restored_cloud_config_writes_expected_node_config() {
        let _g = crate::test_locks::folddb_home_lock();
        let tmp = tempfile::tempdir().expect("tempdir");
        std::env::set_var("FOLDDB_HOME", tmp.path());

        let config_dir = tmp.path().join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        let config_path = config_dir.join("node_config.json");

        save_restored_cloud_config(
            &config_path,
            "https://api.example.test",
            "secret-cloud-api-key",
            Some("user-hash-abc".to_string()),
        )
        .expect("save_restored_cloud_config");

        assert!(
            config_path.exists(),
            "node_config.json must exist after save_restored_cloud_config"
        );
        let on_disk: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&config_path).expect("read node_config.json"))
                .expect("parse node_config.json");
        let cloud = on_disk
            .get("database")
            .and_then(|d| d.get("cloud_sync"))
            .expect("cloud_sync present after save_restored_cloud_config");
        // api_url is the only cloud_sync field that round-trips through
        // node_config.json — api_key, session_token, user_hash are stripped
        // by `CloudSyncConfig`'s `skip_serializing` markers (see PR #932).
        assert_eq!(
            cloud.get("api_url").and_then(|v| v.as_str()),
            Some("https://api.example.test"),
        );

        std::env::remove_var("FOLDDB_HOME");
    }

    /// `save_restored_local_config` is the registration-failed fallback
    /// branch. Same shape as the cloud test but the on-disk database
    /// must be local-only (no cloud_sync field).
    #[test]
    fn save_restored_local_config_writes_local_only_node_config() {
        let _g = crate::test_locks::folddb_home_lock();
        let tmp = tempfile::tempdir().expect("tempdir");
        std::env::set_var("FOLDDB_HOME", tmp.path());

        let config_dir = tmp.path().join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        let config_path = config_dir.join("node_config.json");

        save_restored_local_config(&config_path).expect("save_restored_local_config");

        assert!(
            config_path.exists(),
            "node_config.json must exist after save_restored_local_config"
        );
        let on_disk: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&config_path).expect("read node_config.json"))
                .expect("parse node_config.json");
        let db = on_disk
            .get("database")
            .expect("database object must be present");
        assert!(
            db.get("cloud_sync").is_none(),
            "local-only fallback must not write a cloud_sync block; got: {db}"
        );

        std::env::remove_var("FOLDDB_HOME");
    }
}
