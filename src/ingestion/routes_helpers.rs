//! Shared utilities, types, and file-processing helpers used by ingestion route handlers.

use crate::ingestion::ingestion_service::IngestionService;
use crate::ingestion::progress::ProgressService;
use crate::ingestion::IngestionRequest;
use crate::ingestion::ProgressTracker;
use crate::server::http_server::AppState;
use crate::server::routes::require_node;
use actix_web::{web, HttpResponse, Responder};
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Return a 503 response when the ingestion service is unavailable.
pub(crate) fn ingestion_unavailable() -> HttpResponse {
    HttpResponse::ServiceUnavailable().json(json!({
        "success": false,
        "error": "Ingestion service not available"
    }))
}

/// Shared ingestion service state — wrapped in RwLock so config saves can reload it.
pub type IngestionServiceState = tokio::sync::RwLock<Option<Arc<IngestionService>>>;

/// Extract the user/node/ingestion triple that most ingestion handlers need.
pub(crate) async fn require_ingestion_context(
    state: &web::Data<AppState>,
    ingestion_service: &web::Data<IngestionServiceState>,
) -> Result<(String, Arc<tokio::sync::RwLock<crate::fold_node::FoldNode>>, Arc<IngestionService>), HttpResponse> {
    let (user_id, node_arc) = require_node(state).await?;
    let service = get_ingestion_service(ingestion_service)
        .await
        .ok_or_else(ingestion_unavailable)?;
    Ok((user_id, node_arc, service))
}

/// Helper to get a clone of the current IngestionService Arc from the RwLock.
pub async fn get_ingestion_service(
    state: &web::Data<IngestionServiceState>,
) -> Option<Arc<IngestionService>> {
    state.read().await.clone()
}

/// Resolve a folder path — expands `~` to the home directory, absolute paths
/// pass through, relative paths are resolved against the current working directory.
pub(crate) fn resolve_folder_path(path: &str) -> PathBuf {
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

/// Initialize progress tracking for a list of files, returning a FileProgressInfo per file.
pub(crate) async fn start_file_progress(
    files: &[std::path::PathBuf],
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

/// Validate that a path exists and is a directory, returning an error HttpResponse if not.
pub(crate) fn validate_folder(path: &Path) -> Result<(), HttpResponse> {
    if !path.exists() {
        return Err(HttpResponse::BadRequest().json(json!({
            "success": false,
            "error": format!("Folder not found: {}", path.display())
        })));
    }
    if !path.is_dir() {
        return Err(HttpResponse::BadRequest().json(json!({
            "success": false,
            "error": format!("Path is not a directory: {}", path.display())
        })));
    }
    Ok(())
}

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

/// Spawn a single background task that processes files sequentially.
///
/// Files are ingested one at a time so that schema expansion works correctly:
/// each file sees the schema established by previous files, avoiding redundant
/// expansion chains and scattered data across schema versions.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_file_ingestion_tasks(
    files_with_progress: impl IntoIterator<Item = (std::path::PathBuf, String)>,
    progress_tracker: &ProgressTracker,
    node_arc: &std::sync::Arc<tokio::sync::RwLock<crate::fold_node::FoldNode>>,
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

    tokio::spawn(async move {
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
    });
}

/// Process a single file for smart ingest using shared smart_folder module.
/// Reads the file, computes its SHA256 hash, encrypts and stores in upload storage,
/// then ingests the JSON content with file_hash metadata.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn process_single_file_via_smart_folder(
    file_path: &std::path::Path,
    progress_id: &str,
    progress_service: &ProgressService,
    node_arc: &std::sync::Arc<tokio::sync::RwLock<crate::fold_node::FoldNode>>,
    auto_execute: bool,
    service: &IngestionService,
    upload_storage: &fold_db::storage::UploadStorage,
    encryption_key: &[u8; 32],
    force_reingest: bool,
) -> Result<(), String> {
    // Try native parser first (handles json, js/Twitter, csv, txt, md),
    // fall back to file_to_json for unsupported types (images, PDFs, etc.)
    let (data, file_hash, raw_bytes, image_descriptive_name) = match crate::ingestion::smart_folder::read_file_with_hash(
        file_path,
    ) {
        Ok(result) => {
            let (data, hash, bytes) = result;
            (data, hash, bytes, None)
        }
        Err(_) => {
            let raw_bytes = std::fs::read(file_path)
                .map_err(|e| format!("Failed to read file: {}", e))?;
            let hash_hex = {
                use sha2::{Digest, Sha256};
                format!("{:x}", Sha256::digest(&raw_bytes))
            };
            let mut data =
                crate::ingestion::json_processor::convert_file_to_json(&file_path.to_path_buf())
                    .await
                    .map_err(|e| e.to_string())?;
            // Enrich image JSON with image_type and created_at for HashRange schema support
            let image_descriptive_name = file_path
                .file_name()
                .and_then(|n| n.to_str())
                .filter(|name| crate::ingestion::is_image_file(name))
                .and_then(|name| {
                    crate::ingestion::json_processor::enrich_image_json(
                        &mut data,
                        &file_path.to_path_buf(),
                        Some(name),
                    )
                });
            (data, hash_hex, raw_bytes, image_descriptive_name)
        }
    };

    // Encrypt and store the raw file in upload storage (content-addressed)
    let encrypted_data = fold_db::crypto::envelope::encrypt_envelope(encryption_key, &raw_bytes)
        .map_err(|e| format!("Failed to encrypt file: {}", e))?;
    // Content-addressed: user_id=None (same file = same hash = same object)
    upload_storage
        .save_file_if_not_exists(&file_hash, &encrypted_data, None)
        .await
        .map_err(|e| format!("Failed to store encrypted file: {}", e))?;

    let node = node_arc.read().await;
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
        source_folder: file_path
            .parent()
            .map(|p| p.to_string_lossy().to_string()),
        image_descriptive_name,
    };

    service
        .process_json_with_node_and_progress(
            request,
            &node,
            progress_service,
            progress_id.to_string(),
        )
        .await
        .map_err(|e| e.user_message())?;

    Ok(())
}

/// Query parameters for the Ollama models endpoint.
#[derive(Debug, Deserialize)]
pub struct OllamaModelsQuery {
    pub base_url: String,
}

/// A single model entry returned by the Ollama `/api/tags` endpoint.
#[derive(Debug, Serialize, Deserialize)]
pub struct OllamaModelInfo {
    pub name: String,
    pub size: u64,
}

/// Return a 200 response with an empty models list and an error message.
fn ollama_models_error(msg: String) -> HttpResponse {
    HttpResponse::Ok().json(json!({ "models": [], "error": msg }))
}

/// List models available on a remote Ollama instance.
///
/// Proxies `GET {base_url}/api/tags` and returns the model list.
/// Short timeout (5 s) to avoid hanging on unreachable servers.
pub async fn list_ollama_models(query: web::Query<OllamaModelsQuery>) -> impl Responder {
    let base_url = query.base_url.trim_end_matches('/');

    let url = format!("{}/api/tags", base_url);

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => return ollama_models_error(format!("Failed to create HTTP client: {}", e)),
    };

    match client.get(&url).send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                return ollama_models_error(format!("Ollama returned status {}", resp.status()));
            }
            match resp.json::<serde_json::Value>().await {
                Ok(body) => {
                    let models: Vec<OllamaModelInfo> = body
                        .get("models")
                        .and_then(|m| m.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| {
                                    let name = v.get("name")?.as_str()?.to_string();
                                    let size = v
                                        .get("size")
                                        .and_then(|s| s.as_u64())
                                        .unwrap_or(0);
                                    Some(OllamaModelInfo { name, size })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    HttpResponse::Ok().json(json!({ "models": models }))
                }
                Err(e) => ollama_models_error(format!("Failed to parse Ollama response: {}", e)),
            }
        }
        Err(e) => ollama_models_error(format!("Failed to connect to Ollama at {}: {}", base_url, e)),
    }
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
