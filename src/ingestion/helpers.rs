//! Pure (framework-agnostic) helpers used by both ingestion HTTP routes and
//! background workers such as the smart-folder batch coordinator and the
//! Apple auto-sync scheduler.
//!
//! No actix/HTTP types live here — route handlers translate between these
//! helpers and `HttpResponse`.

use crate::ingestion::ingestion_service::IngestionService;
use crate::ingestion::progress::ProgressService;
use crate::ingestion::{IngestionRequest, ProgressTracker};
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::Instrument;

/// Response for batch folder ingestion
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchFolderResponse {
    pub success: bool,
    pub batch_id: String,
    pub files_found: usize,
    pub file_progress_ids: Vec<FileProgressInfo>,
    pub message: String,
}

/// Progress info for a single file in a batch
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileProgressInfo {
    pub file_name: String,
    pub progress_id: String,
}

/// Error describing why folder validation failed.
#[derive(Debug, Clone)]
pub enum FolderValidationError {
    NotFound(PathBuf),
    NotADirectory(PathBuf),
}

impl std::fmt::Display for FolderValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(p) => write!(f, "Folder not found: {}", p.display()),
            Self::NotADirectory(p) => write!(f, "Path is not a directory: {}", p.display()),
        }
    }
}

/// Validate that a path exists and is a directory.
pub fn validate_folder(path: &Path) -> Result<(), FolderValidationError> {
    if !path.exists() {
        return Err(FolderValidationError::NotFound(path.to_path_buf()));
    }
    if !path.is_dir() {
        return Err(FolderValidationError::NotADirectory(path.to_path_buf()));
    }
    Ok(())
}

/// Resolve a folder path — expands `~` to the home directory, absolute paths
/// pass through, relative paths are resolved against the current working directory.
pub fn resolve_folder_path(path: &str) -> PathBuf {
    let expanded = if path == "~" {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from(path))
    } else if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            home.join(rest)
        } else {
            PathBuf::from(path)
        }
    } else {
        PathBuf::from(path)
    };

    if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir().unwrap_or_default().join(expanded)
    }
}

/// Initialize progress tracking for a list of files, returning a `FileProgressInfo` per file.
pub async fn start_file_progress(
    files: &[PathBuf],
    user_id: &str,
    progress_service: &ProgressService,
) -> Vec<FileProgressInfo> {
    let mut infos = Vec::with_capacity(files.len());
    for file_path in files {
        let progress_id = uuid::Uuid::new_v4().to_string();
        let file_name = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        progress_service
            .start_progress(progress_id.clone(), user_id.to_string())
            .await;

        infos.push(FileProgressInfo {
            file_name,
            progress_id,
        });
    }
    infos
}

/// Spawn a single background task that processes files sequentially.
///
/// Files are ingested one at a time so that schema expansion works correctly:
/// each file sees the schema established by previous files, avoiding redundant
/// expansion chains and scattered data across schema versions.
#[allow(clippy::too_many_arguments)]
pub fn spawn_file_ingestion_tasks(
    files_with_progress: impl IntoIterator<Item = (PathBuf, String)>,
    progress_tracker: &ProgressTracker,
    node_arc: &Arc<crate::fold_node::FoldNode>,
    user_id: &str,
    auto_execute: bool,
    ingestion_service: Arc<IngestionService>,
    upload_storage: fold_db::storage::UploadStorage,
    encryption_key: [u8; 32],
    force_reingest: bool,
) {
    let files: Vec<_> = files_with_progress.into_iter().collect();
    let progress_tracker_clone = progress_tracker.clone();
    let node_arc_clone = node_arc.clone();
    let user_id_clone = user_id.to_string();

    tokio::spawn(
        async move {
            fold_db::logging::core::run_with_user(&user_id_clone, async move {
                for (file_path, progress_id) in files {
                    let progress_service = ProgressService::new(progress_tracker_clone.clone());

                    if let Err(e) = process_single_file_via_smart_folder(
                        &file_path,
                        &progress_id,
                        &progress_service,
                        &node_arc_clone,
                        auto_execute,
                        &ingestion_service,
                        &upload_storage,
                        &encryption_key,
                        force_reingest,
                        None,
                    )
                    .await
                    {
                        log_feature!(
                            LogFeature::Ingestion,
                            error,
                            "Failed to process file {}: {}",
                            file_path.display(),
                            e
                        );
                        progress_service
                            .fail_progress(&progress_id, format!("Processing failed: {}", e))
                            .await;
                    }
                }
            })
            .await
        }
        .instrument(tracing::Span::current()),
    );
}

/// Content-address the given file bytes: compute SHA256, encrypt the payload,
/// and write it to `upload_storage` under its hash. Returns the hex hash so the
/// caller can put it on the ingestion request (which ends up as
/// `metadata.file_hash` on the atom, letting the data browser fetch the file
/// back via `/api/file/<hash>` for inline previews).
///
/// `save_file_if_not_exists` is idempotent, so re-calling this for the same
/// bytes is cheap — duplicate photos won't double-encrypt or double-store.
pub async fn store_file_content_addressed(
    raw_bytes: &[u8],
    upload_storage: &fold_db::storage::UploadStorage,
    encryption_key: &[u8; 32],
) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let file_hash = format!("{:x}", Sha256::digest(raw_bytes));
    let encrypted = fold_db::crypto::envelope::encrypt_envelope(encryption_key, raw_bytes)
        .map_err(|e| format!("Failed to encrypt file: {}", e))?;
    upload_storage
        .save_file_if_not_exists(&file_hash, &encrypted, None)
        .await
        .map_err(|e| format!("Failed to store encrypted file: {}", e))?;
    Ok(file_hash)
}

/// Process a single file for smart ingest using shared smart_folder module.
/// Reads the file, computes its SHA256 hash, encrypts and stores in upload storage,
/// then ingests the JSON content with file_hash metadata.
#[allow(clippy::too_many_arguments)]
pub async fn process_single_file_via_smart_folder(
    file_path: &Path,
    progress_id: &str,
    progress_service: &ProgressService,
    node_arc: &Arc<crate::fold_node::FoldNode>,
    auto_execute: bool,
    service: &IngestionService,
    upload_storage: &fold_db::storage::UploadStorage,
    encryption_key: &[u8; 32],
    force_reingest: bool,
    org_hash: Option<&str>,
) -> Result<(), String> {
    // Try native parser first (handles json, js/Twitter, csv, txt, md),
    // fall back to file_to_markdown for unsupported types (images, PDFs, etc.)
    // The hash returned by either path is discarded — we re-derive it inside
    // `store_file_content_addressed` over the same bytes so the store/path
    // invariant lives in one place. The rehash cost is negligible vs. the AI
    // calls that dominate this path.
    let (data, raw_bytes, image_descriptive_name) =
        match crate::ingestion::smart_folder::read_file_with_hash(file_path) {
            Ok((data, _hash, bytes)) => (data, bytes, None),
            Err(_) => {
                let raw_bytes =
                    std::fs::read(file_path).map_err(|e| format!("Failed to read file: {}", e))?;
                let mut data =
                    crate::ingestion::file_handling::json_processor::convert_file_to_json(
                        &file_path.to_path_buf(),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                // Enrich image JSON with image_type and created_at for HashRange schema support
                let is_image_file = file_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .filter(|name| crate::ingestion::is_image_file(name));
                let image_descriptive_name = is_image_file.and_then(|name| {
                    crate::ingestion::file_handling::json_processor::enrich_image_json(
                        &mut data,
                        &file_path.to_path_buf(),
                        Some(name),
                    )
                });
                // Classify photo visibility using AI
                if is_image_file.is_some() {
                    crate::ingestion::file_handling::json_processor::classify_and_set_visibility(
                        &mut data, service,
                    )
                    .await;
                }
                (data, raw_bytes, image_descriptive_name)
            }
        };

    let file_hash =
        store_file_content_addressed(&raw_bytes, upload_storage, encryption_key).await?;

    let node = node_arc.as_ref();
    let pub_key = node.get_node_public_key().to_string();

    // Check per-user file dedup — skip entire pipeline if this user already ingested this file
    if !force_reingest {
        if let Some(record) = node.is_file_ingested(&pub_key, &file_hash).await {
            log_feature!(
                LogFeature::Ingestion,
                info,
                "File already ingested by this user (at {}), skipping: {}",
                record.ingested_at,
                file_path.display()
            );
            progress_service
                .update_progress(
                    progress_id,
                    crate::ingestion::IngestionStep::Completed,
                    format!("Skipped (already ingested at {})", record.ingested_at),
                )
                .await;
            return Ok(());
        }
    }

    // Pass raw image bytes for face detection
    let image_bytes = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|name| crate::ingestion::is_image_file(name))
        .map(|_| raw_bytes.clone());

    let request = IngestionRequest {
        data,
        auto_execute,
        pub_key,
        source_file_name: file_path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string()),
        progress_id: Some(progress_id.to_string()),
        file_hash: Some(file_hash),
        source_folder: file_path.parent().map(|p| p.to_string_lossy().to_string()),
        image_descriptive_name,
        org_hash: org_hash.map(|s| s.to_string()),
        image_bytes,
    };

    service
        .process_json_with_node_and_progress(
            request,
            node,
            progress_service,
            progress_id.to_string(),
        )
        .await
        .map_err(|e| e.user_message())?;

    Ok(())
}

/// Fetch and parse models from a remote Ollama instance.
///
/// Lives here (not in an HTTP module) so it can be called from any context.
pub async fn fetch_ollama_models(base_url: &str) -> Result<Vec<OllamaModelInfo>, String> {
    let url = format!("{}/api/tags", base_url);
    // trace-egress: skip-3p (Ollama, third-party)
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Failed to connect to Ollama at {}: {}", base_url, e))?;
    if !resp.status().is_success() {
        return Err(format!("Ollama returned status {}", resp.status()));
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse Ollama response: {}", e))?;
    Ok(body
        .get("models")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    let name = v.get("name")?.as_str()?.to_string();
                    let size = v.get("size").and_then(|s| s.as_u64()).unwrap_or(0);
                    Some(OllamaModelInfo { name, size })
                })
                .collect()
        })
        .unwrap_or_default())
}

/// A single model entry returned by the Ollama `/api/tags` endpoint.
#[derive(Debug, Serialize, Deserialize)]
pub struct OllamaModelInfo {
    pub name: String,
    pub size: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_resolve_folder_path_tilde() {
        let result = resolve_folder_path("~/Documents");
        let home = dirs::home_dir().expect("home_dir must exist for this test");
        assert_eq!(result, home.join("Documents"));
    }

    #[tokio::test]
    async fn test_resolve_folder_path_tilde_only() {
        let result = resolve_folder_path("~");
        let home = dirs::home_dir().expect("home_dir must exist for this test");
        assert_eq!(result, home);
    }

    #[tokio::test]
    async fn test_resolve_folder_path_absolute() {
        let result = resolve_folder_path("/tmp/test");
        assert_eq!(result, PathBuf::from("/tmp/test"));
    }
}
