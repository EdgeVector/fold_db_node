//! HTTP route handlers for the ingestion API.
//!
//! All actix-web glue for ingestion lives here. The pure pipeline logic is
//! parameterized and lives in `crate::ingestion`.

use crate::ingestion::helpers::{
    fetch_ollama_models, resolve_folder_path, spawn_file_ingestion_tasks, start_file_progress,
    validate_folder, BatchFolderResponse, FolderValidationError, OllamaModelInfo,
};
use crate::ingestion::ingestion_service::IngestionService;
use crate::ingestion::progress::ProgressService;
use crate::ingestion::service_state::{get_ingestion_service, IngestionServiceState};
use crate::ingestion::IngestionRequest;
use crate::ingestion::ProgressTracker;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_error_to_response, require_node, require_user_context};
use actix_web::{web, HttpResponse, Responder};
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

use crate::ingestion::batch_controller::{BatchControllerMap, BatchStatus, BatchStatusResponse};

// ── Shared helpers ──────────────────────────────────────────────────

/// Return a 503 response when the ingestion service is unavailable.
pub(crate) fn ingestion_unavailable() -> HttpResponse {
    HttpResponse::ServiceUnavailable().json(json!({
        "success": false,
        "error": "Ingestion service not available"
    }))
}

/// Convert a `FolderValidationError` into an HTTP 400 response.
pub(crate) fn folder_error_to_response(err: FolderValidationError) -> HttpResponse {
    HttpResponse::BadRequest().json(json!({
        "success": false,
        "error": err.to_string(),
    }))
}

/// Extract the user/node/ingestion triple that most ingestion handlers need.
pub(crate) async fn require_ingestion_context(
    state: &web::Data<AppState>,
    ingestion_service: &web::Data<IngestionServiceState>,
) -> Result<
    (
        String,
        Arc<tokio::sync::RwLock<crate::fold_node::FoldNode>>,
        Arc<IngestionService>,
    ),
    HttpResponse,
> {
    let (user_id, node_arc) = require_node(state).await?;
    let service = get_ingestion_service(ingestion_service.get_ref())
        .await
        .ok_or_else(ingestion_unavailable)?;
    Ok((user_id, node_arc, service))
}

// ── Core ingestion routes ──────────────────────────────────────────

/// Process JSON ingestion request
#[utoipa::path(
    post,
    path = "/api/ingestion/process",
    tag = "ingestion",
    request_body = IngestionRequest,
    responses((status = 200, description = "Ingestion response", body = IngestionResponse))
)]
pub async fn process_json(
    request: web::Json<IngestionRequest>,
    progress_tracker: web::Data<ProgressTracker>,
    state: web::Data<AppState>,
    ingestion_service: web::Data<IngestionServiceState>,
) -> impl Responder {
    log_feature!(
        LogFeature::Ingestion,
        info,
        "Received JSON ingestion request"
    );

    let (user_id, node_arc, service) =
        match require_ingestion_context(&state, &ingestion_service).await {
            Ok(ctx) => ctx,
            Err(response) => return response,
        };

    // Lock briefly — handler clones the node and spawns a background task
    let node = node_arc.read().await;

    match crate::handlers::ingestion::process_json(
        request.into_inner(),
        &user_id,
        progress_tracker.get_ref(),
        &node,
        service,
    )
    .await
    {
        Ok(api_response) => HttpResponse::Accepted().json(api_response.data),
        Err(e) => handler_error_to_response(e),
    }
}

/// Get ingestion status
#[utoipa::path(
    get,
    path = "/api/ingestion/status",
    tag = "ingestion",
    responses((status = 200, description = "Ingestion status", body = crate::ingestion::IngestionStatus))
)]
pub async fn get_status(ingestion_service: web::Data<IngestionServiceState>) -> impl Responder {
    log_feature!(
        LogFeature::Ingestion,
        debug,
        "Received ingestion status request"
    );

    match get_ingestion_service(ingestion_service.get_ref()).await {
        Some(service) => match service.get_status() {
            Ok(status) => HttpResponse::Ok().json(status),
            Err(e) => HttpResponse::InternalServerError().json(json!({
                "error": format!("Failed to get status: {}", e)
            })),
        },
        None => HttpResponse::ServiceUnavailable().json(json!({
            "error": "Ingestion service not available",
            "enabled": false,
            "configured": false
        })),
    }
}

/// Validate JSON data without processing
#[utoipa::path(
    post,
    path = "/api/ingestion/validate",
    tag = "ingestion",
    request_body = Value,
    responses((status = 200, description = "Validation result", body = Value), (status = 400, description = "Invalid"))
)]
pub async fn validate_json(
    request: web::Json<serde_json::Value>,
    ingestion_service: web::Data<IngestionServiceState>,
) -> impl Responder {
    log_feature!(
        LogFeature::Ingestion,
        info,
        "Received JSON validation request"
    );

    match get_ingestion_service(ingestion_service.get_ref()).await {
        Some(service) => match service.validate_input(&request.into_inner()) {
            Ok(()) => HttpResponse::Ok().json(json!({
                "valid": true,
                "message": "JSON data is valid for ingestion"
            })),
            Err(e) => HttpResponse::BadRequest().json(json!({
                "valid": false,
                "error": format!("Validation failed: {}", e)
            })),
        },
        None => HttpResponse::ServiceUnavailable().json(json!({
            "valid": false,
            "error": "Ingestion service not available"
        })),
    }
}

/// Get Ingestion configuration
#[utoipa::path(
    get,
    path = "/api/ingestion/config",
    tag = "ingestion",
    responses((status = 200, description = "Ingestion config", body = IngestionConfig))
)]
pub async fn get_ingestion_config() -> impl Responder {
    log_feature!(
        LogFeature::Ingestion,
        debug,
        "Received ingestion config request"
    );

    let config = crate::ingestion::config::IngestionConfig::load_or_default();
    HttpResponse::Ok().json(config.redacted())
}

/// Save Ingestion configuration
#[utoipa::path(
    post,
    path = "/api/ingestion/config",
    tag = "ingestion",
    request_body = SavedConfig,
    responses((status = 200, description = "Saved"), (status = 500, description = "Failed"))
)]
pub async fn save_ingestion_config(
    request: web::Json<crate::ingestion::config::SavedConfig>,
    ingestion_service: web::Data<IngestionServiceState>,
    llm_state: web::Data<crate::fold_node::llm_query::LlmQueryState>,
) -> impl Responder {
    log_feature!(
        LogFeature::Ingestion,
        info,
        "Received ingestion config save request"
    );

    let saved_config = request.into_inner();

    // AI config is per-device (saved to ingestion_config.json only, not Sled).
    // A laptop might run Ollama locally while a phone uses Anthropic's API.
    match crate::ingestion::config::IngestionConfig::save_to_file(&saved_config) {
        Ok(()) => {
            // Reload the IngestionService so the new config takes effect immediately.
            let reload_config = crate::ingestion::config::IngestionConfig::load_or_default();
            match IngestionService::new(reload_config) {
                Ok(new_service) => {
                    let mut guard = ingestion_service.write().await;
                    *guard = Some(Arc::new(new_service));
                    log_feature!(
                        LogFeature::Ingestion,
                        info,
                        "IngestionService reloaded with new configuration"
                    );
                }
                Err(e) => {
                    log_feature!(
                        LogFeature::Ingestion,
                        warn,
                        "Config saved but failed to reload IngestionService: {}. Service may be unavailable until restart.",
                        e
                    );
                }
            }
            // Also reload the LLM query service so model changes take effect
            llm_state.reload().await;

            HttpResponse::Ok().json(json!({
                "success": true,
                "message": "Configuration saved successfully"
            }))
        }
        Err(e) => HttpResponse::InternalServerError().json(json!({
            "success": false,
            "error": format!("Failed to save configuration: {}", e)
        })),
    }
}

/// Get ingestion progress by ID
#[utoipa::path(
    get,
    path = "/api/ingestion/progress/{id}",
    tag = "ingestion",
    responses((status = 200, description = "Progress information", body = IngestionProgress), (status = 404, description = "Progress not found"))
)]
pub async fn get_progress(
    path: web::Path<String>,
    progress_tracker: web::Data<ProgressTracker>,
) -> impl Responder {
    let id = path.into_inner();

    log_feature!(
        LogFeature::Ingestion,
        debug,
        "Received progress request for ID: {}",
        id
    );

    let user_hash = match require_user_context() {
        Ok(hash) => hash,
        Err(response) => return response,
    };

    match crate::handlers::ingestion::get_progress(&id, &user_hash, progress_tracker.get_ref())
        .await
    {
        Ok(api_response) => HttpResponse::Ok().json(api_response.data),
        Err(e) => handler_error_to_response(e),
    }
}

/// Get all active ingestion progress
#[utoipa::path(
    get,
    path = "/api/ingestion/progress",
    tag = "ingestion",
    responses((status = 200, description = "All active progress", body = Vec<IngestionProgress>))
)]
pub async fn get_all_progress(progress_tracker: web::Data<ProgressTracker>) -> impl Responder {
    log_feature!(
        LogFeature::Ingestion,
        debug,
        "Received request for all progress"
    );

    let user_hash = match require_user_context() {
        Ok(hash) => hash,
        Err(response) => return response,
    };

    match crate::handlers::ingestion::get_all_progress(&user_hash, progress_tracker.get_ref()).await
    {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// Lightweight progress summary — just counts, no per-job details.
#[utoipa::path(
    get,
    path = "/api/ingestion/progress/summary",
    tag = "ingestion",
    responses((status = 200, description = "Progress summary counts"))
)]
pub async fn get_progress_summary(progress_tracker: web::Data<ProgressTracker>) -> impl Responder {
    let user_hash = match require_user_context() {
        Ok(hash) => hash,
        Err(response) => return response,
    };

    let response =
        match crate::handlers::ingestion::get_all_progress(&user_hash, progress_tracker.get_ref())
            .await
        {
            Ok(r) => r,
            Err(e) => return handler_error_to_response(e),
        };

    let empty = Vec::new();
    let jobs = response
        .data
        .as_ref()
        .map(|d| &d.progress)
        .unwrap_or(&empty);
    let total = jobs.len();
    let done = jobs.iter().filter(|j| j.is_complete).count();
    let failed = jobs.iter().filter(|j| j.is_complete && j.is_failed).count();
    let passed = done - failed;

    HttpResponse::Ok().json(serde_json::json!({
        "total": total,
        "done": done,
        "passed": passed,
        "failed": failed,
        "running": total - done,
    }))
}

// ── Batch folder routes ────────────────────────────────────────────

/// Request for batch folder ingestion
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchFolderRequest {
    /// Path to the folder (relative to project root or absolute)
    pub folder_path: String,
    /// Optional schema hint for all files
    pub schema_hint: Option<String>,
    /// Whether to auto-execute mutations (default: true)
    pub auto_execute: Option<bool>,
}

/// Request to resume a paused batch
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchResumeRequest {
    pub batch_id: String,
    pub new_spend_limit: f64,
}

/// Request to cancel a batch
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchCancelRequest {
    pub batch_id: String,
}

/// Batch ingest all files from a folder
#[utoipa::path(
    post,
    path = "/api/ingestion/batch-folder",
    tag = "ingestion",
    request_body = BatchFolderRequest,
    responses((status = 202, description = "Batch ingestion started", body = BatchFolderResponse), (status = 400, description = "Invalid folder path"))
)]
pub async fn batch_folder_ingest(
    request: web::Json<BatchFolderRequest>,
    progress_tracker: web::Data<ProgressTracker>,
    state: web::Data<AppState>,
    ingestion_service: web::Data<IngestionServiceState>,
    upload_storage: web::Data<fold_db::storage::UploadStorage>,
) -> impl Responder {
    log_feature!(
        LogFeature::Ingestion,
        info,
        "Received batch folder ingestion request for: {}",
        request.folder_path
    );

    let (user_id, node_arc, service) =
        match require_ingestion_context(&state, &ingestion_service).await {
            Ok(ctx) => ctx,
            Err(response) => return response,
        };

    let folder_path = resolve_folder_path(&request.folder_path);

    if let Err(err) = validate_folder(&folder_path) {
        return folder_error_to_response(err);
    }

    // List supported files in the folder
    let supported_extensions = ["json", "csv", "txt", "md"];
    let mut files_to_ingest: Vec<std::path::PathBuf> = Vec::new();

    match std::fs::read_dir(&folder_path) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                        if supported_extensions.contains(&ext.to_lowercase().as_str()) {
                            files_to_ingest.push(path);
                        }
                    }
                }
            }
        }
        Err(e) => {
            return HttpResponse::BadRequest().json(json!({
                "success": false,
                "error": format!("Failed to read folder: {}", e)
            }));
        }
    }

    if files_to_ingest.is_empty() {
        return HttpResponse::BadRequest().json(json!({
            "success": false,
            "error": "No supported files found in folder (supported: .json, .csv, .txt, .md)"
        }));
    }

    let batch_id = uuid::Uuid::new_v4().to_string();

    let progress_service = ProgressService::new(progress_tracker.get_ref().clone());
    let file_progress_ids =
        start_file_progress(&files_to_ingest, &user_id, &progress_service).await;

    let auto_execute = request.auto_execute.unwrap_or(true);
    let encryption_key = {
        let node = node_arc.read().await;
        node.get_encryption_key()
    };

    spawn_file_ingestion_tasks(
        files_to_ingest
            .into_iter()
            .zip(file_progress_ids.iter())
            .map(|(path, info)| (path, info.progress_id.clone())),
        progress_tracker.get_ref(),
        &node_arc,
        &user_id,
        auto_execute,
        service,
        upload_storage.get_ref().clone(),
        encryption_key,
        false,
    );

    HttpResponse::Accepted().json(BatchFolderResponse {
        success: true,
        batch_id,
        files_found: file_progress_ids.len(),
        file_progress_ids,
        message: "Batch ingestion started. Use progress IDs to track individual file status."
            .to_string(),
    })
}

/// Get batch status
pub async fn get_batch_status(
    path: web::Path<String>,
    batch_controller_map: web::Data<BatchControllerMap>,
    progress_tracker: web::Data<ProgressTracker>,
) -> impl Responder {
    let batch_id = path.into_inner();
    let map_guard = batch_controller_map.lock().await;

    match map_guard.get(&batch_id) {
        Some(ctrl_arc) => {
            let ctrl = ctrl_arc.lock().await;
            let mut resp = BatchStatusResponse::from_controller(&ctrl);
            let progress_id = ctrl.in_flight_files.first().map(|f| f.progress_id.clone());
            drop(ctrl);
            drop(map_guard);

            if let Some(pid) = progress_id {
                match progress_tracker.load(&pid).await {
                    Ok(Some(job)) => {
                        resp.current_file_step = Some(job.message);
                        resp.current_file_progress = Some(job.progress_percentage);
                    }
                    _ => {
                        resp.current_file_step = Some("Processing...".to_string());
                        resp.current_file_progress = Some(0);
                    }
                }
            }
            HttpResponse::Ok().json(resp)
        }
        None => HttpResponse::NotFound().json(json!({
            "error": format!("Batch {} not found", batch_id)
        })),
    }
}

/// Resume a paused batch with a new spend limit
pub async fn resume_batch(
    request: web::Json<BatchResumeRequest>,
    batch_controller_map: web::Data<BatchControllerMap>,
) -> impl Responder {
    let map_guard = batch_controller_map.lock().await;

    match map_guard.get(&request.batch_id) {
        Some(ctrl_arc) => {
            let mut ctrl = ctrl_arc.lock().await;
            if ctrl.status != BatchStatus::Paused {
                return HttpResponse::BadRequest().json(json!({
                    "error": format!("Batch is not paused (status: {})", ctrl.status)
                }));
            }
            ctrl.resume(Some(request.new_spend_limit));
            HttpResponse::Ok().json(BatchStatusResponse::from_controller(&ctrl))
        }
        None => HttpResponse::NotFound().json(json!({
            "error": format!("Batch {} not found", request.batch_id)
        })),
    }
}

/// Cancel a batch
pub async fn cancel_batch(
    request: web::Json<BatchCancelRequest>,
    batch_controller_map: web::Data<BatchControllerMap>,
) -> impl Responder {
    let map_guard = batch_controller_map.lock().await;

    match map_guard.get(&request.batch_id) {
        Some(ctrl_arc) => {
            let mut ctrl = ctrl_arc.lock().await;
            ctrl.cancel();
            HttpResponse::Ok().json(BatchStatusResponse::from_controller(&ctrl))
        }
        None => HttpResponse::NotFound().json(json!({
            "error": format!("Batch {} not found", request.batch_id)
        })),
    }
}

// ── Ollama models proxy ────────────────────────────────────────────

/// Query parameters for the Ollama models endpoint.
#[derive(Debug, Deserialize)]
pub struct OllamaModelsQuery {
    pub base_url: String,
}

/// List models available on a remote Ollama instance.
pub async fn list_ollama_models(query: web::Query<OllamaModelsQuery>) -> impl Responder {
    let base_url = query.base_url.trim_end_matches('/');
    match fetch_ollama_models(base_url).await {
        Ok(models) => HttpResponse::Ok().json(json!({ "models": models })),
        Err(msg) => HttpResponse::Ok()
            .json(json!({ "models": Vec::<OllamaModelInfo>::new(), "error": msg })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{test, App};

    #[actix_web::test]
    async fn test_get_status() {
        let ingestion_service: IngestionServiceState = tokio::sync::RwLock::new(None);
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(ingestion_service))
                .route("/status", web::get().to(get_status)),
        )
        .await;

        let req = test::TestRequest::get().uri("/status").to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_server_error() || resp.status().is_success());
    }

    #[actix_web::test]
    async fn test_get_ingestion_config() {
        let app =
            test::init_service(App::new().route("/config", web::get().to(get_ingestion_config)))
                .await;

        let req = test::TestRequest::get().uri("/config").to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());
    }

    #[tokio::test]
    async fn test_batch_folder_request_serialization() {
        let request = BatchFolderRequest {
            folder_path: "sample_data".to_string(),
            schema_hint: Some("TestSchema".to_string()),
            auto_execute: Some(true),
        };

        let json = serde_json::to_string(&request).unwrap();
        let parsed: BatchFolderRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.folder_path, "sample_data");
        assert_eq!(parsed.schema_hint, Some("TestSchema".to_string()));
        assert_eq!(parsed.auto_execute, Some(true));
    }
}
