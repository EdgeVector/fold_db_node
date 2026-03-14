use crate::server::http_server::AppState;
use crate::server::routes::{handler_result_to_response, node_or_return};
use actix_web::{web, HttpResponse, Responder};
use futures_util::stream::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_stream::wrappers::BroadcastStream; // Keep for backward compatibility

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct LogLevelUpdate {
    pub feature: String,
    pub level: String,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct LogConfigResponse {
    pub message: String,
    pub current_level: String,
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
    handler_result_to_response(crate::handlers::logs::list_logs(query.since, &user_hash, &node).await)
}

/// Stream logs via Server-Sent Events (backward compatibility)
#[utoipa::path(
    get,
    path = "/api/logs/stream",
    tag = "logs",
    responses((status = 200, description = "Stream logs"))
)]
pub async fn stream_logs() -> impl Responder {
    // Subscribe to new WebOutput via logging module
    let rx = match fold_db::logging::subscribe() {
        Some(r) => r,
        None => return HttpResponse::InternalServerError().finish(),
    };

    // The WebOutput now broadcasts JSON strings (LogEntry serialized)
    // We wrap them in SSE format: "data: {JSON}\n\n"
    let stream = BroadcastStream::new(rx).filter_map(|msg| async move {
        match msg {
            Ok(json_str) => Some(Ok::<web::Bytes, actix_web::Error>(web::Bytes::from(
                format!("data: {}\n\n", json_str),
            ))),
            Err(_) => None, // Broadcast error (lagging, etc)
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
    responses((status = 200, description = "Logging configuration", body = LogConfigResponse))
)]
pub async fn get_config(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(crate::handlers::logs::get_log_config(&user_hash, &node).await)
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
    if !crate::handlers::logs::LOG_LEVELS.contains(&level_update.level.as_str()) {
        return HttpResponse::BadRequest().json(json!({
            "error": format!("Invalid log level: {}", level_update.level)
        }));
    }

    let (user_hash, node) = node_or_return!(state);

    handler_result_to_response(
        crate::handlers::logs::update_log_feature_level(
            &level_update.feature,
            &level_update.level,
            &user_hash,
            &node,
        )
        .await,
    )
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
    handler_result_to_response(crate::handlers::logs::reload_log_config("config/logging.toml", &user_hash, &node).await)
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
    handler_result_to_response(crate::handlers::logs::get_log_features(&user_hash, &node).await)
}
