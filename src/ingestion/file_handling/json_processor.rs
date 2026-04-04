//! JSON conversion and processing for file uploads

use file_to_markdown::{
    Config as FtmConfig, Converter as FtmConverter, OllamaConfig as FtmOllamaConfig,
};
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use tempfile::NamedTempFile;

use crate::ingestion::{IngestionError, IngestionResult};
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;

/// Convert a file to structured JSON using file_to_markdown (fully local, no external API calls).
///
/// Uses Ollama for vision/OCR on images and PDFs; all other file types (text, CSV,
/// Office docs, archives) convert without any AI.
pub async fn convert_file_to_json(file_path: &PathBuf) -> Result<Value, IngestionError> {
    log_feature!(
        LogFeature::Ingestion,
        info,
        "Converting file to markdown: {:?}",
        file_path
    );

    let ingestion_config = crate::ingestion::IngestionConfig::load()?;

    let ollama_config = FtmOllamaConfig {
        base_url: ingestion_config.ollama.base_url.clone(),
        vision_model: ingestion_config.ollama.vision_model.clone(),
        ocr_model: ingestion_config.ollama.ocr_model.clone(),
    };

    let config = FtmConfig::from_home_dir(ollama_config, None)
        .map_err(|e| IngestionError::FileConversionFailed(format!("Config init: {}", e)))?;

    let converter = FtmConverter::new(config);

    let file_markdown = converter
        .convert_path(file_path.as_path())
        .await
        .map_err(|e| {
            log_feature!(
                LogFeature::Ingestion,
                error,
                "Failed to convert file: {}",
                e
            );
            IngestionError::FileConversionFailed(e.to_string())
        })?;

    let mut value = serde_json::to_value(&file_markdown)
        .map_err(|e| IngestionError::FileConversionFailed(format!("Serialization: {}", e)))?;

    strip_null_fields(&mut value);

    // For images, use a consistent descriptive_name so all photos land in the
    // same schema (enables schema expansion instead of creating one schema per photo).
    // The vision caption goes into the "markdown" field, not the schema name.
    if let Value::Object(map) = &mut value {
        if map.contains_key("image_format") {
            // Remove title if present — it would be the vision caption, not a schema name
            map.remove("title");
            map.insert(
                "descriptive_name".to_string(),
                Value::String("Photography".to_string()),
            );
        }
    }

    Ok(value)
}

/// Maximum length for a derived descriptive_name (characters).
#[cfg(test)]
const MAX_DESCRIPTIVE_NAME_LEN: usize = 120;

/// Extract the first meaningful sentence from markdown text, skipping YAML frontmatter.
/// Returns `None` if the body is empty after stripping.
#[cfg(test)]
fn first_sentence(md: &str) -> Option<String> {
    // Skip YAML frontmatter (--- ... ---)
    let body = if md.starts_with("---") {
        md.splitn(3, "---").nth(2).unwrap_or("")
    } else {
        md
    };

    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Take the first line, truncate to MAX_DESCRIPTIVE_NAME_LEN
    let first_line = trimmed.lines().next().unwrap_or("");
    let sentence = first_line.trim_start_matches('#').trim();
    if sentence.is_empty() {
        return None;
    }

    if sentence.len() <= MAX_DESCRIPTIVE_NAME_LEN {
        Some(sentence.to_string())
    } else {
        // Truncate at word boundary
        let truncated = &sentence[..MAX_DESCRIPTIVE_NAME_LEN];
        let end = truncated.rfind(' ').unwrap_or(MAX_DESCRIPTIVE_NAME_LEN);
        Some(format!("{}...", &sentence[..end]))
    }
}

/// Remove null-valued entries from a JSON object.
/// FileMarkdown has many Option<T> fields; nulls would clutter the AI schema prompt.
fn strip_null_fields(value: &mut Value) {
    if let Value::Object(map) = value {
        map.retain(|_, v| !v.is_null());
    }
}

/// Convert a file to JSON using file_to_markdown (actix-web wrapper)
pub async fn convert_file_to_json_http(
    file_path: &PathBuf,
) -> Result<Value, actix_web::HttpResponse> {
    use actix_web::HttpResponse;

    match convert_file_to_json(file_path).await {
        Ok(value) => Ok(value),
        Err(e) => {
            log_feature!(
                LogFeature::Ingestion,
                error,
                "File conversion failed: {}",
                e
            );
            Err(HttpResponse::InternalServerError().json(json!({
                "success": false,
                "error": format!("Failed to convert file to JSON: {}", e)
            })))
        }
    }
}

/// Flatten JSON structures with unnecessary root layers
/// Handles patterns:
/// 1. root -> array: {"key": [...]} => [...]
/// 2. root -> root -> array: {"key1": {"key2": [...]}} => [...]
/// 3. array elements with single-field wrappers: [{"wrapper": {...}}] => [{...}]
/// 4. direct arrays with single-field wrappers: [...] => [...]
pub fn flatten_root_layers(json: Value) -> Value {
    // Check if it's already an array - flatten its elements
    if json.is_array() {
        log_feature!(
            LogFeature::Ingestion,
            info,
            "Flattening array elements with single-field wrappers"
        );
        return flatten_array_elements(json);
    }

    // Check for root -> array pattern
    if let Value::Object(ref map) = json {
        // If object has exactly one field
        if map.len() == 1 {
            let (key, value) = map.iter().next().unwrap();

            // If that field is an array, flatten the array and its elements
            if value.is_array() {
                log_feature!(
                    LogFeature::Ingestion,
                    info,
                    "Flattening root->array pattern: removing '{}' wrapper",
                    key
                );
                return flatten_array_elements(value.clone());
            }

            // Check for root -> root -> array pattern
            if let Value::Object(ref inner_map) = value {
                if inner_map.len() == 1 {
                    let (inner_key, inner_value) = inner_map.iter().next().unwrap();
                    if inner_value.is_array() {
                        log_feature!(
                            LogFeature::Ingestion,
                            info,
                            "Flattening root->root->array pattern: removing '{}'->'{}' wrappers",
                            key,
                            inner_key
                        );
                        return flatten_array_elements(inner_value.clone());
                    }
                }
            }
        }
    }

    // No flattening needed
    json
}

/// Flatten array elements that have unnecessary single-field wrapper objects
fn flatten_array_elements(value: Value) -> Value {
    if let Value::Array(arr) = value {
        let flattened_elements: Vec<Value> = arr
            .into_iter()
            .map(|element| {
                // If element is an object with exactly one field
                if let Value::Object(ref map) = element {
                    if map.len() == 1 {
                        let (key, inner_value) = map.iter().next().unwrap();

                        // If that field contains an object (not an array or primitive),
                        // flatten by returning the inner object
                        if inner_value.is_object() {
                            log_feature!(
                                LogFeature::Ingestion,
                                debug,
                                "Flattening array element: removing '{}' wrapper from object",
                                key
                            );
                            return inner_value.clone();
                        }
                    }
                }
                element
            })
            .collect();

        Value::Array(flattened_elements)
    } else {
        value
    }
}

/// Ensure the JSON object produced by the vision model contains `image_type` and
/// `created_at` fields.  Existing values are preserved so the model's own output
/// is respected when present.
///
/// Returns the `descriptive_name` extracted from the vision model output (if any)
/// so it can be injected into the schema definition later.
pub fn enrich_image_json(
    json: &mut Value,
    file_path: &std::path::PathBuf,
    source_file_name: Option<&str>,
) -> Option<String> {
    let mut descriptive_name = None;
    if let Value::Object(map) = json {
        // Extract descriptive_name — it's schema metadata, not record data
        descriptive_name = map.remove("descriptive_name").and_then(|v| match v {
            Value::String(s) => Some(s),
            _ => None,
        });
        // source_file_name — used as hash key for unique record identity
        if !map.contains_key("source_file_name") {
            if let Some(sfn) = source_file_name {
                map.insert(
                    "source_file_name".to_string(),
                    Value::String(sfn.to_string()),
                );
            }
        }
        // image_type — keep if already set
        if !map.contains_key("image_type") {
            let image_type = classify_image_type(source_file_name.unwrap_or(""));
            map.insert("image_type".to_string(), Value::String(image_type));
        }
        // created_at — keep if already set
        if !map.contains_key("created_at") {
            let created_at = get_file_creation_date(file_path);
            map.insert("created_at".to_string(), Value::String(created_at));
        }
    }
    descriptive_name
}

/// Heuristic classification of an image based on the source filename.
///
/// - "screenshot" if the filename contains "screenshot"
/// - "diagram" for SVG files or filenames containing "chart" or "diagram"
/// - "photo" otherwise (default)
pub fn classify_image_type(source_file_name: &str) -> String {
    let lower = source_file_name.to_lowercase();
    if lower.contains("screenshot") {
        "screenshot".to_string()
    } else if lower.ends_with(".svg") || lower.contains("chart") || lower.contains("diagram") {
        "diagram".to_string()
    } else {
        "photo".to_string()
    }
}

/// Prompt for AI-powered visibility classification of photo content.
const VISIBILITY_CLASSIFICATION_PROMPT: &str = r#"Classify this photo as either "public" or "private" for a social feed.

"public": suitable for sharing — landscapes, food, pets, group activities, events, nature, architecture, art, travel, selfies, sports
"private": sensitive or personal content — documents, IDs, medical records, financial info, nudity, screenshots of private messages, personal correspondence, passwords, credit cards, prescriptions

Photo description:
{description}

Respond with exactly one word: public or private"#;

/// Classify a photo's visibility as `"public"` or `"private"` using the AI backend.
///
/// Reads the `markdown` field (vision model description) from the JSON and sends
/// it to the text AI for a simple binary classification. Returns the classification
/// string to be inserted into the JSON as the `visibility` field.
pub async fn classify_visibility(
    json: &Value,
    service: &crate::ingestion::ingestion_service::IngestionService,
) -> IngestionResult<String> {
    let description = json
        .get("markdown")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            IngestionError::InvalidInput(
                "Cannot classify visibility: no 'markdown' field in image JSON".to_string(),
            )
        })?;

    let truncated: String = description.chars().take(500).collect();
    let prompt = VISIBILITY_CLASSIFICATION_PROMPT.replace("{description}", &truncated);

    let raw_response = service.call_ai_raw(&prompt).await?;
    parse_visibility_response(&raw_response)
}

/// Parse the raw AI response into `"public"` or `"private"`.
///
/// Conservative: if the response mentions both words, `"private"` wins.
/// Errors on unrecognizable responses.
pub(crate) fn parse_visibility_response(response: &str) -> IngestionResult<String> {
    let trimmed = response.trim().to_lowercase();
    if trimmed.contains("private") {
        Ok("private".to_string())
    } else if trimmed.contains("public") {
        Ok("public".to_string())
    } else {
        Err(IngestionError::ai_response_validation_error(format!(
            "Unexpected visibility response: '{}'. Expected 'public' or 'private'.",
            trimmed
        )))
    }
}

/// Try to extract the original capture date from EXIF metadata.
fn get_exif_date(file_path: &std::path::PathBuf) -> Option<String> {
    let file = std::fs::File::open(file_path).ok()?;
    let mut bufreader = std::io::BufReader::new(file);
    let exif_data = exif::Reader::new()
        .read_from_container(&mut bufreader)
        .ok()?;

    let field = exif_data
        .get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)
        .or_else(|| exif_data.get_field(exif::Tag::DateTime, exif::In::PRIMARY))?;

    // EXIF format: "2024:07:15 14:30:00" → "2024-07-15 14:30:00"
    let raw = field.display_value().to_string();
    let cleaned = raw.trim_matches('"');
    let mut result = cleaned.to_string();
    if let Some(pos) = result.find(':') {
        result.replace_range(pos..pos + 1, "-");
        if let Some(pos2) = result[pos + 1..].find(':') {
            let abs = pos + 1 + pos2;
            if abs < 10 {
                // still in date part
                result.replace_range(abs..abs + 1, "-");
            }
        }
    }
    Some(result)
}

/// Read the file's creation date, preferring EXIF metadata for images.
/// Falls back to filesystem timestamps, then `Utc::now()`.
pub fn get_file_creation_date(file_path: &std::path::PathBuf) -> String {
    // 1. Try EXIF metadata (actual photo capture date)
    if let Some(exif_date) = get_exif_date(file_path) {
        return exif_date;
    }
    // 2. Fallback: prefer created() over modified() — created() is less
    //    likely to be the checkout/copy time on macOS
    std::fs::metadata(file_path)
        .ok()
        .and_then(|meta| meta.created().ok().or_else(|| meta.modified().ok()))
        .map(|time| {
            let dt: chrono::DateTime<chrono::Utc> = time.into();
            dt.format("%Y-%m-%d %H:%M:%S").to_string()
        })
        .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string())
}

/// Save JSON to a temporary file that persists for testing
/// Returns the path to the temporary file
pub fn save_json_to_temp_file(json: &Value) -> std::io::Result<String> {
    // Create temp directory in system temp location (works in Lambda and locally)
    let temp_dir = std::env::temp_dir().join("folddb_debug");
    std::fs::create_dir_all(&temp_dir)?;

    // Create a named temporary file with .json extension
    let temp_file = NamedTempFile::new_in(&temp_dir)?;

    // Write the JSON with pretty formatting
    let json_string = serde_json::to_string_pretty(json)?;

    // Get a mutable handle to write
    let mut file = temp_file.as_file();
    file.write_all(json_string.as_bytes())?;
    file.sync_all()?;

    // Persist the temp file so it doesn't get deleted when dropped
    let (_file, path) = temp_file.keep()?;

    Ok(path.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flatten_root_to_array() {
        let input = json!({
            "data": [
                {"id": 1, "name": "Alice"},
                {"id": 2, "name": "Bob"}
            ]
        });

        let result = flatten_root_layers(input);

        assert!(result.is_array());
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["id"], 1);
    }

    #[test]
    fn test_flatten_root_root_to_array() {
        let input = json!({
            "response": {
                "items": [
                    {"id": 1, "name": "Alice"},
                    {"id": 2, "name": "Bob"}
                ]
            }
        });

        let result = flatten_root_layers(input);

        assert!(result.is_array());
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["name"], "Alice");
    }

    #[test]
    fn test_no_flatten_multiple_fields() {
        let input = json!({
            "data": [{"id": 1}],
            "metadata": {"count": 1}
        });

        let result = flatten_root_layers(input.clone());

        // Should remain unchanged
        assert_eq!(result, input);
    }

    #[test]
    fn test_no_flatten_nested_object() {
        let input = json!({
            "user": {
                "id": 1,
                "name": "Alice"
            }
        });

        let result = flatten_root_layers(input.clone());

        // Should remain unchanged
        assert_eq!(result, input);
    }

    #[test]
    fn test_no_flatten_direct_array() {
        let input = json!([
            {"id": 1, "name": "Alice"},
            {"id": 2, "name": "Bob"}
        ]);

        let result = flatten_root_layers(input.clone());

        // Should remain unchanged
        assert_eq!(result, input);
    }

    #[test]
    fn test_no_flatten_deep_nesting() {
        let input = json!({
            "level1": {
                "level2": {
                    "level3": [{"id": 1}]
                }
            }
        });

        let result = flatten_root_layers(input.clone());

        // Should remain unchanged (we only flatten up to 2 levels)
        assert_eq!(result, input);
    }

    #[test]
    fn test_flatten_with_array_keeps_array_structure() {
        let input = json!({
            "data": [
                {"id": 1, "name": "Alice"},
                {"id": 2, "name": "Bob"}
            ]
        });

        let result = flatten_root_layers(input);

        // Verify it's an array, not wrapped in an object
        assert!(result.is_array(), "Result should be an array");
        assert!(
            !result.is_object(),
            "Result should not be wrapped in an object"
        );

        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 2);
    }

    #[test]
    fn test_flatten_array_elements_with_single_field_wrappers() {
        let input = json!({
            "data": [
                {"item": {"id": 1, "name": "Alice"}},
                {"item": {"id": 2, "name": "Bob"}}
            ]
        });

        let result = flatten_root_layers(input);

        assert!(result.is_array());
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 2);

        // Each array element should be flattened (no "item" wrapper)
        assert_eq!(arr[0]["id"], 1);
        assert_eq!(arr[0]["name"], "Alice");
        assert!(arr[0].get("item").is_none());

        assert_eq!(arr[1]["id"], 2);
        assert_eq!(arr[1]["name"], "Bob");
        assert!(arr[1].get("item").is_none());
    }

    #[test]
    fn test_flatten_array_elements_preserves_multi_field_objects() {
        let input = json!({
            "data": [
                {
                    "id": 1,
                    "wrapper": {"name": "Alice"}
                },
                {
                    "id": 2,
                    "wrapper": {"name": "Bob"}
                }
            ]
        });

        let result = flatten_root_layers(input.clone());

        // Should flatten root but NOT array elements (they have multiple fields)
        assert!(result.is_array());
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["id"], 1);
        assert!(arr[0].get("wrapper").is_some());
    }

    #[test]
    fn test_flatten_array_elements_preserves_primitives() {
        let input = json!({
            "data": [
                {"value": "Alice"},
                {"value": 42},
                {"value": true}
            ]
        });

        let result = flatten_root_layers(input);

        assert!(result.is_array());
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 3);

        // Should NOT flatten when the inner value is a primitive
        assert_eq!(arr[0]["value"], "Alice");
        assert_eq!(arr[1]["value"], 42);
        assert_eq!(arr[2]["value"], true);
    }

    #[test]
    fn test_flatten_complex_nested_structure() {
        let input = json!({
            "response": {
                "items": [
                    {"record": {"id": 1, "name": "Alice", "email": "alice@example.com"}},
                    {"record": {"id": 2, "name": "Bob", "email": "bob@example.com"}}
                ]
            }
        });

        let result = flatten_root_layers(input);

        assert!(result.is_array());
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 2);

        // Should flatten both root layers AND array element wrappers
        assert_eq!(arr[0]["id"], 1);
        assert_eq!(arr[0]["name"], "Alice");
        assert!(arr[0].get("record").is_none());

        assert_eq!(arr[1]["id"], 2);
        assert_eq!(arr[1]["name"], "Bob");
        assert!(arr[1].get("record").is_none());
    }

    #[test]
    fn test_flatten_direct_array_with_single_field_wrappers() {
        // Test case for arrays returned directly by file converter
        let input = json!([
            {"tweet": {"id": 1, "text": "Hello", "user": "alice"}},
            {"tweet": {"id": 2, "text": "World", "user": "bob"}}
        ]);

        let result = flatten_root_layers(input);

        assert!(result.is_array());
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 2);

        // Should flatten the "tweet" wrapper from each element
        assert_eq!(arr[0]["id"], 1);
        assert_eq!(arr[0]["text"], "Hello");
        assert_eq!(arr[0]["user"], "alice");
        assert!(arr[0].get("tweet").is_none());

        assert_eq!(arr[1]["id"], 2);
        assert_eq!(arr[1]["text"], "World");
        assert_eq!(arr[1]["user"], "bob");
        assert!(arr[1].get("tweet").is_none());
    }

    #[test]
    fn test_parse_visibility_public() {
        assert_eq!(parse_visibility_response("public").unwrap(), "public");
        assert_eq!(parse_visibility_response("  Public  \n").unwrap(), "public");
        assert_eq!(
            parse_visibility_response("The image is public.").unwrap(),
            "public"
        );
    }

    #[test]
    fn test_parse_visibility_private() {
        assert_eq!(parse_visibility_response("private").unwrap(), "private");
        assert_eq!(parse_visibility_response("  PRIVATE  ").unwrap(), "private");
        assert_eq!(
            parse_visibility_response("This looks private.").unwrap(),
            "private"
        );
    }

    #[test]
    fn test_parse_visibility_ambiguous_defaults_to_private() {
        // If response contains both words, "private" wins (conservative)
        assert_eq!(
            parse_visibility_response("not private, actually public").unwrap(),
            "private"
        );
    }

    #[test]
    fn test_parse_visibility_invalid() {
        assert!(parse_visibility_response("unknown").is_err());
        assert!(parse_visibility_response("").is_err());
        assert!(parse_visibility_response("maybe").is_err());
    }

    #[test]
    fn test_enrich_image_json_preserves_existing_visibility() {
        let mut json = json!({
            "description": "A sunset",
            "visibility": "private"
        });
        let path = std::path::PathBuf::from("/tmp/test.jpg");
        enrich_image_json(&mut json, &path, Some("test.jpg"));
        // enrich_image_json should NOT touch visibility — it's set by classify_visibility
        assert_eq!(json["visibility"], "private");
    }

    #[test]
    fn test_classify_image_type_photo() {
        assert_eq!(classify_image_type("vacation.jpg"), "photo");
        assert_eq!(classify_image_type("IMG_1234.PNG"), "photo");
        assert_eq!(classify_image_type(""), "photo");
    }

    #[test]
    fn test_classify_image_type_screenshot() {
        assert_eq!(
            classify_image_type("Screenshot_2024-01-01.png"),
            "screenshot"
        );
        assert_eq!(classify_image_type("my_screenshot.jpg"), "screenshot");
    }

    #[test]
    fn test_classify_image_type_diagram() {
        assert_eq!(classify_image_type("architecture.svg"), "diagram");
        assert_eq!(classify_image_type("sales_chart.png"), "diagram");
        assert_eq!(classify_image_type("system_diagram.jpg"), "diagram");
    }

    #[test]
    fn test_enrich_image_json_adds_fields() {
        let mut json = json!({"description": "A sunset"});
        let path = std::path::PathBuf::from("/tmp/test.jpg");
        enrich_image_json(&mut json, &path, Some("test.jpg"));

        assert_eq!(json["image_type"], "photo");
        assert!(json.get("created_at").is_some());
    }

    #[test]
    fn test_enrich_image_json_preserves_existing() {
        let mut json = json!({
            "description": "A sunset",
            "image_type": "landscape",
            "created_at": "2024-06-15 10:00:00"
        });
        let path = std::path::PathBuf::from("/tmp/test.jpg");
        enrich_image_json(&mut json, &path, Some("test.jpg"));

        // Should NOT overwrite existing values
        assert_eq!(json["image_type"], "landscape");
        assert_eq!(json["created_at"], "2024-06-15 10:00:00");
    }

    #[test]
    fn test_enrich_image_json_noop_for_non_object() {
        let mut json = json!([1, 2, 3]);
        let path = std::path::PathBuf::from("/tmp/test.jpg");
        enrich_image_json(&mut json, &path, Some("test.jpg"));
        // Should remain unchanged
        assert!(json.is_array());
    }

    #[test]
    fn test_strip_null_fields() {
        let mut value = json!({
            "source": "report.pdf",
            "file_type": "pdf",
            "title": null,
            "author": null,
            "page_count": 12,
            "duration_seconds": null
        });
        strip_null_fields(&mut value);

        let map = value.as_object().unwrap();
        assert_eq!(map.len(), 3);
        assert_eq!(map["source"], "report.pdf");
        assert_eq!(map["file_type"], "pdf");
        assert_eq!(map["page_count"], 12);
        assert!(!map.contains_key("title"));
        assert!(!map.contains_key("author"));
        assert!(!map.contains_key("duration_seconds"));
    }

    #[test]
    fn test_strip_null_fields_non_object() {
        let mut value = json!([1, null, 3]);
        strip_null_fields(&mut value);
        // Arrays are not stripped — only object fields
        assert!(value.is_array());
        assert_eq!(value.as_array().unwrap().len(), 3);
    }

    #[test]
    fn test_first_sentence_with_frontmatter() {
        let md = "---\nsource: photo.jpg\n---\nA sunset over the ocean with warm orange tones.";
        assert_eq!(
            first_sentence(md).unwrap(),
            "A sunset over the ocean with warm orange tones."
        );
    }

    #[test]
    fn test_first_sentence_no_frontmatter() {
        let md = "A cat sitting on a keyboard.";
        assert_eq!(first_sentence(md).unwrap(), "A cat sitting on a keyboard.");
    }

    #[test]
    fn test_first_sentence_strips_heading() {
        let md = "---\nfoo: bar\n---\n# Image Description\nA diagram of system architecture.";
        assert_eq!(first_sentence(md).unwrap(), "Image Description");
    }

    #[test]
    fn test_first_sentence_empty_body() {
        assert!(first_sentence("---\nfoo: bar\n---\n").is_none());
        assert!(first_sentence("").is_none());
        assert!(first_sentence("   ").is_none());
    }

    #[test]
    fn test_first_sentence_truncates_long_text() {
        let long = format!("---\nk: v\n---\n{}", "word ".repeat(50));
        let result = first_sentence(&long).unwrap();
        assert!(result.len() <= MAX_DESCRIPTIVE_NAME_LEN + 3); // +3 for "..."
        assert!(result.ends_with("..."));
    }
}
