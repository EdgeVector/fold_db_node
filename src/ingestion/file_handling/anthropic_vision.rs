//! Anthropic vision backend for image → markdown conversion.
//!
//! Mirrors the JSON shape produced by the `file_to_markdown` crate's Ollama
//! image extractor so downstream code (schema extraction, visibility
//! classification, record shape) doesn't care which backend produced the
//! markdown. Used when `ingestion_config.vision_backend` is set to
//! `Anthropic` — primarily CI and fresh installs without a local Ollama.
//!
//! Non-goals: EXIF extraction, image resizing, OCR-vs-description routing.
//! Anthropic handles image-of-text natively; EXIF is filled in by the
//! downstream `enrich_image_json` pass (it already reads EXIF via the
//! `exif` crate for the `created_at` / `date_taken` path).
//!
//! # Output JSON shape
//!
//! ```json
//! {
//!   "source":             "face-a.jpg",
//!   "file_type":          "jpg",
//!   "size_bytes":         12345,
//!   "mime_type":          "image/jpeg",
//!   "extraction_method":  "vision-anthropic",
//!   "markdown":           "<Claude's description of the image>",
//!   "image_format":       "jpg",
//!   "descriptive_name":   "Photography"
//! }
//! ```

use serde_json::{json, Value};
use std::path::Path;

use crate::ingestion::{
    ai::client::AnthropicBackend, config::AnthropicConfig, IngestionError, IngestionResult,
};

/// Matches Anthropic's accepted media types. Anything else returns an error
/// — caller should fall back to Ollama or report the file as unsupported.
/// Per Anthropic docs: image/jpeg, image/png, image/gif, image/webp.
fn media_type_for_extension(ext: &str) -> Option<&'static str> {
    match ext.to_lowercase().as_str() {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// True if we can route this file through Anthropic vision — otherwise caller
/// should keep the Ollama / `file_to_markdown` path.
pub fn supports_extension(ext: &str) -> bool {
    media_type_for_extension(ext).is_some()
}

/// Prompt sent alongside every image. Kept close to `file_to_markdown`'s
/// VISION_PROMPT so the resulting markdown reads the same whether the
/// backend is Ollama or Anthropic.
const VISION_PROMPT: &str = "Describe this image thoroughly and in detail: what is depicted, \
    including objects, people, and context. If there are charts, tables, or diagrams, describe \
    their content and structure. If the primary content is text (a document, receipt, screenshot \
    of text, etc.), transcribe the text faithfully. Output structured Markdown.";

/// Convert an image file to the `FileMarkdown`-shaped JSON the rest of the
/// ingestion pipeline expects.
///
/// Returns an error when:
/// - the file extension isn't an Anthropic-supported image format
/// - the file can't be read
/// - the Anthropic API call fails
pub async fn convert_image_to_json(
    file_path: &Path,
    anthropic_config: &AnthropicConfig,
    timeout_seconds: u64,
    max_retries: u32,
) -> IngestionResult<Value> {
    let file_type = file_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let media_type = media_type_for_extension(&file_type).ok_or_else(|| {
        IngestionError::InvalidInput(format!(
            "Anthropic vision does not support file extension {file_type:?}"
        ))
    })?;

    let source = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let bytes = std::fs::read(file_path).map_err(|e| {
        IngestionError::FileConversionFailed(format!(
            "Failed to read {}: {e}",
            file_path.display()
        ))
    })?;
    let size_bytes = bytes.len() as u64;

    let backend =
        AnthropicBackend::new(anthropic_config.clone(), timeout_seconds, max_retries)
            .map_err(|e| {
                IngestionError::configuration_error(format!(
                    "Failed to build Anthropic backend for vision: {e}"
                ))
            })?;

    let markdown = backend
        .call_vision(&bytes, media_type, VISION_PROMPT)
        .await
        .map_err(|e| {
            IngestionError::FileConversionFailed(format!("Anthropic vision call failed: {e}"))
        })?;

    Ok(json!({
        "source":            source,
        "file_type":         file_type,
        "size_bytes":        size_bytes,
        "mime_type":         media_type,
        "extraction_method": "vision-anthropic",
        "markdown":          markdown.trim(),
        "image_format":      file_type,
        "descriptive_name":  "Photography",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_type_handles_common_image_extensions() {
        assert_eq!(media_type_for_extension("jpg"), Some("image/jpeg"));
        assert_eq!(media_type_for_extension("JPG"), Some("image/jpeg"));
        assert_eq!(media_type_for_extension("jpeg"), Some("image/jpeg"));
        assert_eq!(media_type_for_extension("png"), Some("image/png"));
        assert_eq!(media_type_for_extension("gif"), Some("image/gif"));
        assert_eq!(media_type_for_extension("webp"), Some("image/webp"));
    }

    #[test]
    fn media_type_rejects_unsupported_extensions() {
        // Anthropic vision doesn't accept these — caller should fall back.
        assert_eq!(media_type_for_extension("bmp"), None);
        assert_eq!(media_type_for_extension("tiff"), None);
        assert_eq!(media_type_for_extension("pdf"), None);
        assert_eq!(media_type_for_extension("svg"), None);
        assert_eq!(media_type_for_extension(""), None);
    }

    #[test]
    fn supports_extension_matches_media_type_check() {
        assert!(supports_extension("jpg"));
        assert!(supports_extension("PNG"));
        assert!(!supports_extension("pdf"));
        assert!(!supports_extension("txt"));
    }

    // Note: convert_image_to_json is tested end-to-end via the scenario
    // runner (face-discovery-self.yaml with INGESTION_VISION_BACKEND=anthropic).
    // A pure unit test would need either a live Anthropic key or a mock HTTP
    // server — both add infrastructure disproportionate to the surface area
    // (one match statement + one JSON-shape assembly).
}
