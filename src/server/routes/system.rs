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
use std::sync::OnceLock;
use std::time::Instant;

// Note: /api/system/sync-status reuses super::sync::get_sync_status (same handler).

/// Server start time. Initialized in `FoldHttpServer::run()`; read by
/// `/api/health` to report uptime. Kept as a process-global `OnceLock` so
/// the liveness probe can answer without depending on any per-request node
/// or user context.
static SERVER_START: OnceLock<Instant> = OnceLock::new();

/// Record server start time. Called exactly once from `FoldHttpServer::run()`.
pub fn mark_server_start() {
    let _ = SERVER_START.set(Instant::now());
}

/// Unauthenticated liveness endpoint. Returns `{ok, version, uptime_s}` with
/// no middleware gating. Use this — not `/api/system/status` — for uptime
/// monitors, load balancers, and `curl`-style probes. `/api/system/status`
/// leaks node state and therefore requires `X-User-Hash`.
#[utoipa::path(
    get,
    path = "/api/health",
    tag = "system",
    responses(
        (status = 200, description = "Server is alive", body = serde_json::Value)
    )
)]
pub async fn health_check() -> impl Responder {
    let uptime_s = SERVER_START
        .get()
        .map(|t| t.elapsed().as_secs())
        .unwrap_or(0);
    // Use FOLDDB_BUILD_VERSION (stamped from GITHUB_REF_NAME / git describe
    // in build.rs) for parity with `folddb --version`. CARGO_PKG_VERSION
    // reads the workspace manifest, which drifts: v0.3.6 tarballs reported
    // CARGO_PKG_VERSION=0.3.1 via this endpoint during brew-install-dogfood-
    // run-3 even though the binary itself correctly reported 0.3.6.
    HttpResponse::Ok().json(json!({
        "ok": true,
        "version": env!("FOLDDB_BUILD_VERSION"),
        "uptime_s": uptime_s,
    }))
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
    use crate::server::middleware::auth::UserContextMiddleware;
    use crate::server::routes::common::test_helpers::create_test_state;
    use actix_web::{test, App};
    use tempfile::tempdir;

    /// `/api/health` must answer 200 with no auth, no `X-User-Hash`, and no
    /// `AppState`. Regression guard for the brew-install dogfood papercut
    /// (2026-04-20) where external monitors had no unauth liveness probe.
    #[actix_web::test]
    async fn health_endpoint_is_unauthenticated() {
        let app = test::init_service(
            App::new()
                .wrap(UserContextMiddleware)
                .route("/api/health", actix_web::web::get().to(health_check)),
        )
        .await;

        let req = test::TestRequest::get().uri("/api/health").to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 200, "health must be 200 without credentials");

        let body: serde_json::Value = test::read_body_json(resp).await;
        assert_eq!(body["ok"], true);
        assert!(body["version"].is_string(), "version must be a string");
        // Must match the compile-time build stamp (FOLDDB_BUILD_VERSION),
        // NOT Cargo.toml — see brew-install-dogfood-run-3 BLOCKER.
        assert_eq!(body["version"], env!("FOLDDB_BUILD_VERSION"));
        assert!(body["uptime_s"].is_u64(), "uptime_s must be a u64");
    }

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
