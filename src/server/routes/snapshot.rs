//! HTTP routes for on-demand snapshot backup and restore.
//!
//! - `POST /api/snapshot/backup` — upload a sealed snapshot of the current
//!   local store to `{user_hash}/snapshots/latest.enc` (plus `{seq}.enc`).
//! - `POST /api/snapshot/restore` — download the most recent snapshot from
//!   the cloud and replay it into the local store (calls `bootstrap_all` so
//!   personal + any configured org targets are both restored).
//!
//! Both endpoints require cloud sync to be enabled. For first-run bootstrap
//! on a fresh device, the existing `handlers::auth::bootstrap_from_cloud`
//! flow runs automatically after `folddb restore --phrase`.

use crate::server::http_server::AppState;
use crate::server::routes::{handler_result_to_response, node_or_return};
use actix_web::{web, Responder};

pub async fn backup(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(crate::handlers::snapshot::backup(&user_hash, &node).await)
}

pub async fn restore(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(crate::handlers::snapshot::restore(&user_hash, &node).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::routes::common::test_helpers::create_test_state;
    use actix_web::test;
    use tempfile::tempdir;

    #[tokio::test]
    async fn backup_returns_400_in_local_mode() {
        let temp_dir = tempdir().unwrap();
        let state = create_test_state(&temp_dir).await;

        fold_db::user_context::run_with_user("test_user", async move {
            let req = test::TestRequest::post().to_http_request();
            let resp = backup(state).await.respond_to(&req);
            assert_eq!(resp.status(), 400);
        })
        .await;
    }

    #[tokio::test]
    async fn restore_returns_400_in_local_mode() {
        let temp_dir = tempdir().unwrap();
        let state = create_test_state(&temp_dir).await;

        fold_db::user_context::run_with_user("test_user", async move {
            let req = test::TestRequest::post().to_http_request();
            let resp = restore(state).await.respond_to(&req);
            assert_eq!(resp.status(), 400);
        })
        .await;
    }
}
