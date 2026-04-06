//! HTTP route handlers for the ingestion API

use crate::ingestion::ingestion_service::IngestionService;
use crate::ingestion::IngestionRequest;
use crate::ingestion::ProgressTracker;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_error_to_response, require_user_context};
use actix_web::{web, HttpResponse, Responder};
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use serde_json::json;
use std::sync::Arc;

// Re-export from sibling modules so external callers (http_server.rs) can still
// reference everything through `crate::ingestion::routes::*`.
pub use super::apple_import::routes as apple_import_routes;
pub use super::routes_batch::*;
pub use super::routes_helpers::*;
pub use super::smart_folder::routes::*;

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

    match get_ingestion_service(&ingestion_service).await {
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

    match get_ingestion_service(&ingestion_service).await {
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

    match crate::ingestion::config::IngestionConfig::save_to_file(&request.into_inner()) {
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

    // Get user from context - required for multi-tenancy
    let user_hash = match crate::server::routes::require_user_context() {
        Ok(hash) => hash,
        Err(response) => return response,
    };

    // Use shared handler
    match crate::handlers::ingestion::get_all_progress(&user_hash, progress_tracker.get_ref()).await
    {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// Lightweight progress summary — just counts, no per-job details.
/// Use this for polling in scripts instead of the full progress endpoint.
#[utoipa::path(
    get,
    path = "/api/ingestion/progress/summary",
    tag = "ingestion",
    responses((status = 200, description = "Progress summary counts"))
)]
pub async fn get_progress_summary(
    progress_tracker: web::Data<ProgressTracker>,
) -> impl Responder {
    let user_hash = match crate::server::routes::require_user_context() {
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
    let jobs = response.data.as_ref().map(|d| &d.progress).unwrap_or(&empty);
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
        // Should return service unavailable if not configured
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
}
