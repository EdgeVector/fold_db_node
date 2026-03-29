//! Export photos from Apple Photos via osascript, convert HEIC→JPEG.

use std::path::{Path, PathBuf};

use super::run_osascript;
use crate::ingestion::IngestionError;

/// Temporary directory where Photos.app exports originals.
const EXPORT_DIR: &str = "/tmp/folddb_photos";

/// Export photos from Apple Photos to a temp directory and return their paths.
///
/// HEIC/HEIF files are automatically converted to JPEG via macOS `sips`.
pub fn export(album: Option<&str>, limit: usize) -> Result<Vec<PathBuf>, IngestionError> {
    let export_dir = Path::new(EXPORT_DIR);

    // Clean and recreate export directory
    if export_dir.exists() {
        std::fs::remove_dir_all(export_dir).map_err(|e| {
            IngestionError::Extraction(format!("Failed to clean export dir: {}", e))
        })?;
    }
    std::fs::create_dir_all(export_dir)
        .map_err(|e| IngestionError::Extraction(format!("Failed to create export dir: {}", e)))?;

    let script = build_script(album, limit);
    run_osascript(&script)?;

    collect_and_convert(export_dir)
}

pub fn build_script(album: Option<&str>, limit: usize) -> String {
    let target_items = match album {
        Some(name) => format!(
            r#"set targetAlbum to album "{album}"
    set allItems to every media item of targetAlbum"#,
            album = name.replace('"', "\\\""),
        ),
        None => "set allItems to every media item".to_string(),
    };

    format!(
        r#"tell application "Photos"
    {target_items}
    set totalCount to count of allItems
    if totalCount is 0 then return
    set maxItems to {limit}
    if totalCount < maxItems then set maxItems to totalCount
    set exportItems to items 1 thru maxItems of allItems
    export exportItems to POSIX file "{export_dir}" with using originals
end tell"#,
        target_items = target_items,
        limit = limit,
        export_dir = EXPORT_DIR,
    )
}

/// Walk the export directory, convert HEIC→JPEG via `sips`, return uploadable paths.
pub fn collect_and_convert(export_dir: &Path) -> Result<Vec<PathBuf>, IngestionError> {
    let mut result = Vec::new();

    let entries = std::fs::read_dir(export_dir)
        .map_err(|e| IngestionError::Extraction(format!("Failed to read export dir: {}", e)))?;

    for entry in entries {
        let entry = entry
            .map_err(|e| IngestionError::Extraction(format!("Failed to read dir entry: {}", e)))?;
        let path = entry.path();

        if !path.is_file() {
            continue;
        }

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();

        if ext == "heic" || ext == "heif" {
            let jpeg_path = path.with_extension("jpg");
            // 30-second timeout per file; large HEIC conversions rarely exceed this.
            let mut child = std::process::Command::new("sips")
                .args(["-s", "format", "jpeg"])
                .arg(&path)
                .arg("--out")
                .arg(&jpeg_path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|e| IngestionError::Extraction(format!("Failed to run sips: {}", e)))?;

            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(child.wait());
            });

            let success = match rx.recv_timeout(std::time::Duration::from_secs(30)) {
                Ok(Ok(status)) => status.success(),
                _ => false, // timeout or error — skip conversion, use original
            };

            if success {
                let _ = std::fs::remove_file(&path);
                result.push(jpeg_path);
            } else {
                // Clean up partial JPEG if it exists
                let _ = std::fs::remove_file(&jpeg_path);
                result.push(path);
            }
        } else {
            result.push(path);
        }
    }

    result.sort();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_script_with_album() {
        let script = build_script(Some("Vacation"), 10);
        assert!(script.contains(r#"album "Vacation""#));
        assert!(script.contains("10"));
        assert!(script.contains("export"));
        assert!(script.contains(EXPORT_DIR));
    }

    #[test]
    fn build_script_all() {
        let script = build_script(None, 50);
        assert!(!script.contains("targetAlbum"));
        assert!(script.contains("50"));
        assert!(script.contains("export"));
    }

    #[test]
    fn collect_and_convert_empty_dir() {
        let dir = std::env::temp_dir().join("folddb_test_empty_photos_shared");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let paths = collect_and_convert(&dir).unwrap();
        assert!(paths.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn collect_and_convert_jpeg_passthrough() {
        let dir = std::env::temp_dir().join("folddb_test_jpeg_photos_shared");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let jpg = dir.join("test.jpg");
        std::fs::write(&jpg, b"fake jpeg").unwrap();
        let paths = collect_and_convert(&dir).unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].extension().unwrap(), "jpg");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
