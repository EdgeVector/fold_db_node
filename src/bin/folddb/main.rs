#[cfg(target_os = "macos")]
mod apple;
mod cli;
mod client;
mod commands;
mod error;
mod output;
mod restore;
mod update_check;

use clap::Parser;
use cli::{Cli, Command, DaemonCommand};
use client::FoldDbClient;
use error::CliError;
use output::OutputMode;

use fold_db_node::utils::crypto::user_hash_from_pubkey;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    if !cli.json {
        update_check::spawn_update_check();
    }

    let json_mode = cli.json;
    let mode = if json_mode {
        OutputMode::Json
    } else {
        OutputMode::Human
    };

    let dev = cli.dev;

    // Commands that don't need a daemon or FoldNode
    match &cli.command {
        Command::Daemon { action } => {
            let result = match action {
                DaemonCommand::Start { port } => commands::daemon::start(*port, dev)
                    .await
                    .map(commands::CommandOutput::Message),
                DaemonCommand::Stop => {
                    commands::daemon::stop().map(commands::CommandOutput::Message)
                }
                DaemonCommand::Status => commands::daemon::status()
                    .await
                    .map(commands::CommandOutput::Message),
                DaemonCommand::Install => {
                    commands::daemon::install().map(commands::CommandOutput::Message)
                }
                DaemonCommand::Uninstall => {
                    commands::daemon::uninstall().map(commands::CommandOutput::Message)
                }
            };
            match result {
                Ok(out) => output::render(&out, mode),
                Err(e) => e.exit(json_mode),
            }
            return;
        }
        Command::Completions { shell } => {
            match commands::completions::run(*shell, cli.verbose) {
                Ok(out) => output::render(&out, mode),
                Err(e) => e.exit(json_mode),
            }
            return;
        }
        Command::Restore => {
            match restore::run_restore().await {
                Ok(out) => output::render(&out, mode),
                Err(e) => e.exit(json_mode),
            }
            return;
        }
        _ => {}
    }

    // All remaining commands need identity + config
    let config_path = cli.config.clone();

    let mut config = match fold_db_node::fold_node::load_node_config(config_path.as_deref(), None) {
        Ok(c) => c,
        Err(e) => {
            CliError::new(format!("Failed to load config: {}", e))
                .with_hint("Check NODE_CONFIG env var or pass --config <path>")
                .exit(json_mode);
        }
    };

    // If identity is missing OR config is incomplete, run the setup wizard.
    // This handles both fresh installs and partial setup failures.
    let needs_setup = config.public_key.is_none();
    if needs_setup {
        if json_mode {
            CliError::new("Not configured")
                .with_hint("Run `folddb` interactively to set up")
                .exit(json_mode);
        }
        config = match commands::setup::run_setup_wizard() {
            Ok(c) => c,
            Err(e) => e.exit(false),
        };
    }

    // Recovery phrase reads from local identity file — no daemon needed
    if let Command::RecoveryPhrase = &cli.command {
        match show_recovery_phrase() {
            Ok(out) => {
                output::render(&out, mode);
                return;
            }
            Err(e) => e.exit(json_mode),
        }
    }

    // Cloud enable/disable modify config directly — no daemon needed
    if let Command::Cloud { action } = &cli.command {
        let result = match action {
            cli::CloudCommand::Enable => cloud_enable(&config, config_path.as_deref()).await,
            cli::CloudCommand::Disable => cloud_disable(config_path.as_deref()),
            cli::CloudCommand::Status | cli::CloudCommand::Sync => {
                // Status and Sync go through daemon HTTP (handled later)
                None
            }
        };
        if let Some(result) = result {
            match result {
                Ok(out) => output::render(&out, mode),
                Err(e) => e.exit(json_mode),
            }
            return;
        }
    }

    // Config set doesn't need daemon
    if let Command::Config {
        action: Some(cli::ConfigCommand::Set { key, value }),
    } = &cli.command
    {
        match commands::system::config_set(key, value, config_path.as_deref()).await {
            Ok(out) => output::render(&out, mode),
            Err(e) => e.exit(json_mode),
        }
        return;
    }

    // Derive user hash from config — error if no public key (incomplete setup)
    let user_hash = cli
        .user_hash
        .clone()
        .or_else(|| std::env::var("FOLD_USER_HASH").ok())
        .unwrap_or_else(|| {
            config
                .public_key
                .as_ref()
                .map(|pk| user_hash_from_pubkey(pk))
                .unwrap_or_else(|| {
                    CliError::new("No public key configured — cannot derive user hash")
                        .with_hint("Run `folddb setup` to configure your node")
                        .exit(json_mode)
                })
        });

    // Data commands go through the daemon HTTP API
    let port = match commands::daemon::ensure_running(dev).await {
        Ok(p) => p,
        Err(e) => e.exit(json_mode),
    };

    let client = FoldDbClient::new(port, &user_hash);

    let result = dispatch_http(
        &cli.command,
        &client,
        &user_hash,
        mode,
        config_path.as_deref(),
    )
    .await;

    match result {
        Ok(out) => output::render(&out, mode),
        Err(e) => e.exit(json_mode),
    }
}

/// Dispatch commands through the daemon HTTP API
async fn dispatch_http(
    command: &Command,
    client: &FoldDbClient,
    _user_hash: &str,
    mode: OutputMode,
    config_path: Option<&str>,
) -> Result<commands::CommandOutput, CliError> {
    match command {
        Command::Schema { action } => dispatch_schema(action, client).await,
        Command::Query {
            schema,
            fields,
            hash,
            range,
        } => {
            let field_list: Vec<String> = fields.split(',').map(|s| s.trim().to_string()).collect();
            let json = client
                .query(schema, &field_list, hash.as_deref(), range.as_deref())
                .await?;
            Ok(commands::CommandOutput::RawJson(json))
        }
        Command::Search { term } => {
            let json = client.search(term).await?;
            Ok(commands::CommandOutput::RawJson(json))
        }
        Command::Mutate { action } => dispatch_mutate(action, client).await,
        Command::Ingest { action } => dispatch_ingest(action, client).await,
        Command::Ask {
            query,
            max_iterations,
        } => {
            let json = client.ask(query, *max_iterations).await?;
            Ok(commands::CommandOutput::RawJson(json))
        }
        Command::Status => {
            let json = client.status().await?;
            Ok(commands::CommandOutput::RawJson(json))
        }
        Command::Config { action } => {
            let action = action.as_ref().unwrap_or(&cli::ConfigCommand::Show);
            match action {
                cli::ConfigCommand::Show => {
                    let json = client.database_config().await?;
                    Ok(commands::CommandOutput::RawJson(json))
                }
                cli::ConfigCommand::Path => {
                    let path = config_path
                        .map(|p| p.to_string())
                        .or_else(|| std::env::var("NODE_CONFIG").ok())
                        .unwrap_or_else(|| "node_config.json".to_string());
                    Ok(commands::CommandOutput::Message(path))
                }
                cli::ConfigCommand::Set { .. } => unreachable!("Handled earlier"),
            }
        }
        Command::Cloud { action } => match action {
            cli::CloudCommand::Status => {
                let config_json = client.database_config().await?;
                let has_cloud =
                    config_json.get("cloud_sync").is_some() && !config_json["cloud_sync"].is_null();

                if !has_cloud {
                    return Ok(commands::CommandOutput::Message(
                        "Cloud sync: disabled".to_string(),
                    ));
                }

                let endpoint = config_json["cloud_sync"]["api_url"]
                    .as_str()
                    .unwrap_or("unknown");

                // Fetch live sync engine status
                let sync_json = client.sync_status().await?;
                let state = sync_json["state"].as_str().unwrap_or("unknown");
                let pending = sync_json["pending_count"].as_u64();
                let encrypted = sync_json["encryption_active"].as_bool().unwrap_or(false);

                let mut msg = format!("Cloud sync: enabled\nEndpoint:   {}", endpoint);
                msg.push_str(&format!("\nState:      {}", state));
                if let Some(count) = pending {
                    msg.push_str(&format!("\nPending:    {} entries", count));
                }
                msg.push_str(&format!(
                    "\nEncryption: {}",
                    if encrypted { "active" } else { "inactive" }
                ));

                Ok(commands::CommandOutput::Message(msg))
            }
            cli::CloudCommand::Sync => {
                let json = client.sync_trigger().await?;
                let message = json["message"]
                    .as_str()
                    .unwrap_or("Sync triggered")
                    .to_string();
                Ok(commands::CommandOutput::Message(message))
            }
            _ => unreachable!("Cloud enable/disable handled before daemon dispatch"),
        },
        Command::RecoveryPhrase => unreachable!("Handled before daemon dispatch"),
        Command::Reset { confirm } => {
            if !confirm {
                if mode == OutputMode::Json {
                    return Err(CliError::new("Database reset requires --confirm flag"));
                }
                let confirmed = dialoguer::Confirm::new()
                    .with_prompt("This will permanently delete all data. Are you sure?")
                    .default(false)
                    .interact()
                    .map_err(|e| CliError::new(format!("Prompt failed: {}", e)))?;
                if !confirmed {
                    return Err(CliError::new("Reset cancelled"));
                }
            }
            client.reset().await?;
            Ok(commands::CommandOutput::Message(
                "Database reset complete".to_string(),
            ))
        }
        Command::Daemon { .. } | Command::Completions { .. } | Command::Restore => {
            unreachable!()
        }
    }
}

async fn dispatch_schema(
    action: &cli::SchemaCommand,
    client: &FoldDbClient,
) -> Result<commands::CommandOutput, CliError> {
    match action {
        cli::SchemaCommand::List => {
            let json = client.schema_list().await?;
            Ok(commands::CommandOutput::RawJson(json))
        }
        cli::SchemaCommand::Get { name } => {
            let json = client.schema_get(name).await?;
            Ok(commands::CommandOutput::RawJson(json))
        }
        cli::SchemaCommand::Approve { name } => {
            client.schema_approve(name).await?;
            Ok(commands::CommandOutput::Message(format!(
                "Schema '{}' approved",
                name
            )))
        }
        cli::SchemaCommand::Block { name } => {
            client.schema_block(name).await?;
            Ok(commands::CommandOutput::Message(format!(
                "Schema '{}' blocked",
                name
            )))
        }
        cli::SchemaCommand::Load => {
            let json = client.schema_load().await?;
            Ok(commands::CommandOutput::RawJson(json))
        }
    }
}

async fn dispatch_mutate(
    action: &cli::MutateCommand,
    client: &FoldDbClient,
) -> Result<commands::CommandOutput, CliError> {
    match action {
        cli::MutateCommand::Run {
            schema,
            r#type,
            fields,
            hash,
            range,
        } => {
            let fields_value: serde_json::Value = serde_json::from_str(fields)
                .map_err(|e| CliError::new(format!("Invalid fields JSON: {}", e)))?;
            let type_str = format!("{:?}", r#type).to_lowercase();
            let json = client
                .mutate(
                    schema,
                    &type_str,
                    &fields_value,
                    hash.as_deref(),
                    range.as_deref(),
                )
                .await?;
            Ok(commands::CommandOutput::RawJson(json))
        }
        cli::MutateCommand::Batch { file } => {
            let input = match file {
                Some(path) => std::fs::read_to_string(path)
                    .map_err(|e| CliError::new(format!("Failed to read file: {}", e)))?,
                None => {
                    use std::io::Read;
                    let mut buf = String::new();
                    std::io::stdin()
                        .read_to_string(&mut buf)
                        .map_err(|e| CliError::new(format!("Failed to read stdin: {}", e)))?;
                    buf
                }
            };
            let mutations: serde_json::Value = serde_json::from_str(&input)
                .map_err(|e| CliError::new(format!("Invalid JSON: {}", e)))?;
            let json = client.mutate_batch(&mutations).await?;
            Ok(commands::CommandOutput::RawJson(json))
        }
    }
}

async fn dispatch_ingest(
    action: &cli::IngestCommand,
    client: &FoldDbClient,
) -> Result<commands::CommandOutput, CliError> {
    match action {
        cli::IngestCommand::File { path } => {
            let input = match path {
                Some(p) => std::fs::read_to_string(p)
                    .map_err(|e| CliError::new(format!("Failed to read file: {}", e)))?,
                None => {
                    use std::io::Read;
                    let mut buf = String::new();
                    std::io::stdin()
                        .read_to_string(&mut buf)
                        .map_err(|e| CliError::new(format!("Failed to read stdin: {}", e)))?;
                    buf
                }
            };
            let data: serde_json::Value = serde_json::from_str(&input)
                .map_err(|e| CliError::new(format!("Invalid JSON: {}", e)))?;
            let json = client.ingest_json(&data).await?;
            Ok(commands::CommandOutput::RawJson(json))
        }
        cli::IngestCommand::SmartScan {
            path,
            max_depth,
            max_files,
        } => {
            let json = client
                .smart_scan(path.to_string_lossy().as_ref(), *max_depth, *max_files)
                .await?;
            Ok(commands::CommandOutput::RawJson(json))
        }
        cli::IngestCommand::Smart {
            path,
            all,
            files,
            no_execute,
        } => {
            if *all {
                let json = client
                    .smart_ingest(path.to_string_lossy().as_ref(), !no_execute)
                    .await?;
                Ok(commands::CommandOutput::RawJson(json))
            } else if let Some(file_list) = files {
                let mut results = Vec::new();
                for file in file_list {
                    let full_path = path.join(file);
                    let json = client
                        .smart_ingest(full_path.to_string_lossy().as_ref(), !no_execute)
                        .await?;
                    results.push(json);
                }
                Ok(commands::CommandOutput::RawJson(serde_json::json!(results)))
            } else {
                Err(CliError::new("Specify --all or --files"))
            }
        }
        #[cfg(target_os = "macos")]
        cli::IngestCommand::AppleNotes { folder, batch_size } => {
            apple::ingest_notes(client, folder.as_deref(), *batch_size).await
        }
        #[cfg(target_os = "macos")]
        cli::IngestCommand::ApplePhotos {
            album,
            limit,
            batch_size,
        } => apple::ingest_photos(client, album.as_deref(), *limit, *batch_size).await,
        #[cfg(target_os = "macos")]
        cli::IngestCommand::AppleReminders { list } => {
            apple::ingest_reminders(client, list.as_deref()).await
        }
    }
}

/// Enable cloud backup — register with Exemem, update config file.
async fn cloud_enable(
    config: &fold_db_node::fold_node::config::NodeConfig,
    config_path: Option<&str>,
) -> Option<Result<commands::CommandOutput, CliError>> {
    if config.database.has_cloud_sync() {
        return Some(Ok(commands::CommandOutput::Message(
            "Cloud backup is already enabled.".to_string(),
        )));
    }

    let invite_code: String = match dialoguer::Input::new()
        .with_prompt("Invite code")
        .interact_text()
    {
        Ok(c) => c,
        Err(e) => return Some(Err(CliError::new(format!("Input cancelled: {}", e)))),
    };

    let pub_key = match &config.public_key {
        Some(k) => k.clone(),
        None => {
            return Some(Err(CliError::new("No public key in config")));
        }
    };
    let pub_key_hex: String = pub_key
        .as_bytes()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();

    let api_url = fold_db_node::endpoints::exemem_api_url();

    eprintln!();
    eprint!("Registering with Exemem...");
    let resp = match commands::setup::register_with_exemem_and_invite(
        &api_url,
        &pub_key_hex,
        Some(&invite_code),
    ) {
        Ok(r) => r,
        Err(e) => return Some(Err(e)),
    };
    eprintln!(" done.");

    let api_key = match resp.api_key {
        Some(k) => k,
        None => return Some(Err(CliError::new("Registration response missing api_key"))),
    };

    // Update config file
    let path = match commands::system::resolve_config_path(config_path) {
        Ok(p) => p,
        Err(e) => return Some(Err(e)),
    };
    let contents = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => return Some(Err(CliError::new(format!("Failed to read config: {}", e)))),
    };
    let mut cfg: serde_json::Value = match serde_json::from_str(&contents) {
        Ok(v) => v,
        Err(e) => return Some(Err(CliError::new(format!("Failed to parse config: {}", e)))),
    };

    cfg["database"]["cloud_sync"] = serde_json::json!({
        "api_url": api_url,
        "api_key": api_key,
        "user_hash": resp.user_hash,
    });

    let updated = serde_json::to_string_pretty(&cfg).unwrap();
    if let Err(e) = std::fs::write(&path, updated) {
        return Some(Err(CliError::new(format!("Failed to write config: {}", e))));
    }

    // Show recovery phrase
    let private_key = config.private_key.as_deref().unwrap_or("");
    let mut msg = "Cloud backup enabled!\n".to_string();
    if let Ok(words) = commands::setup::derive_recovery_phrase(private_key) {
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

    // Mark onboarding complete (consistent with UI and setup wizard)
    if let Ok(home) = fold_db_node::utils::paths::folddb_home() {
        let marker = home.join("data").join(".onboarding_complete");
        if let Some(parent) = marker.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&marker, "1");
    }

    // If daemon is running, offer to restart it so cloud sync starts immediately
    if commands::daemon::read_running_pid().is_some() {
        let restart = dialoguer::Confirm::new()
            .with_prompt("Restart daemon now to activate cloud sync?")
            .default(true)
            .interact()
            .unwrap_or(false);
        if restart {
            let _ = commands::daemon::stop();
            msg.push_str("\nDaemon stopped. Starting with new config...");
            // Start will happen when user runs next command or explicitly starts
            msg.push_str("\nRun `folddb daemon start` to start syncing.");
        } else {
            msg.push_str("\nRestart daemon when ready: folddb daemon stop && folddb daemon start");
        }
    }

    Some(Ok(commands::CommandOutput::Message(msg)))
}

/// Disable cloud backup — remove cloud_sync from config.
fn cloud_disable(config_path: Option<&str>) -> Option<Result<commands::CommandOutput, CliError>> {
    let confirmed = match dialoguer::Confirm::new()
        .with_prompt("Disable cloud backup? Your local data is preserved, but data already synced to Exemem servers will remain there")
        .default(false)
        .interact()
    {
        Ok(c) => c,
        Err(e) => return Some(Err(CliError::new(format!("Input cancelled: {}", e)))),
    };

    if !confirmed {
        return Some(Ok(commands::CommandOutput::Message(
            "Cancelled.".to_string(),
        )));
    }

    let path = match commands::system::resolve_config_path(config_path) {
        Ok(p) => p,
        Err(e) => return Some(Err(e)),
    };
    let contents = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => return Some(Err(CliError::new(format!("Failed to read config: {}", e)))),
    };
    let mut cfg: serde_json::Value = match serde_json::from_str(&contents) {
        Ok(v) => v,
        Err(e) => return Some(Err(CliError::new(format!("Failed to parse config: {}", e)))),
    };

    if let Some(db) = cfg.get_mut("database") {
        if let Some(obj) = db.as_object_mut() {
            obj.remove("cloud_sync");
        }
    }

    let updated = serde_json::to_string_pretty(&cfg).unwrap();
    if let Err(e) = std::fs::write(&path, updated) {
        return Some(Err(CliError::new(format!("Failed to write config: {}", e))));
    }

    let mut msg = "Cloud backup disabled. Your local data is preserved.\nNote: data already synced to Exemem servers is not deleted.".to_string();

    // If daemon is running, offer to restart it so sync stops immediately
    if commands::daemon::read_running_pid().is_some() {
        let restart = dialoguer::Confirm::new()
            .with_prompt("Restart daemon now to stop cloud sync?")
            .default(true)
            .interact()
            .unwrap_or(false);
        if restart {
            let _ = commands::daemon::stop();
            msg.push_str(
                "\nDaemon stopped. Run `folddb daemon start` to restart without cloud sync.",
            );
        } else {
            msg.push_str("\nRestart daemon when ready: folddb daemon stop && folddb daemon start");
        }
    }

    Some(Ok(commands::CommandOutput::Message(msg)))
}

/// Show the 24-word recovery phrase derived from the local identity file.
fn show_recovery_phrase() -> Result<commands::CommandOutput, CliError> {
    let identity_path = fold_db_node::utils::paths::folddb_home()
        .map(|h| h.join("config").join("node_identity.json"))
        .map_err(|e| CliError::new(format!("Cannot find FOLDDB_HOME: {}", e)))?;

    if !identity_path.exists() {
        return Err(CliError::new("No identity found").with_hint("Run `folddb setup` first"));
    }

    let identity_json = std::fs::read_to_string(&identity_path)
        .map_err(|e| CliError::new(format!("Failed to read identity: {}", e)))?;
    let identity: serde_json::Value = serde_json::from_str(&identity_json)
        .map_err(|e| CliError::new(format!("Failed to parse identity: {}", e)))?;
    let private_key = identity["private_key"]
        .as_str()
        .ok_or_else(|| CliError::new("Identity file missing private_key"))?;

    let words = commands::setup::derive_recovery_phrase(private_key)?;

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

    Ok(commands::CommandOutput::Message(msg))
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use fold_db_node::utils::crypto::user_hash_from_pubkey;

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn user_hash_derivation() {
        let key = b64(&[0x42u8; 32]);
        let hash = user_hash_from_pubkey(&key);
        assert_eq!(hash.len(), 32);
        assert_eq!(hash, user_hash_from_pubkey(&key));
    }

    #[test]
    fn user_hash_deterministic() {
        let h1 = user_hash_from_pubkey(&b64(&[0x01u8; 32]));
        let h2 = user_hash_from_pubkey(&b64(&[0x02u8; 32]));
        assert_ne!(h1, h2);
    }
}
