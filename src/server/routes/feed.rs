use crate::handlers::feed as feed_handlers;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_error_to_response, node_or_return};
use actix_web::{web, HttpResponse, Responder};

/// POST /api/feed — Get social photo feed from friends.
#[utoipa::path(
    post,
    path = "/api/feed",
    tag = "feed",
    request_body = feed_handlers::FeedRequest,
    responses(
        (status = 200, description = "Feed items sorted by timestamp descending"),
        (status = 400, description = "Bad request"),
        (status = 404, description = "Schema not found"),
        (status = 500, description = "Server error")
    )
)]
pub async fn get_feed(
    request: web::Json<feed_handlers::FeedRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let request_inner = request.into_inner();
    tracing::info!(
            target: "fold_node::http_server",
        "get_feed: schema={}, friends={}",
        request_inner.schema_name.as_deref().unwrap_or("(all)"),
        request_inner.friend_hashes.len()
    );

    let (user_hash, node) = node_or_return!(state);

    match feed_handlers::get_feed(request_inner, &user_hash, &node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => {
            tracing::error!(
            target: "fold_node::http_server", "Feed query failed: {}", e);
            handler_error_to_response(e)
        }
    }
}
