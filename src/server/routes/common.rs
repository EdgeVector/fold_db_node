//! Common utilities for HTTP routes.

use crate::fold_node::FoldNode;
use crate::server::http_server::AppState;
use actix_web::{http::StatusCode, web, HttpResponse};
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use serde_json::json;
use std::sync::Arc;

/// Convert a `HandlerResult<T>` directly to an `HttpResponse`.
///
/// Eliminates the 3-line `match { Ok => json, Err => error }` boilerplate
/// repeated across route handlers.
pub fn handler_result_to_response<T: serde::Serialize>(
    result: Result<T, crate::handlers::HandlerError>,
) -> HttpResponse {
    match result {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// Convert a HandlerError to an appropriate HTTP response.
///
/// This is the centralized conversion function used by all HTTP routes
/// to convert shared handler errors to HTTP responses.
pub fn handler_error_to_response(e: crate::handlers::HandlerError) -> HttpResponse {
    let status_code = match e.status_code() {
        400 => StatusCode::BAD_REQUEST,
        401 => StatusCode::UNAUTHORIZED,
        404 => StatusCode::NOT_FOUND,
        503 => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    HttpResponse::build(status_code).json(e.to_response())
}

/// Require user context from task-local storage.
/// Returns 401 Unauthorized error if no user context is present.
///
/// This is critical for multi-tenant isolation - all data operations
/// must have a valid user context to ensure proper data partitioning.
pub fn require_user_context() -> Result<String, HttpResponse> {
    fold_db::logging::core::get_current_user_id().ok_or_else(|| {
        HttpResponse::Unauthorized().json(json!({
            "ok": false,
            "error": "Authentication required. Please provide X-User-Hash header.",
            "code": "MISSING_USER_CONTEXT"
        }))
    })
}

/// Get a node for the current user from the NodeManager.
///
/// This is the key function for lazy per-user node initialization.
/// Nodes are created on first request and cached for subsequent requests.
async fn get_node_for_user(
    state: &web::Data<AppState>,
    user_id: &str,
) -> Result<Arc<FoldNode>, HttpResponse> {
    state.node_manager.get_node(user_id).await.map_err(|e| {
        log_feature!(
            LogFeature::HttpServer,
            error,
            "Failed to get node for user {}: {}",
            user_id,
            e
        );
        HttpResponse::InternalServerError().json(json!({
            "ok": false,
            "error": format!("Failed to initialize user context: {}", e),
            "code": "NODE_CREATION_FAILED"
        }))
    })
}

/// Helper macro-like pattern to get node for current user context
pub async fn require_node(
    state: &web::Data<AppState>,
) -> Result<(String, Arc<FoldNode>), HttpResponse> {
    let user_hash = require_user_context()?;
    let node = get_node_for_user(state, &user_hash).await?;
    Ok((user_hash, node))
}

/// Macro that calls `require_node` and returns early on error.
///
/// Replaces the 4-line match boilerplate used in every route handler:
/// ```ignore
/// let (user_hash, node) = match require_node(&state).await {
///     Ok(res) => res,
///     Err(response) => return response,
/// };
/// ```
macro_rules! node_or_return {
    ($state:expr) => {
        match $crate::server::routes::common::require_node(&$state).await {
            Ok(res) => res,
            Err(response) => return response,
        }
    };
}
pub(crate) use node_or_return;

/// Macro that calls `require_user_context` and returns early on error.
///
/// Replaces the 4-line match boilerplate:
/// ```ignore
/// let user_id = match require_user_context() {
///     Ok(hash) => hash,
///     Err(response) => return response,
/// };
/// ```
macro_rules! user_context_or_return {
    () => {
        match $crate::server::routes::common::require_user_context() {
            Ok(hash) => hash,
            Err(response) => return response,
        }
    };
}
pub(crate) use user_context_or_return;

#[cfg(test)]
pub mod test_helpers {
    use crate::fold_node::{FoldNode, NodeConfig};
    use crate::server::http_server::AppState;
    use crate::server::node_manager::{NodeManager, NodeManagerConfig};
    use actix_web::web;
    use std::sync::Arc;

    /// Create a test `AppState` with a pre-populated node for "test_user".
    ///
    /// Shared across route test modules to avoid duplicating the same
    /// 12-line setup in every file that needs an `AppState`.
    pub async fn create_test_state(temp_dir: &tempfile::TempDir) -> web::Data<AppState> {
        let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
        let config = NodeConfig::new(temp_dir.path().to_path_buf())
            .with_schema_service_url("test://mock")
            .with_seed_identity(crate::identity::identity_from_keypair(&keypair));
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
}
