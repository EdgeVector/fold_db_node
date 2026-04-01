//! AI types, retry logic, prompt building, and response parsing for ingestion.

use super::prompts::{PROMPT_ACTIONS, PROMPT_HEADER};
use crate::ingestion::{IngestionError, IngestionResult, StructureAnalyzer};
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

// ---- Retry logic ----

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

// ---- Prompt building ----

/// Maximum characters to keep from any single string field when building AI
/// prompts.  Long fields (e.g. the full markdown body from file_to_markdown)
/// would otherwise bloat the prompt and cause Ollama timeouts.
const MAX_PROMPT_FIELD_CHARS: usize = 300;

/// Maximum characters for the content preview hint in `extract_content_hint()`.
/// Slightly larger than `MAX_PROMPT_FIELD_CHARS` because the hint is the only
/// sample the AI sees for naming schemas by topic.
const MAX_CONTENT_HINT_CHARS: usize = 500;

/// Return a deep copy of `value` with every string field longer than
/// [`MAX_PROMPT_FIELD_CHARS`] truncated to that limit plus an ellipsis.
/// Objects and arrays are traversed recursively.
pub fn truncate_long_strings(value: &Value) -> Value {
    match value {
        Value::String(s) if s.len() > MAX_PROMPT_FIELD_CHARS => {
            let truncated: String = s.chars().take(MAX_PROMPT_FIELD_CHARS).collect();
            Value::String(format!("{}...", truncated))
        }
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), truncate_long_strings(v)))
                .collect(),
        ),
        Value::Array(arr) => Value::Array(arr.iter().map(truncate_long_strings).collect()),
        other => other.clone(),
    }
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

    // Match text-file wrappers: "content" + "file_type" (native parsers)
    // or "markdown" + "file_type" (file_to_markdown output)
    if !obj.contains_key("file_type") {
        return None;
    }
    let content = obj
        .get("content")
        .or_else(|| obj.get("markdown"))
        .and_then(|v| v.as_str())?;
    let category = obj.get("category").and_then(|v| v.as_str());
    let source = obj
        .get("source_file")
        .or_else(|| obj.get("source"))
        .and_then(|v| v.as_str());

    let preview: String = content.chars().take(MAX_CONTENT_HINT_CHARS).collect();
    let truncated = if content.chars().count() > MAX_CONTENT_HINT_CHARS {
        "..."
    } else {
        ""
    };

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
        LogFeature::Ingestion,
        info,
        "Analyzing JSON: {} elements, {} unique fields, is_array={}",
        stats.total_elements,
        stats.unique_fields,
        sample_json.is_array()
    );

    let is_array_input = sample_json.is_array();
    let compact_json = truncate_long_strings(sample_json);
    let prompt = create_prompt(&superset_structure, is_array_input, Some(&compact_json));

    log_feature!(
        LogFeature::Ingestion,
        debug,
        "AI prompt ({} chars): {}...",
        prompt.len(),
        &prompt[..prompt.len().min(500)]
    );

    Ok(prompt)
}

// ---- Response parsing ----

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
    let deserialize_stream = serde_json::Deserializer::from_str(text_to_parse).into_iter::<Value>();

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
        let has_desc = descriptions.map(|d| d.contains_key(field)).unwrap_or(false);
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

    let name = schema_obj
        .get("descriptive_name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .unwrap_or("");

    if name.is_empty() {
        return Err(IngestionError::ai_response_validation_error(
            "Schema must have a non-empty 'descriptive_name'. \
             ALWAYS include \"descriptive_name\": a clear, human-readable description.",
        ));
    }

    if fold_db::schema_service::name_validator::is_generic_name(name) {
        return Err(IngestionError::ai_response_validation_error(
            "Schema descriptive_name is too generic (e.g., 'Document Collection'). \
             The name must describe the CONTENT TOPIC — read the actual data and name it \
             specifically (e.g., 'Family Vacation Photos', 'Technical Architecture Notes').",
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
    let field_classifications = match schema_obj
        .get("field_classifications")
        .and_then(|v| v.as_object())
    {
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

    // Unwrap single-element arrays: Ollama often wraps the schema in an array
    // like [{"name": "..."}] instead of {"name": "..."}. The downstream
    // deserializer expects a single object, not an array.
    // Multi-element arrays are rejected — the prompt asks for a single schema.
    let new_schemas = match new_schemas {
        Some(Value::Array(mut arr)) if arr.len() == 1 => Some(arr.remove(0)),
        Some(Value::Array(arr)) if arr.len() > 1 => {
            return Err(IngestionError::ai_response_validation_error(format!(
                "new_schemas array contains {} schemas, expected 1",
                arr.len()
            )));
        }
        other => other,
    };

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
                return Err(IngestionError::ai_response_validation_error(format!(
                    "new_schemas must be an object or array, got: {}",
                    schema_val
                )));
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

    // Auto-fill identity mappers for schema fields missing from mutation_mappers.
    // The AI sometimes omits array fields (e.g., must_see: ["Temple", "Market"])
    // from mutation_mappers even though they appear in the schema's fields list.
    // Without a mapper, the field data is silently dropped during mutation generation.
    let mutation_mappers = backfill_missing_mappers(mutation_mappers, &new_schemas);

    // Sanitize HashMap<String, String> fields in the schema — Ollama models sometimes
    // return nested objects where flat strings are expected (e.g. field_descriptions:
    // {"name": {"description": "text"}} instead of {"name": "text"}).
    let new_schemas = new_schemas.map(sanitize_string_map_fields);

    Ok(AISchemaResponse {
        new_schemas,
        mutation_mappers,
    })
}

/// Sanitize a schema JSON value so that fields expecting strings don't contain nested objects.
///
/// Ollama models sometimes return nested objects where the Schema struct expects
/// flat strings. For example:
///   `"field_descriptions": {"name": {"description": "the person's name"}}`
/// becomes:
///   `"field_descriptions": {"name": "the person's name"}`
///
/// Also flattens non-string values in `key.hash_field`, `key.range_field`,
/// `name`, and `descriptive_name`.
fn sanitize_string_map_fields(mut schema_val: Value) -> Value {
    let schema_obj = match schema_val.as_object_mut() {
        Some(obj) => obj,
        None => return schema_val,
    };

    // 1. Sanitize HashMap<String, String> fields — values must be strings
    const STRING_MAP_FIELDS: &[&str] = &[
        "field_descriptions",
        "field_interest_categories",
        "ref_fields",
        "transform_fields",
    ];

    for field_name in STRING_MAP_FIELDS {
        let map = match schema_obj
            .get_mut(*field_name)
            .and_then(|v| v.as_object_mut())
        {
            Some(m) => m,
            None => continue,
        };

        let mut fixes = Vec::new();
        for (key, val) in map.iter() {
            if val.is_string() {
                continue;
            }
            if let Some(s) = flatten_value_to_string(val) {
                fixes.push((key.clone(), s));
            }
        }

        for (key, val) in fixes {
            log_feature!(
                LogFeature::Ingestion,
                warn,
                "Flattened non-string {} value for key '{}' to string",
                field_name,
                key
            );
            map.insert(key, Value::String(val));
        }
    }

    // 2. Sanitize top-level String fields
    const TOP_LEVEL_STRING_FIELDS: &[&str] = &["name", "descriptive_name"];
    for field_name in TOP_LEVEL_STRING_FIELDS {
        if let Some(val) = schema_obj.get(*field_name) {
            if !val.is_string() && !val.is_null() {
                if let Some(s) = flatten_value_to_string(val) {
                    log_feature!(
                        LogFeature::Ingestion,
                        warn,
                        "Flattened non-string top-level '{}' to string",
                        field_name
                    );
                    schema_obj.insert(field_name.to_string(), Value::String(s));
                }
            }
        }
    }

    // 3. Sanitize key.hash_field and key.range_field
    if let Some(key_obj) = schema_obj.get_mut("key").and_then(|v| v.as_object_mut()) {
        for key_field in &["hash_field", "range_field"] {
            if let Some(val) = key_obj.get(*key_field) {
                if !val.is_string() && !val.is_null() {
                    if let Some(s) = flatten_value_to_string(val) {
                        log_feature!(
                            LogFeature::Ingestion,
                            warn,
                            "Flattened non-string key.{} to string",
                            key_field
                        );
                        key_obj.insert(key_field.to_string(), Value::String(s));
                    }
                }
            }
        }
    }

    schema_val
}

/// Extract a string from a JSON value. For objects, take the first string value.
/// For arrays, join string elements. For primitives, convert to string.
fn flatten_value_to_string(val: &Value) -> Option<String> {
    match val {
        Value::String(s) => Some(s.clone()),
        Value::Object(map) => {
            // Take the first string value in the object
            map.values().find_map(|v| v.as_str()).map(|s| s.to_string())
        }
        Value::Array(arr) => {
            let strings: Vec<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
            if strings.is_empty() {
                None
            } else {
                Some(strings.join(", "))
            }
        }
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Null => None,
    }
}

/// For each field declared in `new_schemas.fields` that has no entry in
/// `mutation_mappers`, insert an identity mapper (`field -> field`).
fn backfill_missing_mappers(
    mut mappers: HashMap<String, String>,
    new_schemas: &Option<Value>,
) -> HashMap<String, String> {
    let schema_val = match new_schemas {
        Some(v) => v,
        None => return mappers,
    };

    // Extract the fields list from the schema definition
    let fields = match schema_val.get("fields").and_then(|f| f.as_array()) {
        Some(arr) => arr,
        None => return mappers,
    };

    let schema_name = schema_val
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("");

    // Collect which schema fields already have mappers pointing to them
    let mapped_targets: std::collections::HashSet<String> = mappers
        .values()
        .map(|v| {
            // "SchemaName.field" -> "field"
            v.rsplit('.').next().unwrap_or(v).to_string()
        })
        .collect();

    let mut added = 0;
    for field_val in fields {
        if let Some(field_name) = field_val.as_str() {
            if !mapped_targets.contains(field_name) && !mappers.contains_key(field_name) {
                let target = if schema_name.is_empty() {
                    field_name.to_string()
                } else {
                    format!("{}.{}", schema_name, field_name)
                };
                mappers.insert(field_name.to_string(), target);
                added += 1;
            }
        }
    }

    if added > 0 {
        log_feature!(
            LogFeature::Ingestion,
            info,
            "Auto-filled {} missing mutation mapper(s) for fields declared in schema '{}'",
            added,
            schema_name
        );
    }

    mappers
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
        LogFeature::Ingestion,
        debug,
        "Parsed AI response: {} mappers, new_schema={}",
        result.mutation_mappers.len(),
        result.new_schemas.is_some()
    );

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    // ---- extract_json_from_response tests ----

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

    #[test]
    fn test_truncate_long_strings_leaves_short_values() {
        let input = serde_json::json!({"name": "Alice", "age": 30});
        let result = truncate_long_strings(&input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_truncate_long_strings_truncates_long_value() {
        let long = "x".repeat(500);
        let input = serde_json::json!({"markdown": long, "source": "test.pdf"});
        let result = truncate_long_strings(&input);
        let md = result["markdown"].as_str().unwrap();
        assert!(md.len() <= MAX_PROMPT_FIELD_CHARS + 3); // +3 for "..."
        assert!(md.ends_with("..."));
        assert_eq!(result["source"], "test.pdf");
    }

    #[test]
    fn test_truncate_long_strings_recurses_into_arrays() {
        let long = "y".repeat(500);
        let input = serde_json::json!([{"content": long}]);
        let result = truncate_long_strings(&input);
        let content = result[0]["content"].as_str().unwrap();
        assert!(content.len() <= MAX_PROMPT_FIELD_CHARS + 3);
        assert!(content.ends_with("..."));
    }

    #[test]
    fn test_truncate_long_strings_preserves_non_strings() {
        let input = serde_json::json!({"count": 42, "active": true, "data": null});
        let result = truncate_long_strings(&input);
        assert_eq!(result, input);
    }

    // ---- backfill_missing_mappers tests ----

    #[test]
    fn test_backfill_adds_missing_array_field_mappers() {
        let mut mappers = HashMap::new();
        mappers.insert("name".to_string(), "vacation_prefs.name".to_string());

        let schema = Some(serde_json::json!({
            "name": "vacation_prefs",
            "fields": ["name", "must_see", "avoid", "interests"]
        }));

        let result = backfill_missing_mappers(mappers, &schema);
        assert_eq!(result.len(), 4);
        assert_eq!(result.get("name").unwrap(), "vacation_prefs.name");
        assert_eq!(result.get("must_see").unwrap(), "vacation_prefs.must_see");
        assert_eq!(result.get("avoid").unwrap(), "vacation_prefs.avoid");
        assert_eq!(result.get("interests").unwrap(), "vacation_prefs.interests");
    }

    #[test]
    fn test_backfill_no_op_when_all_mapped() {
        let mut mappers = HashMap::new();
        mappers.insert("name".to_string(), "s.name".to_string());
        mappers.insert("age".to_string(), "s.age".to_string());

        let schema = Some(serde_json::json!({
            "name": "s",
            "fields": ["name", "age"]
        }));

        let result = backfill_missing_mappers(mappers, &schema);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_backfill_no_op_when_no_schema() {
        let mappers = HashMap::new();
        let result = backfill_missing_mappers(mappers, &None);
        assert!(result.is_empty());
    }

    #[test]
    fn test_backfill_respects_existing_target_mapping() {
        // AI mapped "creator" -> "schema.artist" — "artist" is already covered
        let mut mappers = HashMap::new();
        mappers.insert("creator".to_string(), "schema.artist".to_string());

        let schema = Some(serde_json::json!({
            "name": "schema",
            "fields": ["artist", "title"]
        }));

        let result = backfill_missing_mappers(mappers, &schema);
        // "artist" is already a target, so only "title" should be added
        assert_eq!(result.len(), 2);
        assert_eq!(result.get("creator").unwrap(), "schema.artist");
        assert!(result.contains_key("title"));
    }

    #[test]
    fn test_backfill_end_to_end_with_validate() {
        // Simulates AI response that declares array fields but omits them from mappers
        let ai_json = serde_json::json!({
            "new_schemas": {
                "name": "travel_preferences",
                "descriptive_name": "Travel Preferences",
                "schema_type": "HashRange",
                "key": {"hash_field": "traveler", "range_field": "destination"},
                "fields": ["traveler", "destination", "must_see", "dietary_restrictions"],
                "field_descriptions": {
                    "traveler": "Name of the traveler",
                    "destination": "Travel destination",
                    "must_see": "Must-see attractions",
                    "dietary_restrictions": "Dietary restrictions"
                }
            },
            "mutation_mappers": {
                "traveler": "travel_preferences.traveler",
                "destination": "travel_preferences.destination"
            }
        });

        let result = validate_and_convert_response(ai_json).unwrap();
        // All 4 fields should have mappers now
        assert_eq!(result.mutation_mappers.len(), 4);
        assert!(result.mutation_mappers.contains_key("must_see"));
        assert!(result.mutation_mappers.contains_key("dietary_restrictions"));
    }

    // ---- sanitize_string_map_fields tests ----

    #[test]
    fn test_sanitize_flattens_nested_objects_in_field_descriptions() {
        let schema = serde_json::json!({
            "name": "bank_statement",
            "field_descriptions": {
                "date": {"description": "the transaction date"},
                "amount": {"description": "the dollar amount", "type": "number"},
                "memo": "a plain string already"
            }
        });

        let result = sanitize_string_map_fields(schema);
        let descs = result["field_descriptions"].as_object().unwrap();
        assert_eq!(descs["date"].as_str().unwrap(), "the transaction date");
        assert_eq!(descs["amount"].as_str().unwrap(), "the dollar amount");
        assert_eq!(descs["memo"].as_str().unwrap(), "a plain string already");
    }

    #[test]
    fn test_sanitize_leaves_valid_strings_unchanged() {
        let schema = serde_json::json!({
            "name": "recipes",
            "field_descriptions": {
                "title": "the recipe title",
                "servings": "number of servings"
            }
        });

        let result = sanitize_string_map_fields(schema.clone());
        assert_eq!(result, schema);
    }

    #[test]
    fn test_sanitize_handles_missing_field_descriptions() {
        let schema = serde_json::json!({"name": "test"});
        let result = sanitize_string_map_fields(schema.clone());
        assert_eq!(result, schema);
    }

    #[test]
    fn test_sanitize_handles_array_values() {
        let schema = serde_json::json!({
            "name": "test",
            "field_descriptions": {
                "tags": ["topic", "category"]
            }
        });

        let result = sanitize_string_map_fields(schema);
        assert_eq!(
            result["field_descriptions"]["tags"].as_str().unwrap(),
            "topic, category"
        );
    }

    #[test]
    fn test_sanitize_handles_number_values() {
        let schema = serde_json::json!({
            "name": "test",
            "field_descriptions": {
                "count": 42
            }
        });

        let result = sanitize_string_map_fields(schema);
        assert_eq!(
            result["field_descriptions"]["count"].as_str().unwrap(),
            "42"
        );
    }

    #[test]
    fn test_sanitize_end_to_end_with_validate() {
        // Simulates Ollama returning nested objects in field_descriptions
        let ai_json = serde_json::json!({
            "new_schemas": {
                "name": "bank_transactions",
                "descriptive_name": "Bank Transactions",
                "key": {"hash_field": "id", "range_field": "date"},
                "fields": ["id", "date", "amount", "description"],
                "field_descriptions": {
                    "id": {"description": "unique transaction identifier"},
                    "date": {"description": "date of the transaction", "format": "ISO 8601"},
                    "amount": "transaction amount in dollars",
                    "description": {"description": "merchant or transaction description"}
                }
            },
            "mutation_mappers": {
                "id": "bank_transactions.id",
                "date": "bank_transactions.date",
                "amount": "bank_transactions.amount",
                "description": "bank_transactions.description"
            }
        });

        let result = validate_and_convert_response(ai_json).unwrap();
        let schema = result.new_schemas.unwrap();
        let descs = schema["field_descriptions"].as_object().unwrap();
        // All values should now be strings
        for (key, val) in descs {
            assert!(
                val.is_string(),
                "field_descriptions['{}'] should be a string, got: {}",
                key,
                val
            );
        }
        assert_eq!(
            descs["id"].as_str().unwrap(),
            "unique transaction identifier"
        );
        assert_eq!(
            descs["amount"].as_str().unwrap(),
            "transaction amount in dollars"
        );
    }

    // ---- array unwrap tests ----

    #[test]
    fn test_single_element_array_unwrapped() {
        let ai_json = serde_json::json!({
            "new_schemas": [{
                "name": "expenses",
                "descriptive_name": "Monthly Expenses",
                "fields": ["amount"],
                "field_descriptions": {"amount": "dollar amount"}
            }],
            "mutation_mappers": {"amount": "expenses.amount"}
        });

        let result = validate_and_convert_response(ai_json).unwrap();
        let schema = result.new_schemas.unwrap();
        // Should be unwrapped from array to object
        assert!(schema.is_object(), "expected object, got: {}", schema);
        assert_eq!(schema["name"], "expenses");
    }

    #[test]
    fn test_multi_element_array_rejected() {
        let ai_json = serde_json::json!({
            "new_schemas": [
                {"name": "s1", "descriptive_name": "S1", "fields": ["a"]},
                {"name": "s2", "descriptive_name": "S2", "fields": ["b"]}
            ],
            "mutation_mappers": {}
        });

        let result = validate_and_convert_response(ai_json);
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("2 schemas"), "got: {}", err);
    }

    // ---- sanitize top-level and key field tests ----

    #[test]
    fn test_sanitize_flattens_top_level_name() {
        let schema = serde_json::json!({
            "name": {"value": "bank_statement"},
            "descriptive_name": {"text": "Bank Statement"}
        });

        let result = sanitize_string_map_fields(schema);
        assert_eq!(result["name"].as_str().unwrap(), "bank_statement");
        assert_eq!(
            result["descriptive_name"].as_str().unwrap(),
            "Bank Statement"
        );
    }

    #[test]
    fn test_sanitize_flattens_key_fields() {
        let schema = serde_json::json!({
            "name": "test",
            "key": {
                "hash_field": {"name": "category", "type": "string"},
                "range_field": {"name": "date", "type": "date"}
            }
        });

        let result = sanitize_string_map_fields(schema);
        let key = result["key"].as_object().unwrap();
        assert_eq!(key["hash_field"].as_str().unwrap(), "category");
        assert_eq!(key["range_field"].as_str().unwrap(), "date");
    }

    #[test]
    fn test_sanitize_leaves_valid_key_fields_unchanged() {
        let schema = serde_json::json!({
            "name": "test",
            "key": {
                "hash_field": "id",
                "range_field": "date"
            }
        });

        let result = sanitize_string_map_fields(schema.clone());
        assert_eq!(result, schema);
    }
}
