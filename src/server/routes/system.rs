use crate::handlers::system::NodeKeyResponse;
use crate::handlers::{ApiResponse, HandlerError};
use crate::server::http_server::AppState;
use crate::server::routes::{
    handler_error_to_response, handler_result_to_response, node_or_return,
};
use actix_web::{web, HttpResponse, Responder};
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use serde_json::json;

/// Get sync engine status
#[utoipa::path(
    get,
    path = "/api/system/sync-status",
    tag = "system",
    responses(
        (status = 200, description = "Sync engine status", body = serde_json::Value)
    )
)]
pub async fn get_sync_status(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(crate::handlers::system::get_sync_status(&user_hash, &node).await)
}

/// Get system status information
#[utoipa::path(
    get,
    path = "/api/system/status",
    tag = "system",
    responses(
        (status = 200, description = "System status", body = serde_json::Value)
    )
)]
pub async fn get_system_status(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(crate::handlers::system::get_system_status(&user_hash, &node).await)
}

/// Shared helper for key retrieval endpoints.
fn key_response(
    result: Result<ApiResponse<NodeKeyResponse>, HandlerError>,
    key_name: &str,
    log_msg: &str,
) -> HttpResponse {
    match result {
        Ok(response) => {
            log_feature!(LogFeature::HttpServer, info, "{}", log_msg);
            HttpResponse::Ok().json(json!({
                "success": response.data.as_ref().map(|d| d.success).unwrap_or(false),
                key_name: response.data.as_ref().map(|d| &d.key),
                "message": response.data.as_ref().map(|d| &d.message)
            }))
        }
        Err(e) => handler_error_to_response(e),
    }
}

/// Get the node's public key
///
/// This endpoint returns the node's public key for verification purposes.
/// The public key is generated automatically when the node is created.
#[utoipa::path(
    get,
    path = "/api/system/public-key",
    tag = "system",
    responses(
        (status = 200, description = "Node public key", body = serde_json::Value)
    )
)]
pub async fn get_node_public_key(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let result = crate::handlers::system::get_node_public_key(&user_hash, &node).await;
    key_response(
        result,
        "public_key",
        "Node public key retrieved successfully",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::routes::common::test_helpers::create_test_state;
    use actix_web::test;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_system_status() {
        let temp_dir = tempdir().unwrap();
        let state = create_test_state(&temp_dir).await;

        // Need to run with user context since routes now require authentication
        fold_db::logging::core::run_with_user("test_user", async move {
            let req = test::TestRequest::get().to_http_request();
            let resp = get_system_status(state).await.respond_to(&req);
            assert_eq!(resp.status(), 200);
        })
        .await;
    }

    #[tokio::test]
    async fn test_get_node_public_key() {
        let temp_dir = tempdir().unwrap();
        let state = create_test_state(&temp_dir).await;

        fold_db::logging::core::run_with_user("test_user", async move {
            let req = test::TestRequest::get().to_http_request();
            let resp = get_node_public_key(state).await.respond_to(&req);
            assert_eq!(resp.status(), 200);

            // Parse the response to verify it contains the public key
            let body = resp.into_body();
            let bytes = actix_web::body::to_bytes(body).await.unwrap_or_default();
            let response: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();

            assert!(response["success"].as_bool().unwrap_or(false));
            assert!(response["public_key"].as_str().is_some());
            assert!(!response["public_key"].as_str().unwrap_or("").is_empty());
        })
        .await;
    }
}
