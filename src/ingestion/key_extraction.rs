//! Key value extraction from JSON data based on schema key configuration.
//!
//! Extracts hash and range key values from ingested data, including
//! support for nested field paths and date normalization.

use crate::ingestion::IngestionResult;
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime};
use fold_db::schema::SchemaCore;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

// ---- Date normalization ----

/// Try to normalize a date string to "YYYY-MM-DD HH:MM:SS" format for
/// chronological sorting. Returns the original string if it cannot be
/// parsed as a date.
pub(crate) fn try_normalize_date(value: &str) -> String {
    let trimmed = value.trim();

    // Already normalized — skip parsing
    if NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S").is_ok() {
        return trimmed.to_string();
    }

    // RFC 3339 / ISO 8601 with timezone (e.g. "2024-01-05T15:30:00Z", "2024-01-05T15:30:00+00:00")
    if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
        return dt.format("%Y-%m-%d %H:%M:%S").to_string();
    }

    // RFC 2822 (e.g. "Mon, 05 Jan 2024 15:30:00 +0000")
    // Try built-in first, then strip day-of-week prefix for lenient parsing
    // (source data may have incorrect day names).
    if let Ok(dt) = DateTime::parse_from_rfc2822(trimmed) {
        return dt.format("%Y-%m-%d %H:%M:%S").to_string();
    }
    if let Some(rest) = trimmed.split_once(", ").map(|(_, r)| r) {
        if let Ok(dt) = DateTime::<FixedOffset>::parse_from_str(rest, "%d %b %Y %H:%M:%S %z") {
            return dt.format("%Y-%m-%d %H:%M:%S").to_string();
        }
    }

    // Twitter format: "Mon Jan 05 15:30:00 +0000 2024"
    // chrono can't parse %z followed by %Y, so strip the tz offset and parse
    // the rest as naive datetime with the year moved.
    if let Some(dt) = try_parse_twitter_date(trimmed) {
        return dt.format("%Y-%m-%d %H:%M:%S").to_string();
    }

    // Timezone-aware formats
    let tz_formats = [
        "%Y-%m-%dT%H:%M:%S%z",    // "2024-01-05T15:30:00+0000"
        "%Y-%m-%dT%H:%M:%S%.f%z", // "2024-01-05T15:30:00.000+0000"
    ];
    for fmt in &tz_formats {
        if let Ok(dt) = DateTime::<FixedOffset>::parse_from_str(trimmed, fmt) {
            return dt.format("%Y-%m-%d %H:%M:%S").to_string();
        }
    }

    // Naive datetime formats (no timezone)
    let naive_dt_formats = [
        "%Y-%m-%dT%H:%M:%S", // "2024-01-05T15:30:00"
        "%m/%d/%Y %H:%M:%S", // "01/05/2024 15:30:00"
        "%Y-%m-%d %H:%M",    // "2024-01-05 15:30"
    ];
    for fmt in &naive_dt_formats {
        if let Ok(dt) = NaiveDateTime::parse_from_str(trimmed, fmt) {
            return dt.format("%Y-%m-%d %H:%M:%S").to_string();
        }
    }

    // Date-only formats — normalize to midnight
    let date_formats = [
        "%Y-%m-%d",  // "2024-01-05"
        "%m/%d/%Y",  // "01/05/2024"
        "%B %d, %Y", // "January 5, 2024"
        "%b %d, %Y", // "Jan 5, 2024"
        "%d %B %Y",  // "5 January 2024"
        "%d %b %Y",  // "5 Jan 2024"
    ];
    for fmt in &date_formats {
        if let Ok(d) = NaiveDate::parse_from_str(trimmed, fmt) {
            return d.format("%Y-%m-%d 00:00:00").to_string();
        }
    }

    // Not a recognized date format — return original
    value.to_string()
}

/// Parse Twitter-style dates: "Mon Jan 05 15:30:00 +0000 2024"
/// Skips the day-of-week name and timezone offset, parses the rest.
/// This avoids chrono's strict day-of-week validation (source data may
/// have incorrect day names).
fn try_parse_twitter_date(value: &str) -> Option<NaiveDateTime> {
    // Pattern: "DDD MMM DD HH:MM:SS +ZZZZ YYYY"
    let parts: Vec<&str> = value.split_whitespace().collect();
    if parts.len() != 6 {
        return None;
    }
    // parts[4] should be a timezone offset like "+0000"
    let tz_part = parts[4];
    if !(tz_part.starts_with('+') || tz_part.starts_with('-')) || tz_part.len() != 5 {
        return None;
    }
    // Skip day-of-week (parts[0]) and timezone (parts[4]):
    // "Jan 05 15:30:00 2024"
    let without_dow_tz = format!("{} {} {} {}", parts[1], parts[2], parts[3], parts[5]);
    NaiveDateTime::parse_from_str(&without_dow_tz, "%b %d %H:%M:%S %Y").ok()
}

/// Short human-readable label for a JSON value's type — used in error
/// messages so operators can tell at a glance what shape the data was in.
fn type_label(val: &Value) -> &'static str {
    match val {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

// ---- Key extraction ----

/// Extract key values from JSON data based on schema key fields.
/// Looks up the schema in the node's schema manager to find key configuration,
/// then extracts the corresponding values from the data.
pub(crate) async fn extract_key_values_from_data(
    fields_and_values: &HashMap<String, Value>,
    schema_name: &str,
    schema_manager: &Arc<SchemaCore>,
) -> IngestionResult<HashMap<String, String>> {
    let mut keys_and_values = HashMap::new();

    match schema_manager.get_schema_metadata(schema_name) {
        Ok(Some(schema)) => {
            if let Some(key_def) = &schema.key {
                // Extract hash and range field values using the same logic.
                // Range fields get date normalization for chronological sorting.
                for (key_name, field_name, normalize_date) in [
                    ("hash_field", key_def.hash_field.as_deref(), false),
                    ("range_field", key_def.range_field.as_deref(), true),
                ] {
                    let Some(field) = field_name else { continue };
                    match extract_nested_field_value(fields_and_values, field) {
                        Some(val) if val.is_string() => {
                            let s = val.as_str().unwrap();
                            let s = if normalize_date {
                                try_normalize_date(s)
                            } else {
                                s.to_string()
                            };
                            keys_and_values.insert(key_name.to_string(), s);
                        }
                        Some(val) if val.is_f64() || val.is_i64() || val.is_u64() => {
                            keys_and_values.insert(key_name.to_string(), val.to_string());
                        }
                        Some(val) => {
                            // Schema declared this key but the data has it as a
                            // non-scalar (e.g., array, object, null). We can't
                            // build a usable key from that and silently making
                            // one up creates duplicate records on re-ingest, so
                            // reject the record. The most common cause is an AI
                            // mis-classification — the file probably belongs in
                            // a different schema.
                            return Err(crate::ingestion::IngestionError::InvalidInput(
                                format!(
                                    "{} '{}' in schema '{}' has unsupported type (expected string or number, got: {}). \
                                     The record's data is missing a usable value for the schema's declared key field — \
                                     this usually means the AI routed the file to the wrong schema.",
                                    key_name, field, schema_name, type_label(val)
                                ),
                            ));
                        }
                        None => {
                            return Err(crate::ingestion::IngestionError::InvalidInput(
                                format!(
                                    "{} '{}' not found in data for schema '{}'. \
                                     The record's data is missing the schema's declared key field — \
                                     this usually means the AI routed the file to the wrong schema.",
                                    key_name, field, schema_name
                                ),
                            ));
                        }
                    }
                }
            }
        }
        Ok(None) => {
            return Err(crate::ingestion::IngestionError::SchemaCreationError(format!(
                "Schema '{}' not found — cannot extract key values. Was the schema created successfully?",
                schema_name
            )));
        }
        Err(e) => {
            return Err(crate::ingestion::IngestionError::SchemaCreationError(
                format!(
                    "Failed to get schema '{}' for key extraction: {}",
                    schema_name, e
                ),
            ));
        }
    }

    // Disambiguate range keys: if the data has a content_hash field,
    // append it to the range key so records with the same date don't
    // overwrite each other.  RangePrefix("2024-01-") still matches all
    // January records because the hash comes after the date.
    if let Some(range_val) = keys_and_values.get_mut("range_field") {
        if let Some(hash_val) = fields_and_values
            .get("content_hash")
            .and_then(|v| v.as_str())
        {
            if !hash_val.is_empty() && !range_val.contains(hash_val) {
                *range_val = format!("{}|{}", range_val, hash_val);
            }
        }
    }

    tracing::info!(
            target: "fold_node::ingestion",
        "Extracted key values for schema '{}': {:?}",
        schema_name,
        keys_and_values
    );

    Ok(keys_and_values)
}

/// Extract a field value from JSON data, supporting dot-notation paths of
/// arbitrary depth (e.g. "a.b.c"). Falls back to a shallow search through
/// nested objects when the path doesn't match directly.
pub(crate) fn extract_nested_field_value<'a>(
    fields_and_values: &'a HashMap<String, Value>,
    field_path: &str,
) -> Option<&'a Value> {
    // Direct lookup (covers non-dotted paths and literal keys containing dots)
    if let Some(value) = fields_and_values.get(field_path) {
        return Some(value);
    }

    // Walk dot-separated path to arbitrary depth: "a.b.c" → map["a"]["b"]["c"]
    if field_path.contains('.') {
        let parts: Vec<&str> = field_path.split('.').collect();
        if let Some(root) = fields_and_values.get(parts[0]) {
            let mut current = root;
            for part in &parts[1..] {
                current = current.as_object()?.get(*part)?;
            }
            return Some(current);
        }
    }

    // Shallow fallback: search one level of nested objects by field name
    fields_and_values
        .values()
        .filter_map(|v| v.as_object())
        .find_map(|obj| obj.get(field_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_twitter_date() {
        // Correct day-of-week
        assert_eq!(
            try_normalize_date("Fri Jan 05 15:30:00 +0000 2024"),
            "2024-01-05 15:30:00"
        );
        assert_eq!(
            try_normalize_date("Fri Dec 20 08:45:12 +0000 2024"),
            "2024-12-20 08:45:12"
        );
        // Incorrect day-of-week (should still parse — real data may be wrong)
        assert_eq!(
            try_normalize_date("Mon Jan 05 15:30:00 +0000 2024"),
            "2024-01-05 15:30:00"
        );
    }

    #[test]
    fn test_normalize_iso8601() {
        assert_eq!(
            try_normalize_date("2024-01-05T15:30:00+0000"),
            "2024-01-05 15:30:00"
        );
        assert_eq!(
            try_normalize_date("2024-01-05T15:30:00"),
            "2024-01-05 15:30:00"
        );
    }

    #[test]
    fn test_normalize_already_normalized() {
        assert_eq!(
            try_normalize_date("2024-01-05 15:30:00"),
            "2024-01-05 15:30:00"
        );
    }

    #[test]
    fn test_normalize_date_only() {
        assert_eq!(try_normalize_date("2024-01-05"), "2024-01-05 00:00:00");
        assert_eq!(try_normalize_date("January 5, 2024"), "2024-01-05 00:00:00");
    }

    #[test]
    fn test_normalize_rfc2822() {
        // Correct day-of-week
        assert_eq!(
            try_normalize_date("Fri, 05 Jan 2024 15:30:00 +0000"),
            "2024-01-05 15:30:00"
        );
        // Incorrect day-of-week (lenient parsing)
        assert_eq!(
            try_normalize_date("Mon, 05 Jan 2024 15:30:00 +0000"),
            "2024-01-05 15:30:00"
        );
    }

    #[test]
    fn test_normalize_non_date() {
        assert_eq!(try_normalize_date("not-a-date"), "not-a-date");
        assert_eq!(try_normalize_date("12345"), "12345");
        assert_eq!(try_normalize_date("hello world"), "hello world");
    }

    #[test]
    fn test_extract_nested_field_value_dot_notation() {
        let mut fields = HashMap::new();
        fields.insert(
            "departure".to_string(),
            serde_json::json!({"airport": "SFO", "date": "2025-03-15"}),
        );
        fields.insert("flight".to_string(), serde_json::json!("JL001"));

        // Direct lookup
        assert_eq!(
            extract_nested_field_value(&fields, "flight"),
            Some(&serde_json::json!("JL001"))
        );

        // Dot-notation lookup
        assert_eq!(
            extract_nested_field_value(&fields, "departure.airport"),
            Some(&serde_json::json!("SFO"))
        );
        assert_eq!(
            extract_nested_field_value(&fields, "departure.date"),
            Some(&serde_json::json!("2025-03-15"))
        );

        // Missing nested field
        assert_eq!(
            extract_nested_field_value(&fields, "departure.terminal"),
            None
        );

        // Missing parent
        assert_eq!(extract_nested_field_value(&fields, "arrival.airport"), None);
    }

    /// When a schema's declared `hash_field` isn't present in the record's
    /// data, `extract_key_values_from_data` MUST reject the record rather than
    /// silently letting downstream code fall back to a non-deterministic
    /// content-hash key. The fallback created duplicate records on re-ingest
    /// (each LLM run produces slightly different mapped fields → different
    /// content hash → new record); rejection forces the file to surface as
    /// failed so the user can re-route it.
    #[tokio::test]
    async fn test_extract_rejects_when_declared_hash_field_missing_from_data() {
        use fold_db::schema::SchemaCore;
        use fold_db::test_helpers::TestSchemaBuilder;
        use std::sync::Arc;

        let core = Arc::new(SchemaCore::new_for_testing().await.expect("init core"));
        let schema_json = TestSchemaBuilder::new("Contacts")
            .fields(&["name", "phone"])
            .hash_key("email")
            .build_json();
        core.load_schema_from_json(&schema_json)
            .await
            .expect("load Contacts schema");

        // Data is missing the declared `email` hash field — mirrors the
        // dogfood repro where a calendar event got mis-classified into a
        // Contacts-shaped schema.
        let mut data = HashMap::new();
        data.insert("name".to_string(), serde_json::json!("Alice"));
        data.insert("phone".to_string(), serde_json::json!("555-0101"));

        let result = extract_key_values_from_data(&data, "Contacts", &core).await;

        let err = result.expect_err("missing declared hash_field must error, not warn-and-skip");
        let msg = err.to_string();
        assert!(
            msg.contains("hash_field 'email'") && msg.contains("Contacts"),
            "error must name the missing field and schema; got: {msg}"
        );
    }

    /// Symmetric case for `range_field`: the dogfood repro also paired
    /// `range_field 'completion_date' not found in data` with the same
    /// silent-fallback bug, so both directions must reject.
    #[tokio::test]
    async fn test_extract_rejects_when_declared_range_field_missing_from_data() {
        use fold_db::schema::SchemaCore;
        use fold_db::test_helpers::TestSchemaBuilder;
        use std::sync::Arc;

        let core = Arc::new(SchemaCore::new_for_testing().await.expect("init core"));
        let schema_json = TestSchemaBuilder::new("Tasks")
            .fields(&["title"])
            .hash_key("title")
            .range_key("completion_date")
            .build_json();
        core.load_schema_from_json(&schema_json)
            .await
            .expect("load Tasks schema");

        let mut data = HashMap::new();
        data.insert("title".to_string(), serde_json::json!("Buy groceries"));
        // completion_date intentionally absent

        let result = extract_key_values_from_data(&data, "Tasks", &core).await;

        let err = result.expect_err("missing declared range_field must error, not warn-and-skip");
        let msg = err.to_string();
        assert!(
            msg.contains("range_field 'completion_date'") && msg.contains("Tasks"),
            "error must name the missing field and schema; got: {msg}"
        );
    }

    /// A declared key field whose value is a non-scalar (e.g., the AI
    /// extracted an object or an array for what should be a hash) is just as
    /// unusable as a missing field — and the silent fallback would still
    /// produce drifting content hashes — so reject it the same way.
    #[tokio::test]
    async fn test_extract_rejects_when_declared_key_field_has_unsupported_type() {
        use fold_db::schema::SchemaCore;
        use fold_db::test_helpers::TestSchemaBuilder;
        use std::sync::Arc;

        let core = Arc::new(SchemaCore::new_for_testing().await.expect("init core"));
        let schema_json = TestSchemaBuilder::new("Contacts")
            .fields(&["name"])
            .hash_key("email")
            .build_json();
        core.load_schema_from_json(&schema_json)
            .await
            .expect("load Contacts schema");

        let mut data = HashMap::new();
        data.insert("name".to_string(), serde_json::json!("Alice"));
        data.insert(
            "email".to_string(),
            serde_json::json!(["alice@example.com", "alice@work.example.com"]),
        );

        let result = extract_key_values_from_data(&data, "Contacts", &core).await;

        let err = result.expect_err("non-scalar key field must error");
        let msg = err.to_string();
        assert!(
            msg.contains("unsupported type") && msg.contains("array"),
            "error must label the offending JSON type; got: {msg}"
        );
    }

    /// Sanity: when the declared key fields ARE present and scalar, extraction
    /// still succeeds — guards against the rejection path being too eager.
    #[tokio::test]
    async fn test_extract_succeeds_when_declared_keys_present() {
        use fold_db::schema::SchemaCore;
        use fold_db::test_helpers::TestSchemaBuilder;
        use std::sync::Arc;

        let core = Arc::new(SchemaCore::new_for_testing().await.expect("init core"));
        let schema_json = TestSchemaBuilder::new("Contacts")
            .fields(&["name"])
            .hash_key("email")
            .build_json();
        core.load_schema_from_json(&schema_json)
            .await
            .expect("load Contacts schema");

        let mut data = HashMap::new();
        data.insert("name".to_string(), serde_json::json!("Alice"));
        data.insert("email".to_string(), serde_json::json!("alice@example.com"));

        let keys = extract_key_values_from_data(&data, "Contacts", &core)
            .await
            .expect("happy path must succeed");

        assert_eq!(keys.get("hash_field").map(String::as_str), Some("alice@example.com"));
    }

    #[test]
    fn test_normalize_chronological_ordering() {
        // These Twitter-format dates sort incorrectly by day name:
        // "Fri..." < "Mon..." < "Wed..." alphabetically
        let dates = [
            "Wed Jan 01 00:00:00 +0000 2025",
            "Fri Jan 03 00:00:00 +0000 2025",
            "Mon Jan 06 00:00:00 +0000 2025",
        ];
        let mut normalized: Vec<String> = dates.iter().map(|d| try_normalize_date(d)).collect();
        let sorted = normalized.clone();
        normalized.sort();
        assert_eq!(
            normalized, sorted,
            "Normalized dates should already be in chronological order"
        );
    }
}
