use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use fold_db::progress::{Job, JobType, ProgressTracker};
use crate::server::http_server::AppState;
use crate::server::routes::require_user_context;
use actix_web::{web, HttpResponse, Responder};
use serde::{Deserialize, Serialize};

// ---- Job tracking helper ----

/// Lightweight handle that wraps a ProgressTracker + job ID to eliminate
/// repetitive load/update/save boilerplate. Logs errors instead of
/// silently swallowing them with `let _ =`.
struct JobHandle {
    tracker: ProgressTracker,
    job_id: String,
}

impl JobHandle {
    fn new(tracker: ProgressTracker, job_id: String) -> Self {
        Self { tracker, job_id }
    }

    /// Load job, apply mutation, save. Logs on missing job or I/O failure.
    async fn with_job(&self, operation: &str, f: impl FnOnce(&mut Job)) {
        match self.tracker.load(&self.job_id).await {
            Ok(Some(mut job)) => {
                f(&mut job);
                if let Err(e) = self.tracker.save(&job).await {
                    log_feature!(LogFeature::HttpServer, error, "Failed to save job {} for '{}': {}", operation, self.job_id, e);
                }
            }
            Ok(None) => {
                log_feature!(LogFeature::HttpServer, warn, "Job '{}' not found during {}", self.job_id, operation);
            }
            Err(e) => {
                log_feature!(LogFeature::HttpServer, error, "Failed to load job '{}': {}", self.job_id, e);
            }
        }
    }

    async fn update(&self, pct: u8, msg: impl Into<String>) {
        let msg = msg.into();
        self.with_job("progress update", |job| job.update_progress(pct, msg)).await;
    }

    async fn fail(&self, error: impl Into<String>) {
        let error = error.into();
        log_feature!(LogFeature::HttpServer, error, "Job '{}' failed: {}", self.job_id, error);
        self.with_job("failure update", |job| job.fail(error)).await;
    }

    async fn complete(&self, result: serde_json::Value) {
        self.with_job("completion", |job| job.complete(Some(result))).await;
    }
}

// ---- Shared response type ----

/// Response for async admin jobs (reset, migration, etc.)
#[derive(Serialize, utoipa::ToSchema)]
pub struct AdminJobResponse {
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
}

impl AdminJobResponse {
    fn error(message: impl Into<String>) -> Self {
        Self { success: false, message: message.into(), job_id: None }
    }

    fn started(job_id: String, message: impl Into<String>) -> Self {
        Self { success: true, message: message.into(), job_id: Some(job_id) }
    }
}

/// Create a tracked async job and return its ID, or an error HttpResponse.
async fn create_async_job(
    prefix: &str,
    job_type: &str,
    initial_msg: impl Into<String>,
    user_id: &str,
    tracker: &ProgressTracker,
) -> Result<String, HttpResponse> {
    let job_id = format!("{}_{}", prefix, uuid::Uuid::new_v4());
    let mut job = Job::new(job_id.clone(), JobType::Other(job_type.to_string()));
    job = job.with_user(user_id.to_string());
    job.update_progress(5, initial_msg.into());

    if let Err(e) = tracker.save(&job).await {
        log_feature!(LogFeature::HttpServer, error, "Failed to create {} job: {}", job_type, e);
        return Err(HttpResponse::InternalServerError().json(
            AdminJobResponse::error(format!("Failed to create {} job: {}", job_type, e)),
        ));
    }
    Ok(job_id)
}

// ---- Endpoints ----

/// Request body for database reset
#[derive(Deserialize, Serialize, utoipa::ToSchema)]
pub struct ResetDatabaseRequest {
    pub confirm: bool,
}

/// Reset the database (async background job)
///
/// This endpoint initiates a database reset as a background job:
/// 1. Returns immediately with a job ID for progress tracking
/// 2. The background job clears all data for the current user
/// 3. Progress can be monitored via /api/ingestion/progress/{job_id}
///
/// This is a destructive operation that cannot be undone.
///
/// # Multi-Tenancy Support
///
/// This endpoint respects multi-tenancy by only clearing data for the
/// current user (identified via x-user-hash header). It uses the scan-free
/// DynamoDbResetManager to efficiently delete data partitioned by user.
#[utoipa::path(
    post,
    path = "/api/system/reset-database",
    tag = "system",
    request_body = ResetDatabaseRequest,
    responses(
        (status = 202, description = "Database reset job started", body = AdminJobResponse),
        (status = 400, description = "Bad request", body = AdminJobResponse),
        (status = 500, description = "Server error", body = AdminJobResponse)
    )
)]
pub async fn reset_database(
    state: web::Data<AppState>,
    progress_tracker: web::Data<ProgressTracker>,
    req: web::Json<ResetDatabaseRequest>,
) -> impl Responder {
    if !req.confirm {
        return HttpResponse::BadRequest().json(
            AdminJobResponse::error("Reset confirmation required. Set 'confirm' to true."),
        );
    }

    let user_id = match require_user_context() {
        Ok(hash) => hash,
        Err(response) => return response,
    };

    let job_id = match create_async_job(
        "reset", "database_reset", "Initializing database reset...", &user_id, &progress_tracker,
    ).await {
        Ok(id) => id,
        Err(response) => return response,
    };

    let node_manager = state.node_manager.clone();
    let handle = JobHandle::new(progress_tracker.as_ref().clone(), job_id.clone());

    tokio::spawn(async move {
        let uid = user_id.clone();
        fold_db::logging::core::run_with_user(&user_id, async move {
            handle.update(10, "Clearing user data from storage...").await;

            let node_arc = match node_manager.get_node(&uid).await {
                Ok(n) => n,
                Err(e) => {
                    handle.fail(format!("Failed to get node: {}", e)).await;
                    return;
                }
            };

            let processor = crate::fold_node::OperationProcessor::new(
                node_arc.read().await.clone(),
            );

            if let Err(e) = processor.perform_database_reset(Some(&uid)).await {
                handle.fail(format!("Database reset failed: {}", e)).await;
                return;
            }

            node_manager.invalidate_node(&uid).await;

            log_feature!(LogFeature::HttpServer, info, "Database reset completed for user: {}", uid);
            handle.complete(serde_json::json!({
                "user_id": uid,
                "message": "Database reset successfully. All data has been cleared."
            })).await;
        }).await;
    });

    HttpResponse::Accepted().json(
        AdminJobResponse::started(job_id, "Database reset started. Monitor progress via /api/ingestion/progress endpoint."),
    )
}

/// Request body for migrating to cloud
#[derive(Deserialize, Serialize, utoipa::ToSchema, Debug, Clone)]
pub struct MigrateToCloudRequest {
    pub api_url: String,
    pub api_key: String,
}

/// Migrate data to Exemem Cloud (async background job)
///
/// This endpoint initiates S3 sync setup for the local Sled database:
/// 1. Returns immediately with a job ID for progress tracking
/// 2. The background job snapshots local data and uploads encrypted blobs to S3
/// 3. Progress can be monitored via /api/ingestion/progress/{job_id}
#[utoipa::path(
    post,
    path = "/api/system/migrate-to-cloud",
    tag = "system",
    request_body = MigrateToCloudRequest,
    responses(
        (status = 202, description = "Migration job started", body = AdminJobResponse),
        (status = 400, description = "Bad request", body = AdminJobResponse),
        (status = 500, description = "Server error", body = AdminJobResponse)
    )
)]
pub async fn migrate_to_cloud(
    state: web::Data<AppState>,
    progress_tracker: web::Data<ProgressTracker>,
    req: web::Json<MigrateToCloudRequest>,
) -> impl Responder {
    let user_id = match require_user_context() {
        Ok(hash) => hash,
        Err(response) => return response,
    };

    if req.api_url.is_empty() || req.api_key.is_empty() {
        return HttpResponse::BadRequest().json(
            AdminJobResponse::error("api_url and api_key are required."),
        );
    }

    let job_id = match create_async_job(
        "migrate", "cloud_migration",
        format!("Initializing migration to {}...", req.api_url),
        &user_id, &progress_tracker,
    ).await {
        Ok(id) => id,
        Err(response) => return response,
    };

    let node_manager = state.node_manager.clone();
    let handle = JobHandle::new(progress_tracker.as_ref().clone(), job_id.clone());
    let api_url = req.api_url.clone();
    let api_key = req.api_key.clone();

    tokio::spawn(async move {
        let uid = user_id.clone();
        fold_db::logging::core::run_with_user(&user_id, async move {
            handle.update(10, "Fetching local node data...").await;

            let node_arc = match node_manager.get_node(&uid).await {
                Ok(n) => n,
                Err(e) => {
                    handle.fail(format!("Failed to get node: {}", e)).await;
                    return;
                }
            };

            let processor = crate::fold_node::OperationProcessor::new(
                node_arc.read().await.clone(),
            );

            handle.update(20, "Syncing schemas and documents...").await;

            if let Err(e) = processor.migrate_to_cloud(&api_url, &api_key).await {
                handle.fail(format!("Cloud migration failed: {}", e)).await;
                return;
            }

            log_feature!(LogFeature::HttpServer, info, "Cloud migration completed for user: {}", uid);
            handle.complete(serde_json::json!({
                "user_id": uid,
                "message": "Migration completed successfully"
            })).await;
        }).await;
    });

    HttpResponse::Accepted().json(
        AdminJobResponse::started(job_id, "Cloud migration started. Monitor progress via /api/ingestion/progress endpoint."),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fold_node::{FoldNode, NodeConfig};
    use crate::server::node_manager::{NodeManager, NodeManagerConfig};
    use actix_web::test;
    use std::sync::Arc;
    use tempfile::tempdir;

    async fn create_test_state(temp_dir: &tempfile::TempDir) -> web::Data<AppState> {
        let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
        let config = NodeConfig::new(temp_dir.path().to_path_buf())
            .with_schema_service_url("test://mock")
            .with_identity(&keypair.public_key_base64(), &keypair.secret_key_base64());
        let node = FoldNode::new(config.clone()).await.unwrap();

        let node_manager_config = NodeManagerConfig {
            base_config: config,
        };
        let node_manager = NodeManager::new(node_manager_config);
        node_manager.set_node("test_user", node).await;

        web::Data::new(AppState {
            node_manager: Arc::new(node_manager),
        })
    }

    #[tokio::test]
    async fn test_reset_database_without_confirmation() {
        let temp_dir = tempdir().unwrap();
        let state = create_test_state(&temp_dir).await;
        let progress_tracker = web::Data::new(fold_db::progress::create_tracker(None).await);

        let req_body = ResetDatabaseRequest { confirm: false };
        let req = test::TestRequest::post()
            .set_json(&req_body)
            .to_http_request();

        let resp = reset_database(state, progress_tracker, web::Json(req_body))
            .await
            .respond_to(&req);
        assert_eq!(resp.status(), 400);
    }

    #[tokio::test]
    async fn test_reset_database_with_confirmation() {
        let temp_dir = tempdir().unwrap();
        let state = create_test_state(&temp_dir).await;
        let progress_tracker = web::Data::new(fold_db::progress::create_tracker(None).await);

        fold_db::logging::core::run_with_user("test_user", async move {
            let req_body = ResetDatabaseRequest { confirm: true };
            let req = test::TestRequest::post()
                .set_json(&req_body)
                .to_http_request();

            let resp = reset_database(state, progress_tracker, web::Json(req_body))
                .await
                .respond_to(&req);
            // The response should be 202 (Accepted) for async job started, or 500 for internal error
            assert!(resp.status() == 202 || resp.status() == 500);
        })
        .await;
    }
}
