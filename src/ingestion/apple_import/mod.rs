//! Apple data import: extract Notes, Reminders, Photos, and Calendar events from macOS apps.
//!
//! This module provides shared extraction logic used by both the CLI
//! (`folddb ingest apple-*`) and the HTTP server (Apple Import tab).
//! Extraction uses `osascript` to call AppleScript on macOS.

#[cfg(target_os = "macos")]
pub mod calendar;
#[cfg(target_os = "macos")]
pub mod contacts;
#[cfg(target_os = "macos")]
pub mod notes;
#[cfg(target_os = "macos")]
pub mod photos;
#[cfg(target_os = "macos")]
pub mod reminders;
pub mod sync_config;
pub mod sync_scheduler;

#[cfg(target_os = "macos")]
use crate::ingestion::IngestionError;
#[cfg(target_os = "macos")]
use sha2::{Digest, Sha256};

/// Check whether we're running on macOS (Apple import requires osascript).
pub fn is_available() -> bool {
    cfg!(target_os = "macos")
}

/// Default timeout for osascript calls (5 minutes).
/// Photo exports can be slow for large batches; 5 min handles up to ~200 photos.
#[cfg(target_os = "macos")]
const OSASCRIPT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Run an AppleScript via osascript and return stdout.
///
/// Kills the process after `OSASCRIPT_TIMEOUT` to prevent indefinite hangs
/// (iCloud sync, missing Automation permission, unresponsive target app).
///
/// `app_label` names the target macOS app (e.g. "Reminders.app") so the
/// timeout error can point the user at the correct System Settings pane.
#[cfg(target_os = "macos")]
pub fn run_osascript(script: &str, app_label: &str) -> Result<String, IngestionError> {
    let child = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| IngestionError::Extraction(format!("Failed to run osascript: {}", e)))?;

    // Wait with timeout using a background thread + channel.
    let (tx, rx) = std::sync::mpsc::channel();
    let child_id = child.id();
    std::thread::spawn(move || {
        let result = child.wait_with_output();
        let _ = tx.send(result);
    });

    match rx.recv_timeout(OSASCRIPT_TIMEOUT) {
        Ok(Ok(output)) => {
            if !output.status.success() {
                let stderr_str = String::from_utf8_lossy(&output.stderr);
                return Err(IngestionError::Extraction(format!(
                    "AppleScript error ({}): {}",
                    app_label, stderr_str
                )));
            }
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        }
        Ok(Err(e)) => Err(IngestionError::Extraction(format!(
            "Failed to wait for osascript ({}): {}",
            app_label, e
        ))),
        Err(_timeout) => {
            // Kill the timed-out process via pkill (child ownership moved to thread)
            let _ = std::process::Command::new("kill")
                .arg("-9")
                .arg(child_id.to_string())
                .status();
            Err(IngestionError::Extraction(format!(
                "osascript timed out after {} seconds talking to {}. The app may be \
                 unresponsive, syncing with iCloud, or missing Automation permission. \
                 Grant access in System Settings → Privacy & Security → Automation \
                 (and Full Disk Access for Photos.app).",
                OSASCRIPT_TIMEOUT.as_secs(),
                app_label,
            )))
        }
    }
}

/// Compute a short content hash for deduplication.
#[cfg(target_os = "macos")]
pub fn content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let result = hasher.finalize();
    hex::encode(&result[..8])
}
