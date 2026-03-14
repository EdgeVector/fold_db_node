use crate::fold_node::OperationProcessor;
use crate::handlers::{ApiResponse, HandlerError, IntoHandlerError};
use crate::server::http_server::AppState;
use crate::server::routes::{handler_result_to_response, node_or_return};
use actix_web::{web, HttpResponse, Responder};
use futures_util::stream::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_stream::wrappers::BroadcastStream;

const LOG_LEVELS: &[&str] = &["TRACE", "DEBUG", "INFO", "WARN", "ERROR"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogListResponse {
    pub logs: serde_json::Value,
    pub count: usize,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LogConfigResponse {
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogFeaturesResponse {
    pub features: serde_json::Value,
    pub available_levels: Vec<String>,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct LogLevelUpdate {
    pub feature: String,
    pub level: String,
}

#[derive(Deserialize)]
pub struct ListLogsQuery {
    pub since: Option<i64>,
}

/// List logs with optional pagination
#[utoipa::path(
    get,
    path = "/api/logs",
    tag = "logs",
    params(
        ("since" = Option<i64>, Query, description = "Timestamp to list logs from")
    ),
    responses((status = 200, description = "List logs", body = serde_json::Value))
)]
pub async fn list_logs(
    query: web::Query<ListLogsQuery>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(async {
        let logs = op.list_logs(query.since, Some(1000)).await;
        let count = logs.len();
        let logs_json = serde_json::to_value(&logs).handler_err("serialize logs")?;
        Ok(ApiResponse::success_with_user(
            LogListResponse {
                logs: logs_json,
                count,
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            },
            user_hash,
        ))
    }.await)
}

/// Stream logs via Server-Sent Events (backward compatibility)
#[utoipa::path(
    get,
    path = "/api/logs/stream",
    tag = "logs",
    responses((status = 200, description = "Stream logs"))
)]
pub async fn stream_logs() -> impl Responder {
    let rx = match fold_db::logging::subscribe() {
        Some(r) => r,
        None => return HttpResponse::InternalServerError().finish(),
    };

    let stream = BroadcastStream::new(rx).filter_map(|msg| async move {
        match msg {
            Ok(json_str) => Some(Ok::<web::Bytes, actix_web::Error>(web::Bytes::from(
                format!("data: {}\n\n", json_str),
            ))),
            Err(_) => None,
        }
    });

    HttpResponse::Ok()
        .insert_header(("Content-Type", "text/event-stream"))
        .streaming(stream)
}

/// Get current logging configuration
#[utoipa::path(
    get,
    path = "/api/logs/config",
    tag = "logs",
    responses((status = 200, description = "Logging configuration"))
)]
pub async fn get_config(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(async {
        let config = op.get_log_config().await
            .ok_or_else(|| HandlerError::Internal("Log configuration not available".to_string()))?;
        let config_json = serde_json::to_value(config).handler_err("serialize log config")?;
        Ok(ApiResponse::success_with_user(LogConfigResponse { config: config_json }, user_hash))
    }.await)
}

/// Update feature-specific log level at runtime
#[utoipa::path(
    put,
    path = "/api/logs/level",
    tag = "logs",
    request_body = LogLevelUpdate,
    responses(
        (status = 200, description = "Updated"),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Server error")
    )
)]
pub async fn update_feature_level(
    level_update: web::Json<LogLevelUpdate>,
    state: web::Data<AppState>,
) -> impl Responder {
    if !LOG_LEVELS.contains(&level_update.level.as_str()) {
        return HttpResponse::BadRequest().json(json!({
            "error": format!("Invalid log level: {}", level_update.level)
        }));
    }

    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(async {
        op.update_log_feature_level(&level_update.feature, &level_update.level).await
            .map_err(HandlerError::from)?;
        Ok(ApiResponse::success_with_user(
            crate::handlers::response::SuccessResponse {
                success: true,
                message: Some(format!("Updated {} log level to {}", level_update.feature, level_update.level)),
            },
            user_hash,
        ))
    }.await)
}

/// Reload logging configuration from file
#[utoipa::path(
    post,
    path = "/api/logs/config/reload",
    tag = "logs",
    responses((status = 200, description = "Reloaded"), (status = 400, description = "Bad request"))
)]
pub async fn reload_config(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(async {
        op.reload_log_config("config/logging.toml").await.map_err(HandlerError::from)?;
        Ok(ApiResponse::success_with_user(
            crate::handlers::response::SuccessResponse {
                success: true,
                message: Some("Configuration reloaded successfully".to_string()),
            },
            user_hash,
        ))
    }.await)
}

/// Get available log features and their current levels
#[utoipa::path(
    get,
    path = "/api/logs/features",
    tag = "logs",
    responses((status = 200, description = "Features", body = serde_json::Value))
)]
pub async fn get_features(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(async {
        let features = op.get_log_features().await
            .ok_or_else(|| HandlerError::Internal("Log features not available".to_string()))?;
        let features_json = serde_json::to_value(features).handler_err("serialize log features")?;
        Ok(ApiResponse::success_with_user(
            LogFeaturesResponse {
                features: features_json,
                available_levels: LOG_LEVELS.iter().map(|s| s.to_string()).collect(),
            },
            user_hash,
        ))
    }.await)
}
