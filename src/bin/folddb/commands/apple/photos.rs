use crate::commands::apple::{build_client, run_osascript};
use crate::commands::CommandOutput;
use crate::error::CliError;
use crate::output::spinner;
use crate::output::OutputMode;
use reqwest::multipart;
use std::path::{Path, PathBuf};

/// Temporary directory where Photos.app exports originals.
const EXPORT_DIR: &str = "/tmp/folddb_photos";

pub async fn run(
    album: Option<&str>,
    limit: usize,
    batch_size: usize,
    user_hash: &str,
    mode: OutputMode,
) -> Result<CommandOutput, CliError> {
    let sp = if mode == OutputMode::Human {
        Some(spinner::new_spinner("Exporting from Apple Photos..."))
    } else {
        None
    };

    // Ensure the export directory exists and is empty so we get a clean set.
    let export_dir = Path::new(EXPORT_DIR);
    if export_dir.exists() {
        std::fs::remove_dir_all(export_dir)
            .map_err(|e| CliError::new(format!("Failed to clean export dir: {}", e)))?;
    }
    std::fs::create_dir_all(export_dir)
        .map_err(|e| CliError::new(format!("Failed to create export dir: {}", e)))?;

    // Export photos from Photos.app into the temp directory.
    let script = build_export_script(album, limit);
    run_osascript(&script)?;

    if let Some(ref pb) = sp {
        spinner::finish_spinner(pb, "Photos exported");
    }

    // Collect exported files and convert HEIC → JPEG with sips.
    let paths = collect_and_convert(export_dir)?;
    if paths.is_empty() {
        return Err(CliError::new("No photos found in Apple Photos"));
    }

    let total = paths.len();
    let (client, base_url) = build_client(user_hash)?;
    let upload_url = format!("{}/api/ingestion/upload", base_url);

    let pb = if mode == OutputMode::Human {
        Some(spinner::new_progress_bar(total as u64, "Uploading photos"))
    } else {
        None
    };

    let mut all_ids = Vec::new();
    for (i, chunk) in paths.chunks(batch_size).enumerate() {
        for path in chunk {
            match upload_photo(&client, &upload_url, path).await {
                Ok(ids) => all_ids.extend(ids),
                Err(e) => {
                    if mode == OutputMode::Human {
                        eprintln!("  Skipping {}: {}", path.display(), e);
                    }
                }
            }
        }
        if let Some(ref pb) = pb {
            let pos = ((i + 1) * chunk.len()).min(total) as u64;
            pb.set_position(pos);
        }
    }

    if let Some(ref pb) = pb {
        pb.finish_and_clear();
    }

    Ok(CommandOutput::AppleIngestSuccess {
        source: "apple_photos".to_string(),
        total,
        ingested: all_ids.len(),
        ids: all_ids,
    })
}

async fn upload_photo(
    client: &reqwest::Client,
    upload_url: &str,
    path: &PathBuf,
) -> Result<Vec<String>, CliError> {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "photo.jpg".to_string());

    let file_bytes = tokio::fs::read(path)
        .await
        .map_err(|e| CliError::new(format!("Failed to read {}: {}", path.display(), e)))?;

    let mime_type = mime_guess::from_path(path)
        .first_or_octet_stream()
        .to_string();

    let part = multipart::Part::bytes(file_bytes)
        .file_name(file_name.clone())
        .mime_str(&mime_type)
        .map_err(|e| CliError::new(format!("Invalid MIME type: {}", e)))?;

    let form = multipart::Form::new()
        .part("file", part)
        .text("auto_execute", "true")
        .text("image_descriptive_name", file_name);

    let resp = client
        .post(upload_url)
        .multipart(form)
        .send()
        .await
        .map_err(|e| CliError::new(format!("Upload failed: {}", e)))?;

    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| CliError::new(format!("Invalid response: {}", e)))?;

    if !status.is_success() {
        let msg = body["error"]
            .as_str()
            .unwrap_or("Unknown upload error");
        return Err(CliError::new(format!("Upload failed ({}): {}", status, msg)));
    }

    let mut ids = Vec::new();
    if let Some(arr) = body["mutations_executed"].as_array() {
        for item in arr {
            if let Some(id) = item["id"].as_str() {
                ids.push(id.to_string());
            }
        }
    }
    Ok(ids)
}

/// Build an AppleScript that exports photos to a temp directory.
///
/// Uses `export ... to POSIX file ... using originals` which reliably
/// copies the actual image files out of the Photos library, unlike the
/// `filepath` property which is unavailable on modern macOS.
fn build_export_script(album: Option<&str>, limit: usize) -> String {
    let target_items = match album {
        Some(name) => format!(
            r#"set targetAlbum to album "{album}"
    set allItems to media items of targetAlbum"#,
            album = name.replace('"', "\\\""),
        ),
        None => "set allItems to media items".to_string(),
    };

    format!(
        r#"tell application "Photos"
    {target_items}
    set exportItems to items 1 thru (minimum of {{{limit}, count of allItems}}) of allItems
    export exportItems to POSIX file "{export_dir}" with using originals
end tell"#,
        target_items = target_items,
        limit = limit,
        export_dir = EXPORT_DIR,
    )
}

/// Walk the export directory, convert any HEIC files to JPEG via `sips`,
/// and return the list of uploadable image paths.
fn collect_and_convert(export_dir: &Path) -> Result<Vec<PathBuf>, CliError> {
    let mut result = Vec::new();

    let entries = std::fs::read_dir(export_dir)
        .map_err(|e| CliError::new(format!("Failed to read export dir: {}", e)))?;

    for entry in entries {
        let entry = entry
            .map_err(|e| CliError::new(format!("Failed to read dir entry: {}", e)))?;
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
            // Convert to JPEG using macOS built-in sips
            let jpeg_path = path.with_extension("jpg");
            let status = std::process::Command::new("sips")
                .args(["-s", "format", "jpeg"])
                .arg(&path)
                .arg("--out")
                .arg(&jpeg_path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map_err(|e| CliError::new(format!("Failed to run sips: {}", e)))?;

            if status.success() {
                // Remove the original HEIC to save disk space
                let _ = std::fs::remove_file(&path);
                result.push(jpeg_path);
            } else {
                // Fall back to uploading the HEIC as-is
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
    fn build_export_script_with_album() {
        let script = build_export_script(Some("Vacation"), 10);
        assert!(script.contains(r#"album "Vacation""#));
        assert!(script.contains("10"));
        assert!(script.contains("export"));
        assert!(script.contains(EXPORT_DIR));
    }

    #[test]
    fn build_export_script_all() {
        let script = build_export_script(None, 50);
        assert!(!script.contains("targetAlbum"));
        assert!(script.contains("50"));
        assert!(script.contains("export"));
    }

    #[test]
    fn collect_and_convert_empty_dir() {
        let dir = std::env::temp_dir().join("folddb_test_empty_photos");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let paths = collect_and_convert(&dir).unwrap_or_else(|e| panic!("{}", e));
        assert!(paths.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn collect_and_convert_jpeg_passthrough() {
        let dir = std::env::temp_dir().join("folddb_test_jpeg_photos");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let jpg = dir.join("test.jpg");
        std::fs::write(&jpg, b"fake jpeg").unwrap();
        let paths = collect_and_convert(&dir).unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].extension().unwrap(), "jpg");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
