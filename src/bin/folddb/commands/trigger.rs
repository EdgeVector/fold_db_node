//! `folddb trigger` CLI dispatchers.
//!
//! `trigger log <view>` reads recent rows from the internal `TriggerFiring`
//! audit schema via the daemon's `/api/query` endpoint and renders them as a
//! table (or raw JSON in `--json` mode). `TriggerFiring` is HashRange-keyed by
//! (trigger_id, fired_at); filtering by `view_name` happens client-side since
//! the schema supports only numeric `value_filters` server-side.

use chrono::{DateTime, Local, TimeZone, Utc};
use comfy_table::{presets::UTF8_FULL, Cell, ContentArrangement, Table};
use serde_json::{json, Value};

use crate::client::FoldDbClient;
use crate::commands::CommandOutput;
use crate::error::CliError;
use crate::output::OutputMode;

const TRIGGER_FIRING_SCHEMA: &str = "TriggerFiring";
const LIMIT_CAP: usize = 1000;

/// Parse a duration string in the form `<N><s|m|h|d>`. Returns milliseconds.
///
/// Rejects empty input, missing unit, non-positive numbers, and unknown units.
pub fn parse_duration_ms(input: &str) -> Result<i64, CliError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(CliError::new("--last cannot be empty")
            .with_hint("Use a value like 24h, 30m, 7d, or 600s"));
    }

    let (num_str, unit) =
        trimmed.split_at(trimmed.find(|c: char| !c.is_ascii_digit()).ok_or_else(|| {
            CliError::new(format!("Missing unit in --last '{}'", trimmed))
                .with_hint("Use a value like 24h, 30m, 7d, or 600s")
        })?);
    if num_str.is_empty() {
        return Err(
            CliError::new(format!("Missing number in --last '{}'", trimmed))
                .with_hint("Use a value like 24h, 30m, 7d, or 600s"),
        );
    }
    let n: i64 = num_str.parse().map_err(|_| {
        CliError::new(format!("Invalid number in --last '{}'", trimmed))
            .with_hint("Use a value like 24h, 30m, 7d, or 600s")
    })?;
    if n <= 0 {
        return Err(CliError::new(format!(
            "--last must be positive, got '{}'",
            trimmed
        )));
    }
    let unit_ms: i64 = match unit {
        "s" => 1_000,
        "m" => 60 * 1_000,
        "h" => 60 * 60 * 1_000,
        "d" => 24 * 60 * 60 * 1_000,
        other => {
            return Err(CliError::new(format!("Unknown --last unit '{}'", other))
                .with_hint("Supported units: s (seconds), m (minutes), h (hours), d (days)"))
        }
    };
    n.checked_mul(unit_ms)
        .ok_or_else(|| CliError::new(format!("--last '{}' overflows i64 milliseconds", trimmed)))
}

/// Clamp `limit` into `[1, LIMIT_CAP]`. Callers pass clap-parsed values, so we
/// only need to protect the upper bound; zero is treated as "use default 1".
pub fn clamp_limit(limit: usize) -> usize {
    limit.clamp(1, LIMIT_CAP)
}

pub async fn log(
    client: &FoldDbClient,
    view: &str,
    last: &str,
    limit: usize,
    mode: OutputMode,
) -> Result<CommandOutput, CliError> {
    let window_ms = parse_duration_ms(last)?;
    let effective_limit = clamp_limit(limit);

    let now_ms: i64 = chrono::Utc::now().timestamp_millis();
    let cutoff_ms = now_ms.saturating_sub(window_ms);

    let query_body = json!({
        "schema_name": TRIGGER_FIRING_SCHEMA,
        "fields": [
            "trigger_id",
            "view_name",
            "fired_at",
            "duration_ms",
            "status",
            "input_row_count",
            "output_row_count",
            "error_message",
            "skip_reason",
        ],
        "sort_order": "desc",
        "value_filters": [
            { "GreaterThan": { "field": "fired_at", "value": cutoff_ms as f64 } }
        ],
    });

    // The daemon returns a 400 with "schema not found" if `TriggerFiring`
    // hasn't been registered yet — common on fresh nodes that haven't
    // been through Phase 1 schema registration. From a CLI ergonomics
    // standpoint that's identical to "no firings yet": return an empty
    // envelope rather than surfacing an internal-schema error to the
    // user. Any other error (network, malformed query, real server
    // failure) propagates normally.
    let raw = match client.raw_query(&query_body).await {
        Ok(v) => v,
        Err(e) if e.message.contains("not found as schema or view") => {
            json!({ "ok": true, "results": [] })
        }
        Err(e) => return Err(e),
    };

    if mode == OutputMode::Json {
        return Ok(CommandOutput::RawJson(raw));
    }

    let rows = filter_rows_for_view(&raw, view, effective_limit);

    if rows.is_empty() {
        return Ok(CommandOutput::Message(format!(
            "No firings found in the last {} for view {}",
            last, view
        )));
    }

    Ok(CommandOutput::Message(render_table(&rows)))
}

/// Pull `results` out of the /api/query envelope and keep rows where
/// `view_name` matches `view`. The server already returned rows in
/// range-key (fired_at) descending order and within the time window, so we
/// just filter by view and truncate to `limit`.
fn filter_rows_for_view(raw: &Value, view: &str, limit: usize) -> Vec<Value> {
    let results = raw
        .get("results")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    results
        .into_iter()
        .filter(|row| field_str(row, "view_name").as_deref() == Some(view))
        .take(limit)
        .collect()
}

fn render_table(rows: &[Value]) -> String {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec![
            "fired_at",
            "duration_ms",
            "status",
            "input_rows",
            "output_rows",
            "error",
        ]);

    for row in rows {
        table.add_row(vec![
            Cell::new(format_fired_at(row)),
            Cell::new(field_int_str(row, "duration_ms")),
            Cell::new(format_status(row)),
            Cell::new(field_int_str(row, "input_row_count")),
            Cell::new(field_int_str(row, "output_row_count")),
            Cell::new(field_str(row, "error_message").unwrap_or_default()),
        ]);
    }
    table.to_string()
}

/// Render the status cell. For `skipped` rows, append the `skip_reason`
/// so operators can filter `folddb trigger log | grep skipped:catch_up_budget`
/// without pulling `--json`. Non-skip statuses render bare.
fn format_status(row: &Value) -> String {
    let status = field_str(row, "status").unwrap_or_default();
    if status == "skipped" {
        match field_str(row, "skip_reason") {
            Some(reason) if !reason.is_empty() => format!("skipped:{}", reason),
            _ => status,
        }
    } else {
        status
    }
}

fn field_value<'a>(row: &'a Value, name: &str) -> Option<&'a Value> {
    row.get("fields").and_then(|f| f.get(name))
}

fn field_str(row: &Value, name: &str) -> Option<String> {
    field_value(row, name).and_then(|v| v.as_str().map(String::from))
}

fn field_int(row: &Value, name: &str) -> Option<i64> {
    field_value(row, name).and_then(|v| v.as_i64())
}

fn field_int_str(row: &Value, name: &str) -> String {
    field_int(row, name)
        .map(|n| n.to_string())
        .unwrap_or_default()
}

fn format_fired_at(row: &Value) -> String {
    let Some(ms) = field_int(row, "fired_at") else {
        return String::new();
    };
    match Utc.timestamp_millis_opt(ms).single() {
        Some(dt) => DateTime::<Local>::from(dt)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
        None => ms.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_accepts_common_units() {
        assert_eq!(parse_duration_ms("1s").unwrap(), 1_000);
        assert_eq!(parse_duration_ms("2m").unwrap(), 120_000);
        assert_eq!(parse_duration_ms("1h").unwrap(), 3_600_000);
        assert_eq!(parse_duration_ms("24h").unwrap(), 24 * 3_600_000);
        assert_eq!(parse_duration_ms("7d").unwrap(), 7 * 24 * 3_600_000);
    }

    #[test]
    fn parse_duration_trims_whitespace() {
        assert_eq!(parse_duration_ms("  30m  ").unwrap(), 30 * 60_000);
    }

    #[test]
    fn parse_duration_rejects_empty() {
        let err = parse_duration_ms("").unwrap_err();
        assert!(format!("{}", err).contains("empty"));
    }

    #[test]
    fn parse_duration_rejects_missing_unit() {
        assert!(parse_duration_ms("24").is_err());
    }

    #[test]
    fn parse_duration_rejects_missing_number() {
        assert!(parse_duration_ms("h").is_err());
    }

    #[test]
    fn parse_duration_rejects_unknown_unit() {
        assert!(parse_duration_ms("24w").is_err());
        assert!(parse_duration_ms("24H").is_err());
    }

    #[test]
    fn parse_duration_rejects_garbage() {
        assert!(parse_duration_ms("abc").is_err());
        assert!(parse_duration_ms("1h2m").is_err());
        assert!(parse_duration_ms("-1h").is_err());
    }

    #[test]
    fn parse_duration_rejects_zero() {
        assert!(parse_duration_ms("0h").is_err());
    }

    #[test]
    fn clamp_limit_caps_at_1000() {
        assert_eq!(clamp_limit(50), 50);
        assert_eq!(clamp_limit(999), 999);
        assert_eq!(clamp_limit(1000), 1000);
        assert_eq!(clamp_limit(5000), 1000);
        assert_eq!(clamp_limit(1), 1);
        // Zero clamped up to 1 to keep downstream .take() nonempty.
        assert_eq!(clamp_limit(0), 1);
    }

    fn mock_row(view: &str, fired_at_ms: i64, status: &str) -> Value {
        json!({
            "key": { "hash": "trig:0", "range": fired_at_ms.to_string() },
            "fields": {
                "trigger_id": "trig:0",
                "view_name": view,
                "fired_at": fired_at_ms,
                "duration_ms": 42,
                "status": status,
                "input_row_count": 7,
                "output_row_count": 3,
                "error_message": if status == "success" { Value::Null } else { json!("boom") },
            }
        })
    }

    #[test]
    fn filter_rows_matches_view_and_respects_limit() {
        let raw = json!({
            "ok": true,
            "results": [
                mock_row("other", 3_000, "success"),
                mock_row("target", 2_000, "success"),
                mock_row("target", 1_000, "error"),
                mock_row("target", 500, "success"),
            ]
        });
        let rows = filter_rows_for_view(&raw, "target", 2);
        assert_eq!(rows.len(), 2);
        assert_eq!(
            field_int(&rows[0], "fired_at"),
            Some(2_000),
            "first row should be the newest for `target`"
        );
    }

    #[test]
    fn filter_rows_missing_results_key_is_empty() {
        let raw = json!({ "ok": true });
        assert!(filter_rows_for_view(&raw, "v", 10).is_empty());
    }

    #[test]
    fn render_table_includes_core_columns() {
        let rows = vec![mock_row("v", 1_700_000_000_000, "error")];
        let out = render_table(&rows);
        for header in [
            "fired_at",
            "duration_ms",
            "status",
            "input_rows",
            "output_rows",
            "error",
        ] {
            assert!(
                out.contains(header),
                "header `{}` missing from:\n{}",
                header,
                out
            );
        }
        assert!(out.contains("boom"), "error message not rendered");
        assert!(out.contains("42"), "duration_ms not rendered");
    }

    /// Build a firing row with explicit per-column values. `error` is stored as
    /// JSON null when absent, matching the schema's optional-field encoding.
    fn firing_row(
        view: &str,
        fired_at_ms: i64,
        duration_ms: i64,
        status: &str,
        input_rows: i64,
        output_rows: i64,
        error: Option<&str>,
    ) -> Value {
        firing_row_with_skip(
            view,
            fired_at_ms,
            duration_ms,
            status,
            input_rows,
            output_rows,
            error,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)] // mirrors schema shape for test clarity
    fn firing_row_with_skip(
        view: &str,
        fired_at_ms: i64,
        duration_ms: i64,
        status: &str,
        input_rows: i64,
        output_rows: i64,
        error: Option<&str>,
        skip_reason: Option<&str>,
    ) -> Value {
        json!({
            "key": { "hash": "trig:x", "range": fired_at_ms.to_string() },
            "fields": {
                "trigger_id": "trig:x",
                "view_name": view,
                "fired_at": fired_at_ms,
                "duration_ms": duration_ms,
                "status": status,
                "input_row_count": input_rows,
                "output_row_count": output_rows,
                "error_message": match error {
                    Some(s) => json!(s),
                    None => Value::Null,
                },
                "skip_reason": match skip_reason {
                    Some(s) => json!(s),
                    None => Value::Null,
                },
            }
        })
    }

    #[test]
    fn format_status_renders_skipped_with_reason() {
        let skipped = firing_row_with_skip(
            "v",
            1_700_000_000_000,
            0,
            "skipped",
            0,
            0,
            None,
            Some("catch_up_budget"),
        );
        assert_eq!(format_status(&skipped), "skipped:catch_up_budget");

        // skip_reason missing on a non-skip row (server didn't send it):
        // render the bare status.
        let success = firing_row("v", 1, 42, "success", 1, 1, None);
        assert_eq!(format_status(&success), "success");

        // A "skipped" row with null / missing skip_reason falls back to the
        // bare status rather than rendering "skipped:" with a trailing colon.
        let skipped_no_reason = firing_row("v", 1, 0, "skipped", 0, 0, None);
        assert_eq!(format_status(&skipped_no_reason), "skipped");
    }

    /// Full-stdout snapshot for the rendered `trigger log` table. Pins column
    /// headers, column order, row order, and per-cell formatting for a known
    /// fixture of four firings (success, error, skipped, success). `fired_at`
    /// is formatted via local time, so its cells are computed through
    /// `format_fired_at` rather than hardcoded — the rest of the table is
    /// hardcoded verbatim.
    #[test]
    fn render_table_full_output_snapshot() {
        // Four firings, newest-first (matches the server's range-key desc
        // ordering that `filter_rows_for_view` preserves). The skipped row
        // carries skip_reason=catch_up_budget so the status cell renders as
        // `skipped:catch_up_budget` (wider than plain status values — this
        // drives the status column width in the snapshot).
        let rows = vec![
            firing_row("target", 1_700_172_800_000, 120, "success", 10, 5, None),
            firing_row(
                "target",
                1_700_086_400_000,
                2_500,
                "error",
                7,
                0,
                Some("transform timeout"),
            ),
            firing_row_with_skip(
                "target",
                1_700_043_200_000,
                0,
                "skipped",
                0,
                0,
                None,
                Some("catch_up_budget"),
            ),
            firing_row("target", 1_700_000_000_000, 42, "success", 1, 1, None),
        ];

        let t0 = format_fired_at(&rows[0]);
        let t1 = format_fired_at(&rows[1]);
        let t2 = format_fired_at(&rows[2]);
        let t3 = format_fired_at(&rows[3]);

        let expected = format!(
            "\
┌─────────────────────┬─────────────┬─────────────────────────┬────────────┬─────────────┬───────────────────┐
│ fired_at            ┆ duration_ms ┆ status                  ┆ input_rows ┆ output_rows ┆ error             │
╞═════════════════════╪═════════════╪═════════════════════════╪════════════╪═════════════╪═══════════════════╡
│ {t0} ┆ 120         ┆ success                 ┆ 10         ┆ 5           ┆                   │
├╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ {t1} ┆ 2500        ┆ error                   ┆ 7          ┆ 0           ┆ transform timeout │
├╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ {t2} ┆ 0           ┆ skipped:catch_up_budget ┆ 0          ┆ 0           ┆                   │
├╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ {t3} ┆ 42          ┆ success                 ┆ 1          ┆ 1           ┆                   │
└─────────────────────┴─────────────┴─────────────────────────┴────────────┴─────────────┴───────────────────┘",
        );

        assert_eq!(render_table(&rows), expected);
    }
}
