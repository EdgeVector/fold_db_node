#[cfg(target_os = "macos")]
mod apple;
mod cli;
mod client;
mod commands;
mod error;
mod output;
mod restore;
mod update_check;

use base64::Engine;
use clap::Parser;
use cli::{Cli, Command, DaemonCommand};
use client::FoldDbClient;
use error::CliError;
use output::OutputMode;

use fold_db_node::fold_node::config::NodeConfig;
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
                DaemonCommand::Start { port, no_open } => {
                    commands::daemon::start(*port, dev, *no_open)
                        .await
                        .map(commands::CommandOutput::Message)
                }
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
        Command::Setup {
            non_interactive,
            name,
            email,
            birthday,
            invite_code,
            no_cloud,
            ai_provider,
            anthropic_api_key,
            ollama_url,
            ollama_model,
        } => {
            let args = if *non_interactive {
                let name = name.clone().unwrap_or_default();
                if name.trim().is_empty() {
                    CliError::new("--name is required with --non-interactive")
                        .exit(json_mode);
                }
                Some(commands::setup::NonInteractiveSetupArgs {
                    name,
                    email: email.clone(),
                    birthday: birthday.clone(),
                    invite_code: invite_code.clone(),
                    no_cloud: *no_cloud,
                    ai_provider: ai_provider.clone(),
                    anthropic_api_key: anthropic_api_key.clone(),
                    ollama_url: ollama_url.clone(),
                    ollama_model: ollama_model.clone(),
                })
            } else {
                None
            };
            match commands::setup::run_setup_wizard(dev, args).await {
                Ok(_) => output::render(
                    &commands::CommandOutput::Message("Setup complete.".to_string()),
                    mode,
                ),
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

    // Decide whether setup is needed by checking for a persisted node
    // identity. Two sources: the Sled `node_identity` tree at this
    // config's data path (source of truth), or the running daemon's
    // `/api/system/auto-identity` endpoint (useful when the CLI runs
    // against a different $FOLDDB_HOME than the daemon). Without this
    // check, any CLI call with --config or NODE_CONFIG re-enters the
    // interactive wizard, which explodes in non-TTY contexts (CI, cron).
    //
    // Errors from `read_identity_pubkey` (e.g. encrypted blob on disk we
    // can't decrypt) MUST exit cleanly. The previous "swallow Err -> trigger
    // wizard" path was the silent identity-rotation gateway: the wizard
    // would generate a fresh keypair and stamp it over the unreadable one,
    // orphaning every previously-ingested record.
    let data_path = config.get_storage_path();
    let port = commands::daemon::default_port();
    let identity_public_key = match read_identity_pubkey(&data_path) {
        Ok(opt) => opt.or(fetch_pubkey_from_daemon(port).await),
        Err(e) => {
            CliError::new(format!("Cannot read existing node identity: {e}"))
                .with_hint(
                    "An identity exists on disk but this binary cannot decrypt it. \
                     Run from the Tauri app where the OS keychain is reachable, or \
                     export FOLDDB_MASTER_KEY=<64-hex-bytes> if you saved it. \
                     Do NOT run `folddb setup` — that would silently rotate the \
                     identity and orphan all ingested data.",
                )
                .exit(json_mode);
        }
    };

    // If identity is still missing, run the setup wizard (interactive only).
    let needs_setup = identity_public_key.is_none();
    if needs_setup {
        use std::io::IsTerminal;
        if json_mode || !std::io::stdin().is_terminal() {
            // Distinguish "daemon down" from "daemon up but identity not configured
            // yet". The latter happens to every brew user between `daemon start` and
            // completing the web setup wizard — pointing them back at `daemon start`
            // would be a dead-end loop.
            let daemon_up = commands::daemon::check_daemon_health(port).await;
            let hint = if daemon_up {
                format!(
                    "The daemon is running on port {port} but no node identity is \
                     configured yet. Open http://localhost:{port} in a browser to \
                     finish setup, or run `folddb setup` from an interactive terminal."
                )
            } else {
                format!(
                    "Start the daemon (`folddb daemon start`), then open \
                     http://localhost:{port} in a browser to finish setup, or run \
                     `folddb setup` from an interactive terminal."
                )
            };
            CliError::new("Node not configured")
                .with_hint(hint)
                .exit(json_mode);
        }
        config = match commands::setup::run_setup_wizard(dev, None).await {
            Ok(c) => c,
            Err(e) => e.exit(false),
        };
    }

    // Recovery phrase reads from local identity file — no daemon needed
    if let Command::RecoveryPhrase = &cli.command {
        match show_recovery_phrase(&config) {
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
            cli::CloudCommand::DeleteAccount => {
                cloud_delete_account(&config, config_path.as_deref()).await
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

    // Config commands don't need the daemon — show/path/set read or write the
    // local config file directly. Running these without spinning up the daemon
    // is important for setup debugging and for working in non-TTY contexts.
    if let Command::Config { action } = &cli.command {
        let action_ref = action.as_ref().unwrap_or(&cli::ConfigCommand::Show);
        let result = match action_ref {
            cli::ConfigCommand::Show => {
                let daemon_info = commands::system::gather_daemon_info(&config).await;
                commands::system::config_show(&config, config_path.as_deref(), mode, &daemon_info)
            }
            cli::ConfigCommand::Path => {
                let path = config_path
                    .clone()
                    .or_else(|| std::env::var("NODE_CONFIG").ok())
                    .or_else(|| {
                        fold_db_node::utils::paths::folddb_home().ok().map(|h| {
                            h.join("config")
                                .join("node_config.json")
                                .to_string_lossy()
                                .to_string()
                        })
                    })
                    .unwrap_or_else(|| "node_config.json".to_string());
                Ok(commands::CommandOutput::Message(path))
            }
            cli::ConfigCommand::Set { key, value } => {
                commands::system::config_set(key, value, config_path.as_deref()).await
            }
        };
        match result {
            Ok(out) => output::render(&out, mode),
            Err(e) => e.exit(json_mode),
        }
        return;
    }

    // Derive user hash from the identity we loaded (or re-hydrate it now
    // in case the setup wizard just wrote one). Errors if there's still
    // no identity — that's an incomplete setup.
    let user_hash = cli
        .user_hash
        .clone()
        .or_else(|| std::env::var("FOLD_USER_HASH").ok())
        .unwrap_or_else(|| {
            identity_public_key
                .clone()
                .or_else(|| read_identity_pubkey(&data_path).ok().flatten())
                .map(|pk| user_hash_from_pubkey(&pk))
                .unwrap_or_else(|| {
                    CliError::new("No public key configured — cannot derive user hash")
                        .with_hint("Run `folddb setup` to configure your node")
                        .exit(json_mode)
                })
        });

    // Data commands go through the daemon HTTP API
    let port = match commands::daemon::ensure_running().await {
        Ok(p) => p,
        Err(e) => e.exit(json_mode),
    };

    let client = FoldDbClient::new(port, &user_hash);

    let result = dispatch_http(&cli.command, &client, &user_hash, mode).await;

    match result {
        Ok(out) => output::render(&out, mode),
        Err(e) => e.exit(json_mode),
    }
}

/// Format a Unix timestamp as relative time (e.g. "2m ago", "3h ago").
fn format_relative_time(unix_secs: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let diff = now.saturating_sub(unix_secs);
    if diff < 60 {
        "just now".to_string()
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86400 {
        format!("{}h ago", diff / 3600)
    } else {
        format!("{}d ago", diff / 86400)
    }
}

/// Dispatch commands through the daemon HTTP API
async fn dispatch_http(
    command: &Command,
    client: &FoldDbClient,
    user_hash: &str,
    mode: OutputMode,
) -> Result<commands::CommandOutput, CliError> {
    match command {
        Command::Schema { action } => dispatch_schema(action, client, mode).await,
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
            if mode == OutputMode::Json {
                let json = client.status().await?;
                return Ok(commands::CommandOutput::RawJson(json));
            }
            // Human-readable status: gather info from multiple endpoints
            let status = client.status().await.ok();
            let sync = client.sync_status().await.ok();
            let orgs = client.org_list().await.ok();
            let schemas = client.schema_list().await.ok();

            let version = status
                .as_ref()
                .and_then(|s| s.pointer("/data/version").or_else(|| s.get("version")))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");

            let mut msg = format!("FoldDB v{}\n", version);

            // Node key
            msg.push_str(&format!(
                "Node:       {}\n",
                &user_hash[..16.min(user_hash.len())]
            ));

            // Cloud sync
            if let Some(ref s) = sync {
                let enabled = s.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
                if enabled {
                    let state = s.get("state").and_then(|v| v.as_str()).unwrap_or("unknown");
                    let pending = s.get("pending_count").and_then(|v| v.as_u64()).unwrap_or(0);
                    let last_sync = s
                        .get("last_sync_at")
                        .and_then(|v| v.as_u64())
                        .map(format_relative_time);
                    let last_error = s
                        .get("last_error")
                        .and_then(|v| v.as_str())
                        .filter(|e| !e.is_empty());
                    if let Some(err) = last_error {
                        msg.push_str(&format!("Cloud sync: ERROR — {}\n", err));
                    } else if let Some(ref t) = last_sync {
                        msg.push_str(&format!(
                            "Cloud sync: {} ({} pending, synced {})\n",
                            state, pending, t
                        ));
                    } else {
                        msg.push_str(&format!("Cloud sync: {} ({} pending)\n", state, pending));
                    }
                } else {
                    msg.push_str("Cloud sync: disabled\n");
                }
            }

            // Schemas
            if let Some(ref s) = schemas {
                let count = s
                    .pointer("/data/count")
                    .or_else(|| s.get("count"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                msg.push_str(&format!("Schemas:    {}\n", count));
            }

            // Orgs
            if let Some(ref o) = orgs {
                let org_list = o
                    .pointer("/data/orgs")
                    .or_else(|| o.get("orgs"))
                    .and_then(|v| v.as_array());
                if let Some(list) = org_list {
                    if list.is_empty() {
                        msg.push_str("Orgs:       none\n");
                    } else {
                        let names: Vec<&str> = list
                            .iter()
                            .filter_map(|org| org.get("org_name").and_then(|n| n.as_str()))
                            .collect();
                        msg.push_str(&format!("Orgs:       {}\n", names.join(", ")));
                    }
                }
            }

            Ok(commands::CommandOutput::Message(msg.trim_end().to_string()))
        }
        Command::Config { .. } => unreachable!("Config handled before daemon dispatch"),
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

                let last_sync = sync_json["last_sync_at"].as_u64().map(format_relative_time);
                let last_error = sync_json["last_error"].as_str().filter(|e| !e.is_empty());

                let mut msg = format!("Cloud sync: enabled\nEndpoint:   {}", endpoint);
                msg.push_str(&format!("\nState:      {}", state));
                if let Some(count) = pending {
                    msg.push_str(&format!("\nPending:    {} entries", count));
                }
                if let Some(t) = last_sync {
                    msg.push_str(&format!("\nLast sync:  {}", t));
                }
                msg.push_str(&format!(
                    "\nEncryption: {}",
                    if encrypted { "active" } else { "inactive" }
                ));
                if let Some(err) = last_error {
                    msg.push_str(&format!("\nLast error: {}", err));
                }

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
        Command::Snapshot { action } => match action {
            cli::SnapshotCommand::Backup => commands::snapshot::backup(client, mode).await,
            cli::SnapshotCommand::Restore => commands::snapshot::restore(client, mode).await,
        },
        Command::Org { action } => commands::org::dispatch(action, client, mode).await,
        Command::Discovery { action } => dispatch_discovery(action, client, mode).await,
        Command::Trigger { action } => dispatch_trigger(action, client, mode).await,
        Command::RecoveryPhrase => unreachable!("Handled before daemon dispatch"),
        Command::Reset { confirm } => {
            if !confirm {
                if mode == OutputMode::Json {
                    return Err(CliError::new("Database reset requires --confirm flag"));
                }
                let confirmed = dialoguer::Confirm::new()
                    .with_prompt(
                        "This will permanently delete all local data. \
                         If cloud sync is enabled, your remote sync log will also be deleted \
                         (other devices on this account lose unsynced changes on next sync). \
                         Your node identity and org memberships are preserved. \
                         Are you sure?",
                    )
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
        Command::Daemon { .. }
        | Command::Completions { .. }
        | Command::Restore
        | Command::Setup { .. } => {
            unreachable!()
        }
    }
}

async fn dispatch_schema(
    action: &cli::SchemaCommand,
    client: &FoldDbClient,
    mode: OutputMode,
) -> Result<commands::CommandOutput, CliError> {
    match action {
        cli::SchemaCommand::List { show_hash } => {
            commands::schema::list(client, *show_hash, mode).await
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
        cli::SchemaCommand::SetOrg { name, org_hash } => {
            let trimmed = org_hash.trim();
            let org_hash_opt = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            };
            client.schema_set_org(name, org_hash_opt).await?;
            let display = org_hash_opt.unwrap_or("<cleared>");
            Ok(commands::CommandOutput::Message(format!(
                "Schema '{}' org_hash set to {}",
                name, display
            )))
        }
        cli::SchemaCommand::Load => {
            let json = client.schema_load().await?;
            Ok(commands::CommandOutput::RawJson(json))
        }
    }
}

async fn dispatch_discovery(
    action: &cli::DiscoveryCommand,
    client: &FoldDbClient,
    mode: OutputMode,
) -> Result<commands::CommandOutput, CliError> {
    match action {
        cli::DiscoveryCommand::Status => {
            if mode == OutputMode::Json {
                let json = client.discovery_opt_ins().await?;
                return Ok(commands::CommandOutput::RawJson(json));
            }
            // Human-readable: show opt-ins and interests
            let opt_ins = client.discovery_opt_ins().await.ok();
            let interests = client.discovery_interests().await.ok();

            let mut msg = String::new();

            // Opt-ins
            if let Some(ref o) = opt_ins {
                let configs = o
                    .pointer("/data/configs")
                    .or_else(|| o.get("configs"))
                    .and_then(|v| v.as_array());
                match configs {
                    Some(list) if !list.is_empty() => {
                        msg.push_str(&format!("Shared schemas ({}):\n", list.len()));
                        for c in list {
                            let name = c["schema_name"].as_str().unwrap_or("?");
                            let cat = c["category"].as_str().unwrap_or("uncategorized");
                            msg.push_str(&format!("  {} [{}]\n", name, cat));
                        }
                    }
                    _ => msg.push_str("No schemas shared with the discovery network.\n"),
                }
            }

            // Interests
            if let Some(ref i) = interests {
                let categories = i
                    .pointer("/data/categories")
                    .or_else(|| i.get("categories"))
                    .and_then(|v| v.as_array());
                if let Some(cats) = categories {
                    let enabled: Vec<&str> = cats
                        .iter()
                        .filter(|c| c["enabled"].as_bool().unwrap_or(false))
                        .filter_map(|c| c["name"].as_str())
                        .collect();
                    if !enabled.is_empty() {
                        msg.push_str(&format!("\nInterests: {}", enabled.join(", ")));
                    }
                }
            }

            if msg.is_empty() {
                msg = "Discovery not configured. Enable cloud backup first.".to_string();
            }

            Ok(commands::CommandOutput::Message(msg.trim_end().to_string()))
        }
        cli::DiscoveryCommand::Publish => {
            let json = client.discovery_publish().await?;
            if mode == OutputMode::Json {
                return Ok(commands::CommandOutput::RawJson(json));
            }
            let published = json
                .pointer("/data/published_count")
                .or_else(|| json.get("published_count"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            Ok(commands::CommandOutput::Message(format!(
                "Published {} schema(s) to the discovery network.",
                published
            )))
        }
    }
}

async fn dispatch_trigger(
    action: &cli::TriggerCommand,
    client: &FoldDbClient,
    mode: OutputMode,
) -> Result<commands::CommandOutput, CliError> {
    match action {
        cli::TriggerCommand::Log { view, last, limit } => {
            commands::trigger::log(client, view, last, *limit, mode).await
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

/// Start a smart-folder scan and poll until it completes, returning the full
/// `SmartFolderScanResponse` JSON. The scan endpoint is async — POST returns a
/// `progress_id` and the result is fetched separately.
async fn run_smart_scan(
    client: &FoldDbClient,
    folder_path: &str,
    max_depth: usize,
    max_files: usize,
) -> Result<serde_json::Value, CliError> {
    let start = client.smart_scan(folder_path, max_depth, max_files).await?;
    let progress_id = start
        .get("progress_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CliError::new("Scan start response missing 'progress_id'"))?
        .to_string();

    // 10-minute cap matches the per-request timeout in `FoldDbClient::new`.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
    while std::time::Instant::now() < deadline {
        if let Some(result) = client.smart_scan_result(&progress_id).await? {
            return Ok(result);
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    Err(CliError::new(
        "Smart-folder scan timed out after 10 minutes",
    ))
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
            let folder = path.to_string_lossy();
            let scan = run_smart_scan(client, &folder, *max_depth, *max_files).await?;
            Ok(commands::CommandOutput::RawJson(scan))
        }
        cli::IngestCommand::Smart {
            path,
            all,
            files,
            no_execute,
        } => {
            let folder = path.to_string_lossy();
            let auto_execute = !*no_execute;
            let files_to_ingest: Vec<String> = if *all {
                let scan = run_smart_scan(client, &folder, 5, 500).await?;
                let recs = scan
                    .get("recommended_files")
                    .and_then(|v| v.as_array())
                    .ok_or_else(|| {
                        CliError::new("Scan response missing 'recommended_files' array")
                    })?;
                recs.iter()
                    .filter_map(|r| r.get("path").and_then(|v| v.as_str()).map(String::from))
                    .collect()
            } else if let Some(file_list) = files {
                file_list.clone()
            } else {
                return Err(CliError::new("Specify --all or --files"));
            };
            if files_to_ingest.is_empty() {
                return Err(CliError::new(
                    "No files to ingest (scan returned no recommendations)",
                ));
            }
            let json = client
                .smart_ingest(&folder, &files_to_ingest, auto_execute)
                .await?;
            Ok(commands::CommandOutput::RawJson(json))
        }
        #[cfg(target_os = "macos")]
        cli::IngestCommand::AppleNotes {
            folder,
            batch_size,
            timeout_seconds,
        } => apple::ingest_notes(client, folder.as_deref(), *batch_size, *timeout_seconds).await,
        #[cfg(target_os = "macos")]
        cli::IngestCommand::ApplePhotos {
            album,
            limit,
            batch_size,
            timeout_seconds,
        } => {
            apple::ingest_photos(
                client,
                album.as_deref(),
                *limit,
                *batch_size,
                *timeout_seconds,
            )
            .await
        }
        #[cfg(target_os = "macos")]
        cli::IngestCommand::AppleReminders {
            list,
            batch_size,
            timeout_seconds,
        } => apple::ingest_reminders(client, list.as_deref(), *batch_size, *timeout_seconds).await,
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

    let id = match fold_db_node::identity::load_standalone(&config.get_storage_path()) {
        Ok(Some(id)) => id,
        Ok(None) => {
            return Some(Err(
                CliError::new("No node identity found").with_hint("Run `folddb setup` first")
            ));
        }
        Err(e) => {
            return Some(Err(CliError::new(format!(
                "Failed to read node identity: {e}"
            ))));
        }
    };
    let pub_key_b64 = id.public_key.clone();
    let private_key_b64 = id.private_key.clone();
    let pub_key_bytes = match base64::engine::general_purpose::STANDARD.decode(&pub_key_b64) {
        Ok(b) => b,
        Err(e) => {
            return Some(Err(CliError::new(format!(
                "Failed to decode public key: {}",
                e
            ))));
        }
    };
    let pub_key_hex: String = pub_key_bytes.iter().map(|b| format!("{:02x}", b)).collect();

    let api_url = fold_db_node::endpoints::exemem_api_url();

    eprintln!();
    eprint!("Registering with Exemem...");
    let resp = match commands::setup::register_with_exemem_and_invite(
        &api_url,
        &pub_key_hex,
        &private_key_b64,
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

    // Persist the cloud-sync config and the api_key separately so the api_key
    // never lands in `node_config.json`. See `persist_cloud_enable_state`.
    let path = match commands::system::resolve_config_path(config_path) {
        Ok(p) => p,
        Err(e) => return Some(Err(e)),
    };
    if let Err(e) = persist_cloud_enable_state(&path, &api_url, &api_key, resp.user_hash.clone()) {
        return Some(Err(e));
    }

    // Show recovery phrase (identity already loaded above for registration)
    let mut msg = "Cloud backup enabled!\n".to_string();
    if let Ok(words) = commands::setup::derive_recovery_phrase(&private_key_b64) {
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

    // Mark onboarding complete (consistent with UI and setup wizard). Routed
    // through sensitive_io: the marker's presence reveals a credentialed flow
    // ran (PR #885 reasoning).
    if let Ok(home) = fold_db_node::utils::paths::folddb_home() {
        let marker = home.join("data").join(".onboarding_complete");
        let _ = fold_db_node::sensitive_io::write_sensitive(&marker, b"1");
    }

    // If daemon is running, apply config live via HTTP (same mechanism the UI uses)
    if commands::daemon::read_running_pid().is_some() {
        let port = commands::daemon::default_port();
        let user_hash_str = fold_db_node::utils::crypto::user_hash_from_pubkey(&pub_key_b64);
        let client = FoldDbClient::new(port, &user_hash_str);
        match client
            .apply_setup(&serde_json::json!({
                "type": "exemem",
                "api_url": api_url,
                "api_key": api_key,
            }))
            .await
        {
            Ok(_) => {
                msg.push_str("\nCloud sync activated — syncing will start shortly.");
            }
            Err(e) => {
                msg.push_str(&format!("\nConfig saved but failed to apply live: {}", e));
                msg.push_str(
                    "\nRestart daemon to activate: folddb daemon stop && folddb daemon start",
                );
            }
        }
    }

    Some(Ok(commands::CommandOutput::Message(msg)))
}

/// Persist the cloud-enable result.
///
/// Splits the registration response across two stores so the api_key never
/// lands in plaintext on disk:
/// 1. **`node_config.json`** — typed [`NodeConfig`] save. `CloudSyncConfig`'s
///    `api_key` / `session_token` / `user_hash` are `#[serde(skip_serializing)]`,
///    so the typed serializer drops them automatically. `api_url` is the only
///    cloud-sync field that ends up on disk here.
/// 2. **`credentials.json` / `credentials.enc`** — [`fold_db_node::keychain::store_credentials`].
///    The api_key lives here only: encrypted under `os-keychain`, 0o600
///    plaintext otherwise. `FoldNode::new` rehydrates `cloud_sync.api_key`
///    from this store on every daemon boot. `session_token` is left empty
///    because the CLI register endpoint doesn't return one — the daemon's
///    auth-refresh path mints a fresh one on first sync.
///
/// Mirrors the persistence model `restore.rs::try_register_and_configure`
/// established and that PR #885 hardened in `write_bootstrap_marker`.
fn persist_cloud_enable_state(
    config_path: &str,
    api_url: &str,
    api_key: &str,
    user_hash: Option<String>,
) -> Result<(), CliError> {
    use fold_db::storage::{CloudSyncConfig, DatabaseConfig};

    let mut config = fold_db_node::fold_node::config::load_node_config(Some(config_path), None)
        .map_err(|e| CliError::new(format!("Failed to read config: {}", e)))?;

    config.database = DatabaseConfig::with_cloud_sync(
        config.database.path.clone(),
        CloudSyncConfig {
            api_url: api_url.to_string(),
            api_key: api_key.to_string(),
            session_token: None,
            user_hash: user_hash.clone(),
            p2p_sync: None,
        },
    );

    fold_db_node::fold_node::config::save_node_config(&config)
        .map_err(|e| CliError::new(format!("Failed to write config: {}", e)))?;

    let creds = fold_db_node::keychain::ExememCredentials {
        user_hash: user_hash.unwrap_or_default(),
        session_token: String::new(),
        api_key: api_key.to_string(),
    };
    fold_db_node::keychain::store_credentials(&creds)
        .map_err(|e| CliError::new(format!("Failed to persist credentials: {}", e)))?;

    Ok(())
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

/// Delete Exemem account and all cloud data.
async fn cloud_delete_account(
    config: &fold_db_node::fold_node::config::NodeConfig,
    config_path: Option<&str>,
) -> Option<Result<commands::CommandOutput, CliError>> {
    if !config.database.has_cloud_sync() {
        return Some(Ok(commands::CommandOutput::Message(
            "No cloud account configured.".to_string(),
        )));
    }

    eprintln!("\x1b[31mWARNING: This will permanently delete your Exemem account\x1b[0m");
    eprintln!("  - All cloud backup data will be purged");
    eprintln!("  - All API keys will be revoked");
    eprintln!("  - Your local data will NOT be affected");
    eprintln!();

    let confirmed = dialoguer::Confirm::new()
        .with_prompt("Are you sure you want to delete your Exemem account?")
        .default(false)
        .interact()
        .unwrap_or(false);

    if !confirmed {
        return Some(Ok(commands::CommandOutput::Message(
            "Cancelled.".to_string(),
        )));
    }

    // Double confirmation for destructive action
    let typed: String = match dialoguer::Input::new()
        .with_prompt("Type DELETE to confirm")
        .interact_text()
    {
        Ok(t) => t,
        Err(_) => {
            return Some(Ok(commands::CommandOutput::Message(
                "Cancelled.".to_string(),
            )))
        }
    };

    if typed.trim() != "DELETE" {
        return Some(Ok(commands::CommandOutput::Message(
            "Cancelled — you must type DELETE exactly.".to_string(),
        )));
    }

    // Call Exemem API to delete account
    let api_url = fold_db_node::endpoints::exemem_api_url();

    // Get API key from credentials file for authentication
    let api_key = match fold_db_node::keychain::load_credentials() {
        Ok(Some(creds)) if !creds.api_key.is_empty() => creds.api_key,
        _ => {
            return Some(Err(CliError::new(
                "No credentials found. Cloud account may already be deleted.",
            )))
        }
    };

    eprint!("Deleting account...");
    // trace-egress: propagate (Exemem auth Lambda; .send() wrapped with inject_w3c below)
    let http = reqwest::Client::new();
    let delete_request = observability::propagation::inject_w3c(
        http.delete(format!("{}/api/auth/account", api_url))
            .header("X-API-Key", &api_key),
    );
    let resp = delete_request.send().await;

    match resp {
        Ok(r) if r.status().is_success() => {
            eprintln!(" done.");

            // Purge cloud storage (R2/B2) via the storage_admin_service Lambda.
            // Fail LOUDLY if the purge errors — we previously swallowed this and
            // told the user "All cloud data purged" while objects remained.
            eprint!("Purging cloud storage...");
            let purge_request = observability::propagation::inject_w3c(
                http.post(format!("{}/api/storage-admin/purge-account", api_url))
                    .header("X-API-Key", &api_key),
            );
            let purge_resp = purge_request.send().await;
            let purge_body: serde_json::Value = match purge_resp {
                Ok(pr) => {
                    let status = pr.status();
                    let text = pr.text().await.unwrap_or_default();
                    if !status.is_success() {
                        eprintln!(" FAILED");
                        return Some(Err(CliError::new(format!(
                            "Cloud storage purge failed (HTTP {}): {}. Your Exemem account row was deleted but cloud objects may remain. Contact support.",
                            status, text
                        ))));
                    }
                    match serde_json::from_str(&text) {
                        Ok(v) => v,
                        Err(e) => {
                            eprintln!(" FAILED");
                            return Some(Err(CliError::new(format!(
                                "Cloud storage purge returned unparseable response: {} (body: {})",
                                e, text
                            ))));
                        }
                    }
                }
                Err(e) => {
                    eprintln!(" FAILED");
                    return Some(Err(CliError::new(format!(
                        "Cloud storage purge network error: {}. Your Exemem account row was deleted but cloud objects may remain. Contact support.",
                        e
                    ))));
                }
            };

            // Check server-reported ok flag: storage_admin_service returns
            // `ok: false` when individual object deletes failed.
            let ok = purge_body
                .get("ok")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let deleted = purge_body
                .get("deleted_objects")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            if !ok {
                eprintln!(" FAILED");
                let failed_count = purge_body
                    .get("failed_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                return Some(Err(CliError::new(format!(
                    "Cloud storage purge incomplete: deleted {} objects, {} deletes failed. Your Exemem account row was deleted but some cloud objects remain. Contact support. Server response: {}",
                    deleted, failed_count, purge_body
                ))));
            }
            eprintln!(" done ({} objects).", deleted);

            // Disable cloud locally — also fail loud if this errors.
            if let Some(Err(e)) = cloud_disable(config_path) {
                return Some(Err(CliError::new(format!(
                    "Cloud purge succeeded but local cloud-disable failed: {}. Run `folddb cloud disable` manually.",
                    e
                ))));
            }

            Some(Ok(commands::CommandOutput::Message(
                "Account deleted. All cloud data purged.\nLocal data is preserved.".to_string(),
            )))
        }
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            Some(Err(CliError::new(format!(
                "Account deletion failed (HTTP {}): {}",
                status, body
            ))))
        }
        Err(e) => Some(Err(CliError::new(format!("Network error: {}", e)))),
    }
}

/// Show the 24-word recovery phrase derived from the local identity.
///
/// Reads the node identity from the Sled `node_identity` tree (see
/// `fold_db_node::identity::IdentityStore`). Works without the daemon
/// running because `load_standalone` opens a short-lived pool and
/// releases the flock on return.
fn show_recovery_phrase(config: &NodeConfig) -> Result<commands::CommandOutput, CliError> {
    let data_path = config.get_storage_path();
    let id = fold_db_node::identity::load_standalone(&data_path)
        .map_err(|e| CliError::new(format!("Failed to read identity: {}", e)))?
        .ok_or_else(|| CliError::new("No identity found").with_hint("Run `folddb setup` first"))?;

    let words = commands::setup::derive_recovery_phrase(&id.private_key)?;

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

/// Read the node's public key from the Sled identity tree at this
/// node's data path.
///
/// `Ok(None)` means "no identity has been persisted yet" — callers fall
/// back to the daemon / setup wizard paths. Sled lock contention (the
/// daemon already holds the file lock) ALSO returns `Ok(None)` — that's
/// expected when the CLI runs alongside a live daemon, and the caller's
/// `fetch_pubkey_from_daemon` fallback handles it.
///
/// `Err` is reserved for the decryption-failure case: an identity
/// exists on disk but this binary can't read it. Callers MUST surface
/// that — the setup wizard would otherwise silently rotate the identity
/// and orphan every previously-ingested record.
fn read_identity_pubkey(data_path: &std::path::Path) -> Result<Option<String>, String> {
    match fold_db_node::identity::load_standalone(data_path) {
        Ok(opt) => Ok(opt.map(|id| id.public_key)),
        Err(e) if is_decrypt_error(&e) => Err(e),
        // Sled lock busy / IO / "tree not found" / etc — the legacy fall-through
        // path. The caller's daemon-HTTP probe is the canonical recovery for
        // these.
        Err(_) => Ok(None),
    }
}

/// Heuristic: was this `load_standalone` failure a decryption / master-key
/// problem (the silent-rotation gateway), or just transient Sled IO (which
/// the daemon owns the canonical answer for)?
///
/// We only ever construct these specific error strings from
/// [`fold_db_node::identity::open`] / [`fold_db_node::secure_store`],
/// so substring matching is sufficient. Callers stay safe even when this
/// returns false on an unrecognized decrypt error: `load_or_generate` and
/// `FoldNode::resolve_identity` enforce the same invariant downstream and
/// also refuse to silently rotate.
fn is_decrypt_error(msg: &str) -> bool {
    msg.contains("Encrypted node identity")
        || msg.contains("keychain master key")
        || msg.contains("Failed to decrypt private key")
        || msg.contains("FOLDDB_MASTER_KEY")
}

/// Ask a running daemon for the node's public key via `/api/system/auto-identity`.
///
/// Short timeout (2s) — if the daemon isn't responding we fall through to the
/// setup wizard (or the non-TTY error) rather than hanging the CLI.
async fn fetch_pubkey_from_daemon(port: u16) -> Option<String> {
    let url = format!("http://127.0.0.1:{}/api/system/auto-identity", port);
    // trace-egress: loopback (CLI -> local daemon; .send() wrapped with inject_w3c below)
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .ok()?;
    let request = observability::propagation::inject_w3c(client.get(&url));
    let resp = request.send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: serde_json::Value = resp.json().await.ok()?;
    body.get("public_key")
        .and_then(|v| v.as_str())
        .map(String::from)
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

    #[test]
    fn read_identity_pubkey_missing_store_returns_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        // No Sled tree has been written at this path yet.
        assert_eq!(super::read_identity_pubkey(dir.path()).unwrap(), None);
    }

    #[test]
    fn read_identity_pubkey_reads_from_sled_standalone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let id = fold_db_node::identity::NodeIdentity {
            private_key: "priv".to_string(),
            public_key: "pk-from-sled".to_string(),
        };
        fold_db_node::identity::save_standalone(dir.path(), &id).expect("save");
        assert_eq!(
            super::read_identity_pubkey(dir.path()).unwrap().as_deref(),
            Some("pk-from-sled")
        );
    }

    /// FOLDDB_HOME is a process-global env var. Tests that mutate it must
    /// serialize on this mutex. Binary tests run in their own process
    /// (independent of the lib crate's test binary), so a process-local
    /// `OnceLock<Mutex>` is sufficient — no need to share with the lib's
    /// `secure_store::test_master_key::ENV_LOCK`.
    fn folddb_home_lock() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::{Mutex, OnceLock};
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    /// `persist_cloud_enable_state` must split the registration result so
    /// that `node_config.json` ends up with NO `api_key`, while the api_key
    /// is reachable via [`fold_db_node::keychain::load_credentials`]. This
    /// is the regression guard against the raw-`serde_json::Value` shortcut
    /// that previously persisted the api_key in plaintext on disk.
    ///
    /// Plaintext credentials path only — `os-keychain` writes ciphertext via
    /// the OS keychain, which CI doesn't have. The split is identical in
    /// both modes; only the on-disk bytes of `credentials.{json,enc}` differ.
    #[cfg(not(feature = "os-keychain"))]
    #[test]
    fn persist_cloud_enable_state_strips_api_key_from_node_config_and_stores_it_separately() {
        let _guard = folddb_home_lock();
        let tmp = tempfile::tempdir().expect("tempdir");
        std::env::set_var("FOLDDB_HOME", tmp.path());

        // Pre-seed a local-only config so `persist_cloud_enable_state` has
        // something to load and update — mirrors the real CLI flow where
        // `cloud enable` runs after `setup` has already written the file.
        let config_dir = tmp.path().join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        let config_path = config_dir.join("node_config.json");
        let initial = serde_json::json!({
            "database": { "path": tmp.path().join("data").to_string_lossy() },
            "network_listen_address": "/ip4/0.0.0.0/tcp/0"
        });
        std::fs::write(
            &config_path,
            serde_json::to_string_pretty(&initial).unwrap(),
        )
        .expect("seed node_config.json");

        super::persist_cloud_enable_state(
            config_path.to_str().unwrap(),
            "https://api.example.test",
            "secret-cloud-api-key",
            Some("user-hash-abc".to_string()),
        )
        .expect("persist_cloud_enable_state");

        // (a) On-disk node_config.json must omit api_key (and session_token,
        // user_hash) thanks to `CloudSyncConfig`'s `skip_serializing` markers.
        // api_url is the only cloud_sync field we expect to round-trip.
        let on_disk: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&config_path).expect("read config"))
                .expect("parse rewritten config as JSON");
        let cloud = on_disk
            .get("database")
            .and_then(|d| d.get("cloud_sync"))
            .expect("cloud_sync present after enable");
        assert!(
            cloud.get("api_key").is_none(),
            "api_key MUST be absent from on-disk node_config.json; got: {cloud}"
        );
        assert!(
            cloud.get("session_token").is_none(),
            "session_token MUST be absent from on-disk node_config.json; got: {cloud}"
        );
        assert!(
            cloud.get("user_hash").is_none(),
            "user_hash MUST be absent from on-disk node_config.json; got: {cloud}"
        );
        assert_eq!(
            cloud.get("api_url").and_then(|v| v.as_str()),
            Some("https://api.example.test"),
            "api_url should round-trip through node_config.json"
        );

        // (b) The api_key must be retrievable via the encrypted credential
        // store — this is what `FoldNode::new` reads on every daemon boot.
        let creds = fold_db_node::keychain::load_credentials()
            .expect("load_credentials")
            .expect("credentials should exist after persist_cloud_enable_state");
        assert_eq!(creds.api_key, "secret-cloud-api-key");
        assert_eq!(creds.user_hash, "user-hash-abc");
        assert_eq!(
            creds.session_token, "",
            "CLI register doesn't return a session_token; daemon refreshes lazily"
        );

        std::env::remove_var("FOLDDB_HOME");
    }
}
