//! `folddb org` CLI dispatchers.
//!
//! Wraps the `/api/orgs/*` daemon endpoints (see `handlers::org`). Supports the
//! two-founder dogfood flow: one founder runs `org create`, pastes the printed
//! invite bundle to the other, who feeds it into `org join` via stdin.
//! `org list` shows a short fingerprint of the shared E2E key so both founders
//! can verify they really ended up in the same org.

use base64::Engine;
use sha2::{Digest, Sha256};

use crate::cli::OrgCommand;
use crate::client::FoldDbClient;
use crate::commands::CommandOutput;
use crate::error::CliError;
use crate::output::OutputMode;

/// Derive a short fingerprint of an org's base64-encoded shared E2E secret.
/// Returns the first 16 hex chars of SHA-256 over the decoded key bytes, or
/// `None` if the input isn't valid base64.
fn key_fingerprint(e2e_secret_b64: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(e2e_secret_b64)
        .ok()?;
    let digest = Sha256::digest(&bytes);
    Some(
        digest
            .iter()
            .take(8)
            .map(|b| format!("{:02x}", b))
            .collect(),
    )
}

pub async fn list(client: &FoldDbClient, mode: OutputMode) -> Result<CommandOutput, CliError> {
    let json = client.org_list().await?;
    if mode == OutputMode::Json {
        return Ok(CommandOutput::RawJson(json));
    }
    let orgs = json
        .get("data")
        .and_then(|d| d.get("orgs"))
        .or_else(|| json.get("orgs"))
        .and_then(|v| v.as_array());
    match orgs {
        Some(list) if list.is_empty() => Ok(CommandOutput::Message(
            "No organizations. Create one with: folddb org create <name>".to_string(),
        )),
        Some(list) => {
            let mut msg = format!("{} organization(s):\n", list.len());
            for org in list {
                let name = org["org_name"].as_str().unwrap_or("unnamed");
                let hash = org["org_hash"].as_str().unwrap_or("?");
                let role = org["role"].as_str().unwrap_or("Member");
                let fingerprint = org["org_e2e_secret"]
                    .as_str()
                    .and_then(key_fingerprint)
                    .unwrap_or_else(|| "unavailable".to_string());
                msg.push_str(&format!(
                    "\n  {} ({})\n    Hash:       {}\n    Key fprint: {}\n",
                    name, role, hash, fingerprint
                ));
            }
            Ok(CommandOutput::Message(msg))
        }
        None => Ok(CommandOutput::RawJson(json)),
    }
}

pub async fn create(
    client: &FoldDbClient,
    name: &str,
    mode: OutputMode,
) -> Result<CommandOutput, CliError> {
    let json = client.org_create(name).await?;
    if mode == OutputMode::Json {
        return Ok(CommandOutput::RawJson(json));
    }
    let invite = json
        .pointer("/data/invite_bundle")
        .or_else(|| json.get("invite_bundle"));
    let mut msg = format!("Organization \"{}\" created!", name);
    if let Some(bundle) = invite {
        msg.push_str("\n\nInvite bundle (share with members):\n");
        msg.push_str(&serde_json::to_string(bundle).unwrap_or_else(|_| "{}".to_string()));
    }
    Ok(CommandOutput::Message(msg))
}

pub async fn invites(client: &FoldDbClient, mode: OutputMode) -> Result<CommandOutput, CliError> {
    let json = client.org_pending_invites().await?;
    if mode == OutputMode::Json {
        return Ok(CommandOutput::RawJson(json));
    }
    let invites = json
        .get("data")
        .and_then(|d| d.get("invites"))
        .or_else(|| json.get("invites"))
        .and_then(|v| v.as_array());
    match invites {
        Some(list) if list.is_empty() => Ok(CommandOutput::Message(
            "No pending invitations.".to_string(),
        )),
        Some(list) => {
            let mut msg = format!("{} pending invitation(s):\n", list.len());
            for inv in list {
                let name = inv["org_name"].as_str().unwrap_or("unnamed");
                let hash = inv["org_hash"].as_str().unwrap_or("?");
                msg.push_str(&format!("\n  {} ({})", name, &hash[..16.min(hash.len())]));
            }
            msg.push_str("\n\nAccept with: folddb org join '<invite_json>'");
            Ok(CommandOutput::Message(msg))
        }
        None => Ok(CommandOutput::RawJson(json)),
    }
}

pub async fn join(
    client: &FoldDbClient,
    invite_json: Option<&str>,
    mode: OutputMode,
) -> Result<CommandOutput, CliError> {
    let json_str = match invite_json {
        Some(s) => s.to_string(),
        None => {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)
                .map_err(|e| CliError::new(format!("Failed to read stdin: {}", e)))?;
            buf
        }
    };
    let bundle: serde_json::Value = serde_json::from_str(&json_str)
        .map_err(|e| CliError::new(format!("Invalid invite JSON: {}", e)))?;
    let json = client.org_join(&bundle).await?;
    if mode == OutputMode::Json {
        return Ok(CommandOutput::RawJson(json));
    }
    let org_name = bundle["org_name"].as_str().unwrap_or("the organization");
    Ok(CommandOutput::Message(format!(
        "Joined \"{}\"! Org data will sync shortly.",
        org_name
    )))
}

pub async fn dispatch(
    action: &OrgCommand,
    client: &FoldDbClient,
    mode: OutputMode,
) -> Result<CommandOutput, CliError> {
    match action {
        OrgCommand::List => list(client, mode).await,
        OrgCommand::Create { name } => create(client, name, mode).await,
        OrgCommand::Invites => invites(client, mode).await,
        OrgCommand::Join { invite_json } => join(client, invite_json.as_deref(), mode).await,
    }
}

#[cfg(test)]
mod tests {
    use super::key_fingerprint;
    use base64::Engine;

    #[test]
    fn fingerprint_is_deterministic() {
        let key = base64::engine::general_purpose::STANDARD.encode([0x42u8; 32]);
        let a = key_fingerprint(&key).unwrap();
        let b = key_fingerprint(&key).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), 16);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn fingerprint_differs_for_different_keys() {
        let k1 = base64::engine::general_purpose::STANDARD.encode([0x01u8; 32]);
        let k2 = base64::engine::general_purpose::STANDARD.encode([0x02u8; 32]);
        assert_ne!(key_fingerprint(&k1), key_fingerprint(&k2));
    }

    #[test]
    fn fingerprint_rejects_invalid_base64() {
        assert!(key_fingerprint("not*valid*base64!").is_none());
    }
}
