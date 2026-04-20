//! `folddb snapshot` CLI dispatchers.
//!
//! Wraps the `POST /api/snapshot/backup` and `POST /api/snapshot/restore`
//! daemon endpoints (see `handlers::snapshot`). Requires cloud sync to be
//! enabled on the node.

use crate::client::FoldDbClient;
use crate::commands::CommandOutput;
use crate::error::CliError;
use crate::output::OutputMode;

pub async fn backup(client: &FoldDbClient, mode: OutputMode) -> Result<CommandOutput, CliError> {
    let json = client.snapshot_backup().await?;
    if mode == OutputMode::Json {
        return Ok(CommandOutput::RawJson(json));
    }
    let seq = json
        .pointer("/data/seq")
        .or_else(|| json.get("seq"))
        .and_then(|v| v.as_u64());
    let msg = match seq {
        Some(s) => format!("Snapshot uploaded at seq {s}."),
        None => "Snapshot upload complete.".to_string(),
    };
    Ok(CommandOutput::Message(msg))
}

pub async fn restore(client: &FoldDbClient, mode: OutputMode) -> Result<CommandOutput, CliError> {
    let json = client.snapshot_restore().await?;
    if mode == OutputMode::Json {
        return Ok(CommandOutput::RawJson(json));
    }
    let targets = json
        .pointer("/data/targets_restored")
        .or_else(|| json.get("targets_restored"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let entries = json
        .pointer("/data/entries_replayed")
        .or_else(|| json.get("entries_replayed"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let schemas = json
        .pointer("/data/schemas_refreshed")
        .or_else(|| json.get("schemas_refreshed"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let msg = format!(
        "Restored snapshot into {targets} target(s); refreshed {schemas} schema(s) in cache, \
         {entries} additional log entries applied on top.\n\
         Run `folddb daemon start` if the daemon isn't already running to resume delta sync."
    );
    Ok(CommandOutput::Message(msg))
}
