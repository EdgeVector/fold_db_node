//! `folddb schema` CLI dispatchers.
//!
//! `schema list` reads `/api/schemas` and renders one row per schema, preferring
//! the human-readable `descriptive_name` over the hex `name` (the schema hash).
//! Without this preference the human output was a wall of indistinguishable
//! 64-char hashes; the JSON-mode passthrough still surfaces both fields so
//! scripts can keep the hash for `schema get`/`query`/`mutate` calls.

use comfy_table::{presets::UTF8_FULL, Cell, ContentArrangement, Table};
use serde_json::Value;

use crate::client::FoldDbClient;
use crate::commands::CommandOutput;
use crate::error::CliError;
use crate::output::OutputMode;

pub async fn list(
    client: &FoldDbClient,
    show_hash: bool,
    mode: OutputMode,
) -> Result<CommandOutput, CliError> {
    let json = client.schema_list().await?;

    if mode == OutputMode::Json {
        return Ok(CommandOutput::RawJson(json));
    }

    let schemas = json
        .pointer("/data/schemas")
        .or_else(|| json.get("schemas"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if schemas.is_empty() {
        return Ok(CommandOutput::Message(
            "No schemas loaded. Run `folddb schema load` to discover schemas.".to_string(),
        ));
    }

    Ok(CommandOutput::Message(render_table(&schemas, show_hash)))
}

fn render_table(rows: &[Value], show_hash: bool) -> String {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic);

    let mut header = vec!["NAME", "STATE"];
    if show_hash {
        header.push("HASH");
    }
    table.set_header(header);

    for row in rows {
        let name = row
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let display = row
            .get("descriptive_name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| name.clone());
        let state = row
            .get("state")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown");

        let mut cells = vec![Cell::new(display), Cell::new(state)];
        if show_hash {
            cells.push(Cell::new(name));
        }
        table.add_row(cells);
    }
    table.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn prefers_descriptive_name_over_hash() {
        let rows = vec![json!({
            "name": "00f503feea00cd3553bbea2eebb87cb12e11ec74be4b6ebae93248b30d1dc934",
            "descriptive_name": "Journal Entries",
            "state": "Approved",
        })];
        let out = render_table(&rows, false);
        assert!(out.contains("Journal Entries"), "missing display name: {out}");
        assert!(
            !out.contains("00f503fe"),
            "hash should not appear without --show-hash: {out}"
        );
    }

    #[test]
    fn falls_back_to_name_when_descriptive_missing() {
        let rows = vec![json!({
            "name": "TriggerFiring",
            "state": "Available",
        })];
        let out = render_table(&rows, false);
        assert!(out.contains("TriggerFiring"), "missing fallback name: {out}");
    }

    #[test]
    fn falls_back_when_descriptive_name_is_empty_string() {
        let rows = vec![json!({
            "name": "abc123",
            "descriptive_name": "",
            "state": "Approved",
        })];
        let out = render_table(&rows, false);
        assert!(out.contains("abc123"), "expected fallback to name: {out}");
    }

    #[test]
    fn show_hash_appends_hash_column() {
        let rows = vec![json!({
            "name": "00f503feea00cd3553bbea2eebb87cb12e11ec74be4b6ebae93248b30d1dc934",
            "descriptive_name": "Journal Entries",
            "state": "Approved",
        })];
        let out = render_table(&rows, true);
        assert!(out.contains("Journal Entries"));
        assert!(out.contains("00f503feea00cd3553bbea2eebb87cb12e11ec74be4b6ebae93248b30d1dc934"));
        assert!(out.contains("HASH"));
    }
}
