use crate::commands::apple::{build_client, run_osascript};
use crate::commands::CommandOutput;
use crate::error::CliError;
use crate::output::spinner;
use crate::output::OutputMode;
use reqwest::multipart;
use std::path::PathBuf;

pub async fn run(
    album: Option<&str>,
    limit: usize,
    batch_size: usize,
    user_hash: &str,
    mode: OutputMode,
) -> Result<CommandOutput, CliError> {
    let sp = if mode == OutputMode::Human {
        Some(spinner::new_spinner("Querying Apple Photos..."))
    } else {
        None
    };

    let script = build_photos_script(album, limit);
    let raw = run_osascript(&script)?;

    if let Some(ref pb) = sp {
        spinner::finish_spinner(pb, "Photo list retrieved");
    }

    let paths = parse_photo_paths(&raw);
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
        .unwrap_or_else(|| "photo".to_string());

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

fn build_photos_script(album: Option<&str>, limit: usize) -> String {
    match album {
        Some(name) => format!(
            r#"tell application "Photos"
    set targetAlbum to album "{album}"
    set output to ""
    set count_ to 0
    repeat with m in (get media items of targetAlbum)
        if count_ >= {limit} then exit repeat
        set fpath to filename of m
        set output to output & fpath & linefeed
        set count_ to count_ + 1
    end repeat
    return output
end tell"#,
            album = name.replace('"', "\\\""),
            limit = limit,
        ),
        None => format!(
            r#"tell application "Photos"
    set output to ""
    set count_ to 0
    repeat with m in (get media items)
        if count_ >= {limit} then exit repeat
        set fpath to filename of m
        set output to output & fpath & linefeed
        set count_ to count_ + 1
    end repeat
    return output
end tell"#,
            limit = limit,
        ),
    }
}

fn parse_photo_paths(raw: &str) -> Vec<PathBuf> {
    // Photos app returns filenames; the actual files live in the Photos Library
    let photos_lib = dirs::home_dir()
        .map(|h| h.join("Pictures/Photos Library.photoslibrary/originals"))
        .unwrap_or_else(|| PathBuf::from("/Users/Shared/Photos Library.photoslibrary/originals"));

    raw.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|filename| {
            // Try the Photos Library path first, fall back to just filename
            let candidate = photos_lib.join(filename);
            if candidate.exists() {
                candidate
            } else {
                PathBuf::from(filename)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_photos_script_with_album() {
        let script = build_photos_script(Some("Vacation"), 10);
        assert!(script.contains(r#"album "Vacation""#));
        assert!(script.contains("10"));
    }

    #[test]
    fn build_photos_script_all() {
        let script = build_photos_script(None, 50);
        assert!(!script.contains("targetAlbum"));
        assert!(script.contains("50"));
    }

    #[test]
    fn parse_photo_paths_basic() {
        let raw = "IMG_0001.jpg\nIMG_0002.png\n\n";
        let paths = parse_photo_paths(raw);
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn parse_photo_paths_empty() {
        let paths = parse_photo_paths("");
        assert!(paths.is_empty());
    }
}
