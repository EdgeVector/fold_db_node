//! Shared helper functions for AI service implementations (Anthropic, Ollama).

use super::prompts::{PROMPT_ACTIONS, PROMPT_HEADER};
use super::{IngestionError, IngestionResult, StructureAnalyzer};
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

// ---- AI response types ----

/// Parsed AI response for schema analysis
#[derive(Debug, Serialize, Deserialize)]
pub struct AISchemaResponse {
    /// New schema definition created from the data structure
    pub new_schemas: Option<Value>,
    /// Mapping from JSON field paths to schema field paths
    pub mutation_mappers: HashMap<String, String>,
}

/// Call an async function with retries and exponential backoff.
///
/// Logs each attempt and backs off exponentially (1s, 2s, 4s, ...) on failure.
/// `label` is used in log messages (e.g. "Anthropic API" or "Ollama API").
/// `fallback_error` is called if all attempts fail without producing an error.
pub async fn call_with_retries<F, Fut>(
    label: &str,
    max_retries: u32,
    fallback_error: fn() -> IngestionError,
    mut call_fn: F,
) -> IngestionResult<String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = IngestionResult<String>>,
{
    let mut last_error = None;

    for attempt in 1..=max_retries {
        log_feature!(
            LogFeature::Ingestion,
            info,
            "{} attempt {} of {}",
            label,
            attempt,
            max_retries
        );

        let start_time = std::time::Instant::now();
        match call_fn().await {
            Ok(response) => {
                let elapsed = start_time.elapsed();
                log_feature!(
                    LogFeature::Ingestion,
                    info,
                    "{} call successful on attempt {} (took {:.2?})",
                    label,
                    attempt,
                    elapsed
                );
                return Ok(response);
            }
            Err(e) => {
                let elapsed = start_time.elapsed();
                log_feature!(
                    LogFeature::Ingestion,
                    warn,
                    "{} attempt {} failed (took {:.2?}): {}",
                    label,
                    attempt,
                    elapsed,
                    e
                );
                last_error = Some(e);

                if attempt < max_retries {
                    let delay = Duration::from_secs(2_u64.pow(attempt - 1));
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(fallback_error))
}

/// Pretty-print a JSON value.
pub fn pretty_json(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "Invalid JSON".to_string())
}

/// Create the prompt for the AI from sample JSON and array context.
///
/// `original_json` is the un-skeletonized data. When the data looks like a
/// text-file wrapper (`{content, source_file, file_type}`), we include a
/// preview of the actual content so the AI can name the schema by topic.
pub fn create_prompt(
    sample_json: &Value,
    is_array_input: bool,
    original_json: Option<&Value>,
) -> String {
    let array_note = if is_array_input {
        "\n\nIMPORTANT: The user provided a JSON ARRAY of multiple objects. You MUST create a Range schema with a range_key to store multiple entities."
    } else {
        ""
    };

    // For text-file wrappers, include a preview of the actual content so the
    // AI can determine the topic (e.g., "this is a recipe" → "recipes" schema).
    let content_hint = original_json
        .and_then(extract_content_hint)
        .unwrap_or_default();

    format!(
        "{header}\n\nSample JSON Data:\n{sample}{array_note}{content_hint}\n\n{actions}",
        header = PROMPT_HEADER,
        sample = pretty_json(sample_json),
        array_note = array_note,
        content_hint = content_hint,
        actions = PROMPT_ACTIONS
    )
}

/// Extract a content preview from text-file wrapper JSON for the AI prompt.
/// Returns a formatted hint string or None if the data isn't a text-file wrapper.
fn extract_content_hint(json: &Value) -> Option<String> {
    // Handle both single objects and arrays of objects
    let obj = if let Some(arr) = json.as_array() {
        arr.first()?.as_object()?
    } else {
        json.as_object()?
    };

    // Only for text-file wrappers that have content + file_type
    if !obj.contains_key("content") || !obj.contains_key("file_type") {
        return None;
    }

    let content = obj.get("content")?.as_str()?;
    let category = obj.get("category").and_then(|v| v.as_str());
    let source = obj.get("source_file").and_then(|v| v.as_str());

    // Truncate content to first 500 chars for the preview
    let preview: String = content.chars().take(500).collect();
    let truncated = if content.chars().count() > 500 { "..." } else { "" };

    let mut hint = format!(
        "\n\nCONTENT PREVIEW (use this to determine the schema name topic):\n\"{}{}\"",
        preview, truncated
    );
    if let Some(cat) = category {
        hint.push_str(&format!("\nCategory hint: \"{}\"", cat));
    }
    if let Some(src) = source {
        hint.push_str(&format!("\nSource file: \"{}\"", src));
    }
    hint.push_str("\n\nName the schema based on the CONTENT TOPIC above (e.g., \"recipes\", \"journal_entries\", \"medical_records\"), NOT based on the data format.");

    Some(hint)
}

/// Analyze JSON data and build the AI prompt for schema recommendation.
///
/// Shared between Anthropic and Ollama services.  Returns the prompt string
/// ready to be sent to the AI backend.
pub fn analyze_and_build_prompt(sample_json: &Value) -> IngestionResult<String> {
    let superset_structure = StructureAnalyzer::extract_structure_skeleton(sample_json);
    let stats = StructureAnalyzer::get_analysis_stats(sample_json);

    if let Some(array) = sample_json.as_array() {
        if array.is_empty() {
            return Err(IngestionError::ai_response_validation_error(
                "Cannot determine schema from empty JSON array".to_string(),
            ));
        }
    }

    log_feature!(
        LogFeature::Ingestion, info,
        "Analyzing JSON: {} elements, {} unique fields, is_array={}",
        stats.total_elements, stats.unique_fields, sample_json.is_array()
    );

    let is_array_input = sample_json.is_array();
    let prompt = create_prompt(&superset_structure, is_array_input, Some(sample_json));

    log_feature!(
        LogFeature::Ingestion, debug,
        "AI prompt ({} chars): {}...",
        prompt.len(),
        &prompt[..prompt.len().min(500)]
    );

    Ok(prompt)
}

/// Extract JSON from an AI response text that may contain markdown fences or extra text.
///
/// Handles both JSON objects (`{...}`) and arrays (`[...]`), with support for
/// markdown code blocks and surrounding prose.
pub fn extract_json_from_response(response_text: &str) -> IngestionResult<String> {
    // First try to find a JSON block marker
    let text_to_parse = if let Some(start) = response_text.find("```json") {
        let search_start = start + 7; // Length of "```json"
        if let Some(end_offset) = response_text[search_start..].find("```") {
            let json_end = search_start + end_offset;
            &response_text[search_start..json_end]
        } else {
            &response_text[search_start..]
        }
    } else {
        // Find the first '{' or '[' — whichever comes first
        let obj_start = response_text.find('{');
        let arr_start = response_text.find('[');
        match (obj_start, arr_start) {
            (Some(o), Some(a)) => &response_text[o.min(a)..],
            (Some(o), None) => &response_text[o..],
            (None, Some(a)) => &response_text[a..],
            (None, None) => response_text,
        }
    };

    // Use serde_json stream deserializer to parse the first valid JSON value
    let deserialize_stream =
        serde_json::Deserializer::from_str(text_to_parse).into_iter::<Value>();

    for value in deserialize_stream {
        match value {
            Ok(v) => {
                // Valid JSON found, re-serialize it to ensure it's clean
                return serde_json::to_string(&v).map_err(|e| {
                    IngestionError::ai_response_validation_error(format!(
                        "Failed to serialize extracted JSON: {}",
                        e
                    ))
                });
            }
            Err(_) => continue,
        }
    }

    // Fallback: try brace/bracket matching for objects and arrays
    for (open, close) in [('{', '}'), ('[', ']')] {
        if let (Some(start), Some(end)) = (response_text.find(open), response_text.rfind(close)) {
            if end > start {
                let json_candidate = response_text[start..=end].to_string();
                if serde_json::from_str::<Value>(&json_candidate).is_ok() {
                    return Ok(json_candidate);
                }
            }
        }
    }

    // All extraction strategies failed — return an error instead of passing garbage
    let preview = if response_text.len() > 200 {
        format!("{}...", &response_text[..200])
    } else {
        response_text.to_string()
    };
    Err(IngestionError::ai_response_validation_error(format!(
        "Could not extract valid JSON from AI response: {}",
        preview
    )))
}

/// Log a warning for each schema field missing a field_description entry.
/// Missing descriptions are recoverable (filled by a second AI pass), so this
/// warns instead of erroring to avoid blocking the retry loop.
fn warn_missing_field_descriptions(schema_val: &Value) {
    let schema_obj = match schema_val.as_object() {
        Some(o) => o,
        None => return,
    };

    let fields = match schema_obj.get("fields").and_then(|v| v.as_array()) {
        Some(f) => f,
        None => return,
    };

    let descriptions = schema_obj
        .get("field_descriptions")
        .and_then(|v| v.as_object());

    let schema_name = schema_obj
        .get("schema_name")
        .and_then(|v| v.as_str())
        .unwrap_or("<unnamed>");

    for field in fields.iter().filter_map(|f| f.as_str()) {
        let has_desc = descriptions
            .map(|d| d.contains_key(field))
            .unwrap_or(false);
        if !has_desc {
            log_feature!(
                LogFeature::Ingestion, warn,
                "Schema '{}' field '{}' is missing a field_description entry; will attempt recovery via second AI pass",
                schema_name, field
            );
        }
    }
}

/// Validate that a schema has a non-empty descriptive_name.
pub fn validate_schema_has_descriptive_name(schema_val: &Value) -> IngestionResult<()> {
    let schema_obj = schema_val.as_object().ok_or_else(|| {
        IngestionError::ai_response_validation_error("Schema must be a JSON object")
    })?;

    let has_name = schema_obj
        .get("descriptive_name")
        .and_then(|v| v.as_str())
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);

    if !has_name {
        return Err(IngestionError::ai_response_validation_error(
            "Schema must have a non-empty 'descriptive_name'. \
             ALWAYS include \"descriptive_name\": a clear, human-readable description.",
        ));
    }

    Ok(())
}

/// Validate that a schema has classifications for its fields.
pub fn validate_schema_has_classifications(schema_val: &Value) -> IngestionResult<()> {
    let schema_obj = schema_val.as_object().ok_or_else(|| {
        IngestionError::ai_response_validation_error("Schema must be a JSON object")
    })?;

    let schema_name = schema_obj
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    // field_classifications is optional — AI may not always provide them and defaults will be applied later
    let field_classifications = match schema_obj.get("field_classifications").and_then(|v| v.as_object()) {
        Some(fc) => fc,
        None => return Ok(()),
    };

    // Check each field's classifications are non-empty arrays.
    // Empty arrays are treated the same as absent — defaults will be applied later.
    for (field_name, classifications_val) in field_classifications {
        match classifications_val.as_array() {
            Some(arr) if !arr.is_empty() => {}
            _ => {
                log_feature!(
                    LogFeature::Ingestion, debug,
                    "Schema '{}' field '{}' has empty or non-array classifications — defaults will be applied",
                    schema_name, field_name
                );
            }
        }
    }

    Ok(())
}

/// Validate and convert a parsed JSON value into an AISchemaResponse.
pub fn validate_and_convert_response(parsed: Value) -> IngestionResult<AISchemaResponse> {
    let obj = parsed.as_object().ok_or_else(|| {
        IngestionError::ai_response_validation_error("Response must be a JSON object")
    })?;

    // Parse new_schemas (treat JSON null as absent)
    let new_schemas = obj.get("new_schemas").cloned().filter(|v| !v.is_null());

    // Validate that new schemas have required fields.
    // These checks run INSIDE the retry loop, so a validation failure here
    // triggers a fresh AI call with a new chance to produce correct output.
    if let Some(schema_val) = &new_schemas {
        match schema_val {
            Value::Array(schemas) => {
                for schema in schemas {
                    validate_schema_has_descriptive_name(schema)?;
                    validate_schema_has_classifications(schema)?;
                    warn_missing_field_descriptions(schema);
                }
            }
            Value::Object(_) => {
                validate_schema_has_descriptive_name(schema_val)?;
                validate_schema_has_classifications(schema_val)?;
                warn_missing_field_descriptions(schema_val);
            }
            _ => {
                return Err(IngestionError::ai_response_validation_error(
                    format!("new_schemas must be an object or array, got: {}", schema_val),
                ));
            }
        }
    }

    // Parse mutation_mappers
    let mutation_mappers = match obj.get("mutation_mappers") {
        Some(Value::Object(map)) => {
            let mut result = HashMap::new();
            for (key, value) in map {
                if let Some(value_str) = value.as_str() {
                    result.insert(key.clone(), value_str.to_string());
                }
            }
            result
        }
        Some(Value::Null) | None => HashMap::new(),
        _ => {
            return Err(IngestionError::ai_response_validation_error(
                "mutation_mappers must be an object with string values",
            ))
        }
    };

    Ok(AISchemaResponse {
        new_schemas,
        mutation_mappers,
    })
}

/// Parse the raw AI response text into an AISchemaResponse.
pub fn parse_ai_response(response_text: &str) -> IngestionResult<AISchemaResponse> {
    let json_str = extract_json_from_response(response_text)?;

    let parsed: Value = serde_json::from_str(&json_str).map_err(|e| {
        IngestionError::ai_response_validation_error(format!(
            "Failed to parse AI response as JSON: {}. Response: {}",
            e, json_str
        ))
    })?;

    let result = validate_and_convert_response(parsed)?;

    log_feature!(
        LogFeature::Ingestion, debug,
        "Parsed AI response: {} mappers, new_schema={}",
        result.mutation_mappers.len(), result.new_schemas.is_some()
    );

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_from_response() {
        // Test with JSON block markers
        let response_with_markers = r###"Here's the analysis:
```json
{"new_schemas": {"name": "test"}, "mutation_mappers": {}}
```
That should work."###;

        let result = extract_json_from_response(response_with_markers);
        assert!(result.is_ok());
        assert!(result.unwrap().contains("new_schemas"));

        // Test with direct JSON
        let response_direct = r###"{"new_schemas": null, "mutation_mappers": {}}"###;
        let result = extract_json_from_response(response_direct);
        assert!(result.is_ok());
    }

    #[test]
    fn test_extract_json_with_trailing_brace() {
        let response_trailing = r###"
        {
            "new_schemas": null,
            "mutation_mappers": {}
        }
        some extra text with a } closing brace
        "###;

        let result = extract_json_from_response(response_trailing);
        assert!(result.is_ok());
        let json = result.unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.get("new_schemas").is_some());
    }

    #[test]
    fn test_validate_and_convert_response() {
        let test_json = serde_json::json!({
            "new_schemas": null,
            "mutation_mappers": {
                "field1": "schema.field1",
                "nested.field": "schema.nested_field"
            }
        });

        let result = validate_and_convert_response(test_json);
        assert!(result.is_ok());

        let response = result.unwrap();
        assert_eq!(response.mutation_mappers.len(), 2);
    }

    #[test]
    fn test_create_prompt_includes_sample() {
        let sample = serde_json::json!({"a": 1});

        let prompt = create_prompt(&sample, false, None);
        assert!(prompt.contains("Sample JSON Data:"));
        assert!(prompt.contains("\"a\": 1"));
        assert!(!prompt.contains("Available Schemas:"));
        assert!(prompt.contains(PROMPT_HEADER));
        assert!(prompt.contains(PROMPT_ACTIONS));
    }

    #[test]
    fn test_pretty_json_helpers() {
        let value = serde_json::json!({"x": 1});
        assert!(pretty_json(&value).contains("\"x\": 1"));
    }

    // ---- extract_json_from_response edge cases ----

    #[test]
    fn test_extract_json_array_in_markdown_fence() {
        let response = r#"Here are the results:
```json
[{"path": "a.txt", "should_ingest": true}]
```
Done."#;
        let result = extract_json_from_response(response).unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert!(parsed.is_array());
        assert_eq!(parsed.as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_extract_nested_json_with_trailing_prose() {
        let response = r#"{"outer": {"inner": {"value": 42}}} And here is some extra explanation."#;
        let result = extract_json_from_response(response).unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["outer"]["inner"]["value"], 42);
    }

    #[test]
    fn test_extract_first_json_object_from_multiple() {
        let response = r#"{"first": 1} {"second": 2}"#;
        let result = extract_json_from_response(response).unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["first"], 1);
    }

    #[test]
    fn test_extract_only_prose_returns_error() {
        let response = "This response contains no JSON at all, just plain text.";
        let result = extract_json_from_response(response);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_empty_string_returns_error() {
        let result = extract_json_from_response("");
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_json_with_unicode() {
        let response = r#"{"name": "日本語テスト", "emoji": "🎉"}"#;
        let result = extract_json_from_response(response).unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["name"], "日本語テスト");
        assert_eq!(parsed["emoji"], "🎉");
    }

    #[test]
    fn test_extract_unclosed_markdown_fence() {
        let response = "```json\n{\"key\": \"value\"}\nsome trailing text without closing fence";
        let result = extract_json_from_response(response).unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["key"], "value");
    }

    #[test]
    fn test_extract_brace_matching_fallback() {
        // JSON with invalid trailing content that stream parser may choke on,
        // but brace-matching should rescue
        let response = "prefix {\"a\": 1, \"b\": [2, 3]} suffix with } brace";
        let result = extract_json_from_response(response).unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["a"], 1);
    }
}
