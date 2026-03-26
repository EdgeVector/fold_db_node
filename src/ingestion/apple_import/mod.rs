//! Apple data import: extract Notes, Reminders, and Photos from macOS apps.
//!
//! This module provides shared extraction logic used by both the CLI
//! (`folddb ingest apple-*`) and the HTTP server (Apple Import tab).
//! Extraction uses `osascript` to call AppleScript on macOS.

#[cfg(target_os = "macos")]
pub mod notes;
#[cfg(target_os = "macos")]
pub mod photos;
#[cfg(target_os = "macos")]
pub mod reminders;
pub mod routes;

use crate::ingestion::IngestionError;
use sha2::{Digest, Sha256};

/// Check whether we're running on macOS (Apple import requires osascript).
pub fn is_available() -> bool {
    cfg!(target_os = "macos")
}

/// Run an AppleScript via osascript and return stdout.
#[cfg(target_os = "macos")]
pub fn run_osascript(script: &str) -> Result<String, IngestionError> {
    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|e| IngestionError::Extraction(format!("Failed to run osascript: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(IngestionError::Extraction(format!(
            "AppleScript error: {}",
            stderr
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Compute a short content hash for deduplication.
pub fn content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let result = hasher.finalize();
    hex::encode(&result[..8])
}
