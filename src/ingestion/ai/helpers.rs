//! AI types, retry logic, and prompt building for ingestion.

use super::prompts::{PROMPT_ACTIONS, PROMPT_HEADER};
use crate::ingestion::{IngestionError, IngestionResult, StructureAnalyzer};
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

// Re-export response parsing functions so existing callers don't break.
pub use super::response_parser::{
    extract_json_from_response, parse_ai_response, validate_and_convert_response,
    validate_schema_has_classifications, validate_schema_has_descriptive_name,
};

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
}
