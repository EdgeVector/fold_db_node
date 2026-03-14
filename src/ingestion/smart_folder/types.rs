//! Types and cost-estimation helpers for smart-folder scanning.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

// ---- Cost estimation ----

/// Estimate the ingestion cost for a single file based on its size and type.
///
/// The model accounts for multiple AI calls per file (classification, conversion,
/// schema recommendation, child schema resolution) plus a base schema-service call.
pub fn estimate_file_cost(path: &Path, root: &Path) -> f64 {
    let full_path = root.join(path);
    let file_size = std::fs::metadata(&full_path)
        .map(|m| m.len())
        .unwrap_or(0);

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    // Base cost for schema recommendation call
    let base_cost = 0.003;

    let content_cost = match ext.as_str() {
        // PDF: text extraction + conversion
        "pdf" => {
            let text_cost = text_cost_by_size(file_size);
            0.04 + text_cost
        }
        // Images: vision model call
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "heif" | "bmp" | "tiff" => 0.02,
        // Text-like files: cost scales with size
        _ => text_cost_by_size(file_size),
    };

    base_cost + content_cost
}

/// Helper: estimate the AI cost for text content based on byte size.
fn text_cost_by_size(size: u64) -> f64 {
    if size < 10_000 {
        0.005
    } else if size < 100_000 {
        0.015
    } else {
        0.028
    }
}

/// Get the file size for a path relative to root, returning 0 on error.
pub(crate) fn file_size_bytes(path: &Path, root: &Path) -> u64 {
    let full_path = root.join(path);
    std::fs::metadata(&full_path)
        .map(|m| m.len())
        .unwrap_or(0)
}

// ---- Data types ----

/// A file recommendation from the LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecommendation {
    /// File path relative to the scanned folder
    pub path: String,
    /// Whether the file should be ingested
    pub should_ingest: bool,
    /// Category: "personal_data", "media", "config", "website_scaffolding", "work", "unknown"
    pub category: String,
    /// Brief reason for the recommendation
    pub reason: String,
    /// Size of the file in bytes (populated during scan)
    #[serde(default)]
    pub file_size_bytes: u64,
    /// Estimated ingestion cost in USD
    #[serde(default)]
    pub estimated_cost: f64,
    /// Whether this file has already been ingested (dedup check)
    #[serde(default)]
    pub already_ingested: bool,
}

/// Summary of smart folder scan — category name → count.
/// Serializes as a flat JSON object like `{"personal_data": 5, "media": 3}`.
pub type SmartFolderSummary = HashMap<String, usize>;

/// Response from smart folder scanning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartFolderScanResponse {
    pub success: bool,
    /// Total files scanned
    pub total_files: usize,
    /// Files recommended for ingestion
    pub recommended_files: Vec<FileRecommendation>,
    /// Files recommended to skip
    pub skipped_files: Vec<FileRecommendation>,
    /// Summary statistics
    pub summary: SmartFolderSummary,
    /// Total estimated cost for all recommended files
    #[serde(default)]
    pub total_estimated_cost: f64,
    /// Whether the scan was truncated due to reaching max_files
    #[serde(default)]
    pub scan_truncated: bool,
    /// The max_depth value used for this scan
    #[serde(default)]
    pub max_depth_used: usize,
    /// The max_files value used for this scan
    #[serde(default)]
    pub max_files_used: usize,
}

/// Optional progress reporter for scan operations.
/// Accepts `(percentage, message)` updates.
pub type ScanProgressFn = Box<dyn Fn(u8, String) + Send + Sync>;
