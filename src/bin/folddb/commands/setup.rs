//! CLI setup wizard — a thin REST client around `POST /api/setup/bootstrap`.
//!
//! The CLI collects user prompts (interactively or via `--non-interactive`
//! flags) and forwards them to the daemon. Identity minting, encrypted Sled
//! writes, Exemem registration, and the `node_config.json` write all happen
//! on the server. This wrapper exists only to drive the prompts and surface
//! the recovery phrase in the terminal — keeping a single canonical bootstrap
//! path (the one /api/setup/bootstrap defines) means brew users and Tauri
//! users get identical behavior, and the master-key-never-minted bug from
//! the legacy in-process flow can't come back.
//!
//! The shared signing / registration helpers live in
//! [`fold_db_node::handlers::setup`]; thin `CliError`-friendly re-exports
//! below keep `cloud enable` and `restore` working without touching them.
use crate::commands::daemon;
use crate::error::CliError;
use dialoguer::{Confirm, Input};
use fold_db_node::fold_node::config::NodeConfig;
use fold_db_node::handlers::setup as shared;
use fold_db_node::trust::identity_card::IdentityCard;
use serde::{Deserialize, Serialize};

/// Re-export so existing call sites (`cloud enable`, `restore`) keep their
/// import paths.
pub use shared::ExememRegisterResponse;

// ---------------------------------------------------------------------------
// Thin CliError wrappers around the shared handlers::setup helpers.
// ---------------------------------------------------------------------------

pub fn register_with_exemem(
    api_url: &str,
    public_key_hex: &str,
    private_key_b64: &str,
) -> Result<ExememRegisterResponse, CliError> {
    shared::register_with_exemem_and_invite(api_url, public_key_hex, private_key_b64, None)
        .map_err(CliError::new)
}

pub fn register_with_exemem_and_invite(
    api_url: &str,
    public_key_hex: &str,
    private_key_b64: &str,
    invite_code: Option<&str>,
) -> Result<ExememRegisterResponse, CliError> {
    shared::register_with_exemem_and_invite(api_url, public_key_hex, private_key_b64, invite_code)
        .map_err(CliError::new)
}

pub fn derive_recovery_phrase(private_key_base64: &str) -> Result<Vec<String>, CliError> {
    shared::derive_recovery_phrase(private_key_base64).map_err(CliError::new)
}

// ---------------------------------------------------------------------------
// Non-interactive flag bag.
// ---------------------------------------------------------------------------

/// Pre-supplied answers for `folddb setup --non-interactive` (CI / scripting).
///
/// `name` is the only mandatory field. `invite_code` non-empty implies cloud
/// is enabled unless `no_cloud` is set; if both `invite_code` is empty and
/// `no_cloud` is false, we still bootstrap local-only (no cloud).
#[derive(Debug, Clone)]
pub struct NonInteractiveSetupArgs {
    pub name: String,
    pub email: Option<String>,
    pub birthday: Option<String>,
    pub invite_code: Option<String>,
    pub no_cloud: bool,
    /// One of `"anthropic"`, `"ollama"`, `"skip"`, or `None` (skip).
    pub ai_provider: Option<String>,
    pub anthropic_api_key: Option<String>,
    pub ollama_url: Option<String>,
    pub ollama_model: Option<String>,
}

// ---------------------------------------------------------------------------
// Wire types — kept private so callers go through `run_setup_wizard`.
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct BootstrapRequestBody {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    birthday: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ai_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    anthropic_api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ollama_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ollama_model: Option<String>,
    enable_cloud: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    invite_code: Option<String>,
}

#[derive(Deserialize)]
struct BootstrapResponseBody {
    #[serde(default)]
    recovery_phrase: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Public entry point.
// ---------------------------------------------------------------------------

/// Run the setup wizard against the local daemon's `/api/setup/bootstrap`
/// endpoint. Starts the daemon if it isn't already running.
///
/// `dev` mirrors the global `--dev` flag and is forwarded to `daemon::start`.
/// `non_interactive` provides pre-filled answers; when `None`, the wizard
/// prompts via `dialoguer` (same UX as before this rewrite).
pub async fn run_setup_wizard(
    dev: bool,
    non_interactive: Option<NonInteractiveSetupArgs>,
) -> Result<NodeConfig, CliError> {
    let port = ensure_daemon_for_setup(dev).await?;

    let body = match non_interactive {
        Some(args) => build_request_from_args(args)?,
        None => build_request_interactive()?,
    };

    let resp = post_bootstrap(port, &body).await?;
    if let Some(words) = resp.recovery_phrase.as_ref() {
        print_recovery_phrase(words);
    }

    // The server wrote node_config.json; reload it so main.rs can keep going.
    let config_path = fold_db_node::utils::paths::folddb_home()
        .map_err(|e| CliError::new(format!("Cannot resolve FOLDDB_HOME: {}", e)))?
        .join("config")
        .join("node_config.json");
    let config = fold_db_node::fold_node::load_node_config(
        Some(config_path.to_string_lossy().as_ref()),
        None,
    )
    .map_err(|e| {
        CliError::new(format!(
            "Bootstrap succeeded but reloading config failed: {}",
            e
        ))
    })?;

    eprintln!("Config saved to {}", config_path.display());
    eprintln!();

    Ok(config)
}

// ---------------------------------------------------------------------------
// Daemon orchestration.
// ---------------------------------------------------------------------------

/// Make sure a daemon is reachable on the resolved port, spawning one if not.
///
/// This is the only path in the CLI that's allowed to auto-spawn a daemon —
/// see the safety note on `daemon::ensure_running`. The setup wizard runs
/// only when there is no identity yet, so there's nothing to corrupt.
async fn ensure_daemon_for_setup(dev: bool) -> Result<u16, CliError> {
    let port = daemon::default_port();
    if daemon::check_daemon_health(port).await {
        return Ok(port);
    }
    // Suppress the browser open — the CLI is driving setup, not the web UI.
    let _ = daemon::start(port, dev, /* no_open */ true).await?;
    Ok(port)
}

// ---------------------------------------------------------------------------
// Prompt collection.
// ---------------------------------------------------------------------------

fn build_request_interactive() -> Result<BootstrapRequestBody, CliError> {
    eprintln!();
    eprintln!("Welcome to FoldDB!");
    eprintln!();

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
    eprintln!("This info is synced with your other devices restored from");
    eprintln!("the same recovery phrase. It's never uploaded in plaintext —");
    eprintln!("the sync log is end-to-end encrypted with keys derived from");
    eprintln!("your recovery phrase.");
    eprintln!();

    eprintln!("Configure AI for data ingestion:");
    let ai_providers = &["Anthropic (cloud)", "Ollama (local)", "Skip for now"];
    let ai_idx = dialoguer::Select::new()
        .with_prompt("AI provider")
        .items(ai_providers)
        .default(0)
        .interact()
        .map_err(|e| CliError::new(format!("Input cancelled: {}", e)))?;

    let mut ai_provider: Option<String> = None;
    let mut anthropic_api_key: Option<String> = None;
    let mut ollama_url: Option<String> = None;
    let mut ollama_model: Option<String> = None;
    match ai_idx {
        0 => {
            let key: String = Input::new()
                .with_prompt("Anthropic API key")
                .interact_text()
                .map_err(|e| CliError::new(format!("Input cancelled: {}", e)))?;
            ai_provider = Some("anthropic".to_string());
            anthropic_api_key = Some(key);
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
            ai_provider = Some("ollama".to_string());
            ollama_url = Some(url);
            ollama_model = Some(model);
        }
        _ => {}
    }
    eprintln!();

    let enable_cloud = Confirm::new()
        .with_prompt("Enable cloud backup?")
        .default(false)
        .interact()
        .map_err(|e| CliError::new(format!("Input cancelled: {}", e)))?;

    let invite_code = if enable_cloud {
        let code: String = Input::new()
            .with_prompt("Invite code")
            .interact_text()
            .map_err(|e| CliError::new(format!("Input cancelled: {}", e)))?;
        Some(code)
    } else {
        None
    };

    Ok(BootstrapRequestBody {
        name,
        email,
        birthday,
        ai_provider,
        anthropic_api_key,
        ollama_url,
        ollama_model,
        enable_cloud,
        invite_code,
    })
}

fn build_request_from_args(
    args: NonInteractiveSetupArgs,
) -> Result<BootstrapRequestBody, CliError> {
    let name = args.name.trim().to_string();
    if name.is_empty() {
        return Err(CliError::new("--name is required in non-interactive mode"));
    }
    if let Some(b) = args.birthday.as_deref() {
        IdentityCard::validate_birthday(b).map_err(CliError::new)?;
    }
    let invite_present = args
        .invite_code
        .as_deref()
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    let enable_cloud = invite_present && !args.no_cloud;
    if enable_cloud && args.invite_code.as_deref().unwrap_or("").is_empty() {
        return Err(CliError::new(
            "--invite-code is required when cloud backup is enabled",
        ));
    }

    let ai_provider = args.ai_provider.as_deref().map(|s| s.to_lowercase());
    if let Some(provider) = ai_provider.as_deref() {
        match provider {
            "anthropic" => {
                if args.anthropic_api_key.as_deref().unwrap_or("").is_empty() {
                    return Err(CliError::new(
                        "--anthropic-api-key is required when --ai-provider=anthropic",
                    ));
                }
            }
            "ollama" | "skip" | "none" | "" => {}
            other => {
                return Err(CliError::new(format!(
                    "Unknown --ai-provider {other:?}; expected anthropic, ollama, or skip"
                )));
            }
        }
    }

    Ok(BootstrapRequestBody {
        name,
        email: args.email.filter(|s| !s.is_empty()),
        birthday: args.birthday.filter(|s| !s.is_empty()),
        ai_provider,
        anthropic_api_key: args.anthropic_api_key.filter(|s| !s.is_empty()),
        ollama_url: args.ollama_url.filter(|s| !s.is_empty()),
        ollama_model: args.ollama_model.filter(|s| !s.is_empty()),
        enable_cloud,
        invite_code: if enable_cloud { args.invite_code } else { None },
    })
}

// ---------------------------------------------------------------------------
// HTTP roundtrip.
// ---------------------------------------------------------------------------

async fn post_bootstrap(
    port: u16,
    body: &BootstrapRequestBody,
) -> Result<BootstrapResponseBody, CliError> {
    let url = format!("http://127.0.0.1:{}/api/setup/bootstrap", port);
    // trace-egress: loopback — CLI → local daemon /api/setup/bootstrap; the
    // server is the trace-root for setup, so no W3C propagation needed.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| CliError::new(format!("Failed to build HTTP client: {}", e)))?;

    eprint!("Bootstrapping node...");
    let resp = client
        .post(&url)
        .json(body)
        .send()
        .await
        .map_err(|e| CliError::new(format!("Failed to reach local daemon: {}", e)))?;

    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| CliError::new(format!("Failed to read bootstrap response: {}", e)))?;

    if status == reqwest::StatusCode::GONE {
        return Err(CliError::new(
            "This node is already bootstrapped — `folddb setup` is one-shot.",
        )
        .with_hint(
            "Use `folddb restore` to recover from a 24-word phrase, or remove the data dir to start fresh.",
        ));
    }

    if !status.is_success() {
        let msg = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(String::from))
            .unwrap_or(text);
        return Err(CliError::new(format!(
            "Bootstrap failed (HTTP {}): {}",
            status, msg
        )));
    }

    eprintln!(" done.");
    let parsed: BootstrapResponseBody = serde_json::from_str(&text)
        .map_err(|e| CliError::new(format!("Failed to parse bootstrap response: {}", e)))?;
    Ok(parsed)
}

// ---------------------------------------------------------------------------
// Recovery phrase display — same layout as before the REST rewrite.
// ---------------------------------------------------------------------------

fn print_recovery_phrase(words: &[String]) {
    eprintln!();
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `--no-cloud` overrides a present invite code: the request must go
    /// out as `enable_cloud: false` and the invite must be dropped, not
    /// shadow-leaked into the body.
    #[test]
    fn build_request_from_args_no_cloud_flag_disables_cloud() {
        let args = NonInteractiveSetupArgs {
            name: "Tom".into(),
            email: Some("tom@example.com".into()),
            birthday: None,
            invite_code: Some("INVITE123".into()),
            no_cloud: true,
            ai_provider: None,
            anthropic_api_key: None,
            ollama_url: None,
            ollama_model: None,
        };
        let body = build_request_from_args(args).expect("build body");
        assert!(!body.enable_cloud);
        assert!(body.invite_code.is_none());
    }

    /// Invite code present + no `--no-cloud` → cloud enabled and the invite
    /// flows through to the request body.
    #[test]
    fn build_request_from_args_invite_enables_cloud() {
        let args = NonInteractiveSetupArgs {
            name: "Tom".into(),
            email: None,
            birthday: None,
            invite_code: Some("INVITE123".into()),
            no_cloud: false,
            ai_provider: None,
            anthropic_api_key: None,
            ollama_url: None,
            ollama_model: None,
        };
        let body = build_request_from_args(args).expect("build body");
        assert!(body.enable_cloud);
        assert_eq!(body.invite_code.as_deref(), Some("INVITE123"));
    }

    #[test]
    fn build_request_from_args_blank_name_rejected() {
        let args = NonInteractiveSetupArgs {
            name: "   ".into(),
            email: None,
            birthday: None,
            invite_code: None,
            no_cloud: false,
            ai_provider: None,
            anthropic_api_key: None,
            ollama_url: None,
            ollama_model: None,
        };
        assert!(build_request_from_args(args).is_err());
    }

    #[test]
    fn build_request_from_args_anthropic_requires_key() {
        let args = NonInteractiveSetupArgs {
            name: "Tom".into(),
            email: None,
            birthday: None,
            invite_code: None,
            no_cloud: false,
            ai_provider: Some("anthropic".into()),
            anthropic_api_key: None,
            ollama_url: None,
            ollama_model: None,
        };
        let err = build_request_from_args(args).expect_err("missing key must error");
        assert!(format!("{}", err).contains("anthropic-api-key"));
    }

    /// Bad birthday rejected at the CLI layer too — saves a daemon round
    /// trip and matches the interactive validator's behaviour.
    #[test]
    fn build_request_from_args_invalid_birthday_rejected() {
        let args = NonInteractiveSetupArgs {
            name: "Tom".into(),
            email: None,
            birthday: Some("99-99".into()),
            invite_code: None,
            no_cloud: false,
            ai_provider: None,
            anthropic_api_key: None,
            ollama_url: None,
            ollama_model: None,
        };
        assert!(build_request_from_args(args).is_err());
    }
}
