use crate::server::http_server::AppState;
use crate::server::routes::node_or_return;
use actix_web::{web, HttpResponse, Responder};
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use serde::Serialize;

/// Sync status response returned by GET /api/sync/status
#[derive(Serialize)]
pub struct SyncStatusResponse {
    /// Whether the sync engine is configured and active
    pub enabled: bool,
    /// Current sync state: "idle", "dirty", "syncing", "offline", or null if disabled
    pub state: Option<String>,
    /// Number of pending (unsynced) log entries, or null if disabled
    pub pending_count: Option<usize>,
    /// Whether E2E encryption keys are loaded (always true when sync is enabled)
    pub encryption_active: bool,
}

/// Sync trigger response returned by POST /api/sync/trigger
#[derive(Serialize)]
pub struct SyncTriggerResponse {
    pub success: bool,
    pub message: String,
}

fn sync_state_to_string(state: fold_db::sync::SyncState) -> String {
    match state {
        fold_db::sync::SyncState::Idle => "idle".to_string(),
        fold_db::sync::SyncState::Dirty => "dirty".to_string(),
        fold_db::sync::SyncState::Syncing => "syncing".to_string(),
        fold_db::sync::SyncState::Offline => "offline".to_string(),
    }
}

/// Get current sync/backup status
pub async fn get_sync_status(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    let db = match node.get_fold_db().await {
        Ok(db) => db,
        Err(e) => {
            log_feature!(
                LogFeature::HttpServer,
                error,
                "Failed to get FoldDB for sync status: {}",
                e
            );
            return HttpResponse::InternalServerError().json(SyncStatusResponse {
                enabled: false,
                state: None,
                pending_count: None,
                encryption_active: false,
            });
        }
    };

    let enabled = db.is_sync_enabled();
    let sync_state = db.sync_state().await;
    let pending_count = db.sync_pending_count().await;

    HttpResponse::Ok().json(SyncStatusResponse {
        enabled,
        state: sync_state.map(sync_state_to_string),
        pending_count,
        encryption_active: enabled,
    })
}

/// Manually trigger a sync cycle
pub async fn trigger_sync(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    let db = match node.get_fold_db().await {
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
            assert_eq!(response["enabled"], false);
            assert!(response["state"].is_null());
            assert!(response["pending_count"].is_null());
            assert_eq!(response["encryption_active"], false);
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
