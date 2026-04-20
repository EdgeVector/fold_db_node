use crate::server::http_server::AppState;
use crate::server::routes::{handler_result_to_response, node_or_return};
use actix_web::{web, HttpResponse, Responder};
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use serde::Serialize;

/// Sync trigger response returned by POST /api/sync/trigger
#[derive(Serialize)]
pub struct SyncTriggerResponse {
    pub success: bool,
    pub message: String,
}

/// Get current sync/backup status.
///
/// Delegates to the shared handler in `handlers::system` so there's one
/// implementation for both `/api/sync/status` and `/api/system/sync-status`.
pub async fn get_sync_status(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(crate::handlers::system::get_sync_status(&user_hash, &node).await)
}

/// Manually trigger a sync cycle
pub async fn trigger_sync(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    let db = match node.get_fold_db() {
        Ok(db) => db,
        Err(e) => {
            log_feature!(
                LogFeature::HttpServer,
                error,
                "Failed to get FoldDB for sync trigger: {}",
                e
            );
            return HttpResponse::InternalServerError().json(SyncTriggerResponse {
                success: false,
                message: format!("Failed to access database: {}", e),
            });
        }
    };

    if !db.is_sync_enabled() {
        return HttpResponse::BadRequest().json(SyncTriggerResponse {
            success: false,
            message: "Sync is not enabled. Configure Exemem cloud storage to enable backup sync."
                .to_string(),
        });
    }

    match db.force_sync().await {
        Ok(()) => {
            log_feature!(
                LogFeature::HttpServer,
                info,
                "Manual sync triggered successfully"
            );
            HttpResponse::Ok().json(SyncTriggerResponse {
                success: true,
                message: "Sync triggered successfully".to_string(),
            })
        }
        Err(e) => {
            log_feature!(
                LogFeature::HttpServer,
                error,
                "Manual sync trigger failed: {}",
                e
            );
            HttpResponse::InternalServerError().json(SyncTriggerResponse {
                success: false,
                message: format!("Sync failed: {}", e),
            })
        }
    }
}

/// Get sync status for a specific org
///
/// GET /api/sync/org/{org_hash}/status
pub async fn get_org_sync_status(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);
    let org_hash = path.into_inner();

    match node.get_org_sync_status(&org_hash).await {
        Ok(Some(status)) => HttpResponse::Ok().json(status),
        Ok(None) => HttpResponse::Ok().json(serde_json::json!({
            "org_hash": org_hash,
            "sync_enabled": false,
            "message": "Sync is not enabled on this node"
        })),
        Err(e) => {
            log_feature!(
                LogFeature::HttpServer,
                error,
                "Failed to get org sync status for {}: {}",
                org_hash,
                e
            );
            HttpResponse::NotFound().json(serde_json::json!({
                "error": format!("{}", e)
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::routes::common::test_helpers::create_test_state;
    use actix_web::test;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_get_sync_status_local_mode() {
        let temp_dir = tempdir().unwrap();
        let state = create_test_state(&temp_dir).await;

        fold_db::logging::core::run_with_user("test_user", async move {
            let req = test::TestRequest::get().to_http_request();
            let resp = get_sync_status(state).await.respond_to(&req);
            assert_eq!(resp.status(), 200);

            let body = resp.into_body();
            let bytes = actix_web::body::to_bytes(body).await.unwrap_or_default();
            let response: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();

            // In local mode, sync should be disabled
            // Response is wrapped in the ApiResponse envelope: {"ok": true, "enabled": false, ...}
            assert_eq!(response["ok"], true);
            assert_eq!(response["enabled"], false);
        })
        .await;
    }

    /// `/api/sync/org/{hash}/status` returns the `sync_enabled: false` envelope
    /// in local-only mode (no sync engine present).
    ///
    /// Regression guard for the PAPERCUT fix that added a cloud-member
    /// reconciliation pass inside `FoldNode::get_org_sync_status` — the
    /// early-exit path when `sync_engine()` is `None` must still be taken
    /// BEFORE any cloud call is attempted.
    #[tokio::test]
    async fn test_get_org_sync_status_local_mode_returns_disabled_envelope() {
        let temp_dir = tempdir().unwrap();
        let state = create_test_state(&temp_dir).await;

        fold_db::logging::core::run_with_user("test_user", async move {
            let req = test::TestRequest::get().to_http_request();
            let path = web::Path::from("deadbeefdeadbeefdeadbeefdeadbeef".to_string());
            let resp = get_org_sync_status(path, state).await.respond_to(&req);
            assert_eq!(resp.status(), 200);

            let body = resp.into_body();
            let bytes = actix_web::body::to_bytes(body).await.unwrap_or_default();
            let response: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();

            assert_eq!(response["sync_enabled"], false);
            assert_eq!(response["org_hash"], "deadbeefdeadbeefdeadbeefdeadbeef");
        })
        .await;
    }

    #[tokio::test]
    async fn test_trigger_sync_local_mode() {
        let temp_dir = tempdir().unwrap();
        let state = create_test_state(&temp_dir).await;

        fold_db::logging::core::run_with_user("test_user", async move {
            let req = test::TestRequest::post().to_http_request();
            let resp = trigger_sync(state).await.respond_to(&req);
            // Should return 400 since sync is not enabled in local mode
            assert_eq!(resp.status(), 400);

            let body = resp.into_body();
            let bytes = actix_web::body::to_bytes(body).await.unwrap_or_default();
            let response: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();

            assert_eq!(response["success"], false);
        })
        .await;
    }
}
