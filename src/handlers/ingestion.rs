//! Shared Ingestion Handlers
//!
//! Framework-agnostic handlers for ingestion operations.
//! These can be called by both HTTP server routes and Lambda handlers.

use crate::fold_node::node::FoldNode;
use crate::handlers::handler_response;
use crate::handlers::response::{ApiResponse, HandlerError, HandlerResult, IntoHandlerError};
use crate::ingestion::ingestion_service::IngestionService;
use crate::ingestion::progress::{IngestionProgress, ProgressService, ProgressTracker};
use crate::ingestion::IngestionRequest;
use fold_db::progress::JobType;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

// ============================================================================
// Request/Response Types
// ============================================================================

/// Re-export IngestionRequest as ProcessJsonRequest for backward compatibility
/// with Lambda handlers in exemem-infra.
pub type ProcessJsonRequest = IngestionRequest;

handler_response! {
    /// Response for process_json (immediate response)
    pub struct ProcessJsonResponse {
        pub success: bool,
        pub progress_id: String,
        pub message: String,
    }
}

/// Response type for get_all_progress
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressListResponse {
    /// List of progress items
    pub progress: Vec<IngestionProgress>,
}

handler_response! {
    /// Response for ingestion status
    pub struct IngestionStatusResponse {
        pub enabled: bool,
        pub configured: bool,
        pub provider: String,
        pub model: String,
        pub auto_execute_mutations: bool,
    }
}

// ============================================================================
// Handler Functions
// ============================================================================

/// Get all ingestion/indexing progress for a user
///
/// # Arguments
/// * `user_hash` - The user's hash for isolation
/// * `tracker` - Progress tracker instance
///
/// # Returns
/// * `HandlerResult<ProgressListResponse>` - List of progress items wrapped in standard envelope
pub async fn get_all_progress(
    user_hash: &str,
    tracker: &ProgressTracker,
) -> HandlerResult<ProgressListResponse> {
    let jobs = tracker
        .list_by_user(user_hash)
        .await
        .handler_err("list progress")?;

    let progress: Vec<IngestionProgress> = jobs
        .into_iter()
        .filter(|j| matches!(j.job_type, JobType::Ingestion | JobType::Indexing))
        .map(|j| j.into())
        .collect();

    Ok(ApiResponse::success_with_user(
        ProgressListResponse { progress },
        user_hash,
    ))
}

/// Get progress for a specific job
///
/// # Arguments
/// * `id` - The progress ID
/// * `user_hash` - The user's hash for isolation
/// * `tracker` - Progress tracker instance
///
/// # Returns
/// * `HandlerResult<IngestionProgress>` - Progress item wrapped in standard envelope
pub async fn get_progress(
    id: &str,
    user_hash: &str,
    tracker: &ProgressTracker,
) -> HandlerResult<IngestionProgress> {
    let progress_service = ProgressService::new(tracker.clone());

    match progress_service.get_progress(id).await {
        Some(progress) => Ok(ApiResponse::success_with_user(progress, user_hash)),
        None => Err(HandlerError::NotFound(format!(
            "Progress not found for ID: {}",
            id
        ))),
    }
}

/// Get ingestion service status
///
/// # Arguments
/// * `user_hash` - The user's hash for context
///
/// # Returns
/// * `HandlerResult<IngestionStatusResponse>` - Status wrapped in standard envelope
pub async fn get_status(
    user_hash: &str,
    service: Option<&IngestionService>,
) -> HandlerResult<IngestionStatusResponse> {
    match service {
        Some(service) => match service.get_status() {
            Ok(status) => Ok(ApiResponse::success_with_user(
                IngestionStatusResponse {
                    enabled: status.enabled,
                    configured: status.configured,
                    provider: status.provider.clone(),
                    model: status.model,
                    auto_execute_mutations: status.auto_execute_mutations,
                },
                user_hash,
            )),
            Err(e) => Err(HandlerError::Internal(format!(
                "Failed to get status: {}",
                e
            ))),
        },
        None => {
            // Return a disabled status rather than an error
            Ok(ApiResponse::success_with_user(
                IngestionStatusResponse {
                    enabled: false,
                    configured: false,
                    provider: "None".to_string(),
                    model: "".to_string(),
                    auto_execute_mutations: false,
                },
                user_hash,
            ))
        }
    }
}

/// Process JSON ingestion (starts background task and returns immediately)
///
/// This is the shared handler for JSON ingestion. It:
/// 1. Validates the input data
/// 2. Starts a progress tracking job
/// 3. Spawns background ingestion
/// 4. Returns immediately with progress_id
///
/// # Arguments
/// * `request` - The ingestion request with data and options
/// * `user_hash` - The user's hash for isolation
/// * `tracker` - Progress tracker
/// * `node` - The FoldDB node
///
/// # Returns
/// * `HandlerResult<ProcessJsonResponse>` - Response with progress_id
pub async fn process_json(
    request: IngestionRequest,
    user_hash: &str,
    tracker: &ProgressTracker,
    node: &FoldNode,
    service: Arc<IngestionService>,
) -> HandlerResult<ProcessJsonResponse> {
    // Validate data is not empty
    if request.data.is_null() {
        return Err(HandlerError::BadRequest("Data cannot be null".to_string()));
    }

    if let Value::Object(ref obj) = request.data {
        if obj.is_empty() {
            return Err(HandlerError::BadRequest("Data cannot be empty".to_string()));
        }
    }

    // Validate org_hash if provided
    if let Some(ref org_hash) = request.org_hash {
        if org_hash.len() != 64 || !org_hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(HandlerError::BadRequest(
                "Invalid org_hash format — expected 64-character hex string".to_string(),
            ));
        }

        // Require Exemem config for org ingestion
        crate::handlers::org::require_exemem(node).await?;

        // Verify the org exists locally
        let sled_db = crate::handlers::org::get_sled_db(node).await?;
        let org = fold_db::org::operations::get_org(&sled_db, org_hash)
            .handler_err("check org membership")?;
        if org.is_none() {
            return Err(HandlerError::BadRequest(format!(
                "Not a member of organization '{}'",
                org_hash
            )));
        }
    }

    // Generate or use provided progress_id
    let progress_id = request
        .progress_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    // Start progress tracking
    let progress_service = ProgressService::new(tracker.clone());
    progress_service
        .start_progress(progress_id.clone(), user_hash.to_string())
        .await;

    // Clone what we need for the background task
    let is_org_ingestion = request.org_hash.is_some();
    let node_clone = node.clone();
    let progress_id_clone = progress_id.clone();
    let user_hash_clone = user_hash.to_string();
    let tracker_clone = tracker.clone();

    // Spawn background ingestion
    tokio::spawn(async move {
        fold_db::logging::core::run_with_user(&user_hash_clone, async move {
            let progress_service = ProgressService::new(tracker_clone);

            match service
                .process_json_with_node_and_progress(
                    request,
                    &node_clone,
                    &progress_service,
                    progress_id_clone.clone(),
                )
                .await
            {
                Ok(response) => {
                    if !response.success {
                        fold_db::log_feature!(
                            fold_db::logging::features::LogFeature::Ingestion,
                            error,
                            "Background ingestion failed: {:?}",
                            response.errors
                        );
                    } else if is_org_ingestion {
                        // Trigger immediate sync so org data uploads right away
                        // instead of waiting for the next timer-based sync cycle.
                        node_clone.trigger_immediate_sync().await;
                    }
                }
                Err(e) => {
                    fold_db::log_feature!(
                        fold_db::logging::features::LogFeature::Ingestion,
                        error,
                        "Background ingestion processing failed: {}",
                        e
                    );
                    progress_service
                        .fail_progress(&progress_id_clone, e.user_message())
                        .await;
                }
            }
        })
        .await;
    });

    // Return immediately with progress_id
    Ok(ApiResponse::success_with_user(
        ProcessJsonResponse {
            success: true,
            progress_id,
            message: "Ingestion started. Use progress_id to track status.".to_string(),
        },
        user_hash,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_list_response_serialization() {
        let response = ProgressListResponse { progress: vec![] };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("progress"));
    }
}
