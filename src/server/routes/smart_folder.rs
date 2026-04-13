//! HTTP route handlers for smart-folder scan and ingest endpoints.

use crate::ingestion::batch_controller::{BatchController, BatchControllerMap, PendingFile};
use crate::ingestion::helpers::{
    resolve_folder_path, start_file_progress, validate_folder, BatchFolderResponse,
};
use crate::ingestion::progress::ProgressService;
use crate::ingestion::service_state::{get_ingestion_service, IngestionServiceState};
use crate::ingestion::smart_folder;
use crate::ingestion::smart_folder::batch::spawn_batch_coordinator;
use crate::ingestion::ProgressTracker;
use crate::server::http_server::AppState;
use crate::server::routes::ingestion::{folder_error_to_response, require_ingestion_context};
use crate::server::routes::{require_node, require_user_context};
use actix_web::{web, HttpResponse, Responder};
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use fold_db::progress::{Job, JobStatus, JobType};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Request for smart folder scanning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartFolderScanRequest {
    pub folder_path: String,
    pub max_depth: Option<usize>,
    pub max_files: Option<usize>,
}

/// Request for adjusting scan results via natural language
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdjustScanRequest {
    pub instruction: String,
    pub recommended_files: Vec<smart_folder::FileRecommendation>,
    pub skipped_files: Vec<smart_folder::FileRecommendation>,
}

/// Response from adjusting scan results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdjustScanResponse {
    pub success: bool,
    pub message: String,
    pub recommended_files: Vec<smart_folder::FileRecommendation>,
    pub skipped_files: Vec<smart_folder::FileRecommendation>,
    pub summary: std::collections::HashMap<String, usize>,
    pub total_estimated_cost: f64,
}

/// Request for smart folder ingestion (after user approval)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartFolderIngestRequest {
    pub folder_path: String,
    pub files_to_ingest: Vec<String>,
    pub auto_execute: Option<bool>,
    pub spend_limit: Option<f64>,
    pub file_costs: Option<Vec<f64>>,
    #[serde(default)]
    pub force_reingest: bool,
    pub max_concurrent: Option<usize>,
    pub org_hash: Option<String>,
}

/// Response from initiating an async scan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartFolderScanStartResponse {
    pub success: bool,
    pub progress_id: String,
}

/// Scan a folder and use LLM to recommend which files contain personal data.
#[utoipa::path(
    post,
    path = "/api/ingestion/smart-folder/scan",
    tag = "ingestion",
    request_body = SmartFolderScanRequest,
    responses(
        (status = 202, description = "Scan started", body = SmartFolderScanStartResponse),
        (status = 400, description = "Invalid folder path"),
    )
)]
pub async fn smart_folder_scan(
    request: web::Json<SmartFolderScanRequest>,
    state: web::Data<AppState>,
    ingestion_service: web::Data<IngestionServiceState>,
    progress_tracker: web::Data<ProgressTracker>,
) -> impl Responder {
    log_feature!(
        LogFeature::Ingestion,
        info,
        "Smart folder scan requested for: {}",
        request.folder_path
    );

    let user_id = match require_user_context() {
        Ok(hash) => hash,
        Err(response) => return response,
    };

    let folder_path = resolve_folder_path(&request.folder_path);

    if let Err(err) = validate_folder(&folder_path) {
        return folder_error_to_response(err);
    }

    let max_depth = request.max_depth.unwrap_or(10);
    let max_files = request.max_files.unwrap_or(100);

    let progress_id = uuid::Uuid::new_v4().to_string();
    let tracker = progress_tracker.get_ref().clone();

    let mut job = Job::new(progress_id.clone(), JobType::Other("scan".to_string()));
    job = job.with_user(user_id.clone());
    job.message = "Starting scan...".to_string();
    job.progress_percentage = 0;
    if let Err(e) = tracker.save(&job).await {
        log::warn!("Failed to save scan progress: {}", e);
    }

    let node_arc = require_node(&state).await.ok().map(|(_uid, arc)| arc);
    let service_opt = get_ingestion_service(ingestion_service.get_ref()).await;

    let pid = progress_id.clone();
    tokio::spawn(async move {
        let user_id_inner = user_id.clone();
        fold_db::logging::core::run_with_user(&user_id, async move {
            let tracker_cb = tracker.clone();
            let pid_cb = pid.clone();
            let progress_user_id = fold_db::logging::core::get_current_user_id()
                .unwrap_or_else(|| user_id_inner.clone());
            let on_progress: smart_folder::ScanProgressFn = Box::new(move |pct, msg| {
                let tracker_inner = tracker_cb.clone();
                let pid_inner = pid_cb.clone();
                let uid = progress_user_id.clone();
                tokio::spawn(async move {
                    fold_db::logging::core::run_with_user(&uid, async move {
                        if let Ok(Some(mut job)) = tracker_inner.load(&pid_inner).await {
                            job.update_progress(pct, msg);
                            let _ = tracker_inner.save(&job).await;
                        }
                    })
                    .await
                });
            });

            let node_ref = node_arc.as_deref();
            let result = smart_folder::perform_smart_folder_scan_with_progress(
                &folder_path,
                max_depth,
                max_files,
                service_opt.as_deref(),
                node_ref,
                Some(&on_progress),
            )
            .await;

            tokio::task::yield_now().await;
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            if let Ok(Some(mut job)) = tracker.load(&pid).await {
                match result {
                    Ok(response) => {
                        let result_json = serde_json::to_value(&response).ok();
                        job.complete(result_json);
                    }
                    Err(e) => {
                        job.fail(e.to_string());
                    }
                }
                let _ = tracker.save(&job).await;
            }
        })
        .await
    });

    HttpResponse::Accepted().json(SmartFolderScanStartResponse {
        success: true,
        progress_id,
    })
}

/// Retrieve the scan result after progress reaches 100%.
#[utoipa::path(
    get,
    path = "/api/ingestion/smart-folder/scan/{id}",
    tag = "ingestion",
    responses(
        (status = 200, description = "Scan result", body = SmartFolderScanResponse),
        (status = 404, description = "Scan not found or not complete"),
    )
)]
pub async fn get_scan_result(
    path: web::Path<String>,
    progress_tracker: web::Data<ProgressTracker>,
) -> impl Responder {
    let progress_id = path.into_inner();
    let tracker = progress_tracker.get_ref();

    match tracker.load(&progress_id).await {
        Ok(Some(job)) => match job.status {
            JobStatus::Failed => HttpResponse::InternalServerError().json(json!({
                "success": false,
                "error": job.error.unwrap_or_else(|| "Scan failed".to_string())
            })),
            JobStatus::Completed => match job.result {
                Some(result) => HttpResponse::Ok().json(result),
                None => HttpResponse::InternalServerError().json(json!({
                    "success": false,
                    "error": "Scan completed but result is unavailable"
                })),
            },
            _ => HttpResponse::NotFound().json(json!({
                "success": false,
                "error": "Scan not yet complete"
            })),
        },
        _ => HttpResponse::NotFound().json(json!({
            "success": false,
            "error": "Scan not found"
        })),
    }
}

/// Ingest files from a smart folder scan (after user approval)
#[utoipa::path(
    post,
    path = "/api/ingestion/smart-folder/ingest",
    tag = "ingestion",
    request_body = SmartFolderIngestRequest,
    responses(
        (status = 202, description = "Batch ingestion started", body = BatchFolderResponse),
        (status = 400, description = "Invalid request")
    )
)]
pub async fn smart_folder_ingest(
    request: web::Json<SmartFolderIngestRequest>,
    progress_tracker: web::Data<ProgressTracker>,
    state: web::Data<AppState>,
    ingestion_service: web::Data<IngestionServiceState>,
    batch_controller_map: web::Data<BatchControllerMap>,
    upload_storage: web::Data<fold_db::storage::UploadStorage>,
) -> impl Responder {
    log_feature!(
        LogFeature::Ingestion,
        info,
        "Smart folder ingest requested for {} files (spend_limit: {:?})",
        request.files_to_ingest.len(),
        request.spend_limit
    );

    let folder_path = resolve_folder_path(&request.folder_path);

    let (user_id, node_arc, service) =
        match require_ingestion_context(&state, &ingestion_service).await {
            Ok(ctx) => ctx,
            Err(response) => return response,
        };

    let file_costs = request.file_costs.as_deref();
    let mut files_to_process: Vec<std::path::PathBuf> = Vec::new();
    let mut costs: Vec<f64> = Vec::new();
    for (i, relative_path) in request.files_to_ingest.iter().enumerate() {
        let full_path = folder_path.join(relative_path);
        if full_path.exists() && full_path.is_file() {
            let cost = file_costs
                .and_then(|c| c.get(i).copied())
                .unwrap_or_else(|| {
                    smart_folder::estimate_file_cost(Path::new(relative_path), &folder_path)
                });
            files_to_process.push(full_path);
            costs.push(cost);
        } else {
            log_feature!(
                LogFeature::Ingestion,
                warn,
                "Skipping non-existent file: {}",
                full_path.display()
            );
        }
    }

    if files_to_process.is_empty() {
        return HttpResponse::BadRequest().json(json!({
            "success": false,
            "error": "No valid files to ingest"
        }));
    }

    let batch_id = uuid::Uuid::new_v4().to_string();

    let progress_service = ProgressService::new(progress_tracker.get_ref().clone());
    let file_progress_ids =
        start_file_progress(&files_to_process, &user_id, &progress_service).await;

    let auto_execute = request.auto_execute.unwrap_or(true);
    let force_reingest = request.force_reingest;

    let pending_files: Vec<PendingFile> = files_to_process
        .iter()
        .zip(file_progress_ids.iter())
        .zip(costs.iter())
        .map(|((path, info), &cost)| PendingFile {
            path: path.clone(),
            progress_id: info.progress_id.clone(),
            estimated_cost: cost,
        })
        .collect();

    let is_local = service.is_local_provider();
    let controller = BatchController::new(
        batch_id.clone(),
        request.spend_limit,
        pending_files,
        is_local,
    );
    let ctrl_arc = Arc::new(Mutex::new(controller));

    {
        let mut map_guard = batch_controller_map.lock().await;
        map_guard.insert(batch_id.clone(), ctrl_arc);
    }

    let encryption_key = {
        let node = node_arc.as_ref();
        node.get_encryption_key()
    };

    // Default to 2 concurrent files.  Each file triggers an Ollama inference
    // call for schema recommendation, and Ollama processes requests serially.
    let max_concurrent = request.max_concurrent.unwrap_or(2).clamp(1, 100);

    spawn_batch_coordinator(
        batch_id.clone(),
        batch_controller_map.get_ref().clone(),
        progress_tracker.get_ref(),
        &node_arc,
        &user_id,
        auto_execute,
        service,
        upload_storage.get_ref().clone(),
        encryption_key,
        force_reingest,
        max_concurrent,
        request.org_hash.clone(),
    );

    HttpResponse::Accepted().json(BatchFolderResponse {
        success: true,
        batch_id,
        files_found: file_progress_ids.len(),
        file_progress_ids,
        message: "Smart folder ingestion started with spend tracking.".to_string(),
    })
}

/// Adjust scan results using a natural language instruction.
#[utoipa::path(
    post,
    path = "/api/ingestion/smart-folder/adjust",
    tag = "ingestion",
    request_body = AdjustScanRequest,
    responses(
        (status = 200, description = "Adjusted scan results", body = AdjustScanResponse),
        (status = 400, description = "Invalid request"),
        (status = 500, description = "LLM error"),
    )
)]
pub async fn adjust_scan_results(
    request: web::Json<AdjustScanRequest>,
    ingestion_service: web::Data<IngestionServiceState>,
) -> impl Responder {
    log_feature!(
        LogFeature::Ingestion,
        info,
        "Adjust scan results: instruction='{}', {} recommended, {} skipped",
        request.instruction,
        request.recommended_files.len(),
        request.skipped_files.len(),
    );

    let service = match get_ingestion_service(ingestion_service.get_ref()).await {
        Some(s) => s,
        None => {
            return HttpResponse::InternalServerError().json(json!({
                "success": false,
                "message": "AI service not configured"
            }));
        }
    };

    let prompt = smart_folder::create_adjust_prompt(
        &request.instruction,
        &request.recommended_files,
        &request.skipped_files,
    );

    let llm_response = match smart_folder::call_llm_for_file_analysis(&prompt, &service).await {
        Ok(r) => r,
        Err(e) => {
            log::error!("LLM error during scan adjustment: {}", e);
            return HttpResponse::InternalServerError().json(json!({
                "success": false,
                "message": format!("AI error: {}", e)
            }));
        }
    };

    let all_files: Vec<smart_folder::FileRecommendation> = request
        .recommended_files
        .iter()
        .chain(request.skipped_files.iter())
        .cloned()
        .collect();

    let all_paths: Vec<String> = all_files.iter().map(|f| f.path.clone()).collect();

    match smart_folder::parse_llm_file_recommendations(&llm_response, &all_paths) {
        Ok(updated_recs) => {
            let updated = smart_folder::merge_adjust_results(&all_files, &updated_recs);

            let mut recommended = Vec::new();
            let mut skipped = Vec::new();
            let mut summary: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();
            let mut total_cost = 0.0;

            for file in updated {
                *summary.entry(file.category.clone()).or_insert(0) += 1;
                if file.should_ingest {
                    total_cost += file.estimated_cost;
                    recommended.push(file);
                } else {
                    skipped.push(file);
                }
            }

            let message = format!(
                "Updated: {} files to ingest, {} skipped.",
                recommended.len(),
                skipped.len()
            );

            HttpResponse::Ok().json(AdjustScanResponse {
                success: true,
                message,
                recommended_files: recommended,
                skipped_files: skipped,
                summary,
                total_estimated_cost: total_cost,
            })
        }
        Err(e) => {
            log::error!("Failed to parse LLM adjustment response: {}", e);
            HttpResponse::InternalServerError().json(json!({
                "success": false,
                "message": format!("Failed to parse AI response: {}", e)
            }))
        }
    }
}
