//! Discovery search endpoints — network search, similar profiles, browse.

use super::{auth_token_or_return, discovery_config_or_return, is_auth_error, try_refresh_token};
use crate::handlers::discovery as discovery_handlers;
use crate::server::http_server::AppState;
use crate::server::routes::{
    handler_error_to_response, handler_result_to_response, node_or_return,
};
use actix_web::{web, HttpRequest, HttpResponse, Responder};

/// POST /api/discovery/search — Search the discovery network.
pub async fn search(
    req: HttpRequest,
    body: web::Json<discovery_handlers::SearchRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    let (url, key) = discovery_config_or_return!(state);

    let auth_token = auth_token_or_return!(req);

    match discovery_handlers::search(&body, &node, &url, &auth_token, &key).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) if is_auth_error(&e) => {
            if let Some(new_token) = try_refresh_token(&state).await {
                match discovery_handlers::search(&body, &node, &url, &new_token, &key).await {
                    Ok(response) => return HttpResponse::Ok().json(response),
                    Err(e) => return handler_error_to_response(e),
                }
            }
            handler_error_to_response(e)
        }
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/discovery/browse/categories — Browse available categories on the network.
/// Retries once with a refreshed token on 401.
pub async fn browse_categories(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, _node) = node_or_return!(state);

    let (url, key) = discovery_config_or_return!(state);

    let auth_token = auth_token_or_return!(req);

    match discovery_handlers::browse_categories(&url, &auth_token, &key).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) if is_auth_error(&e) => {
            // Try refreshing the token and retrying once
            if let Some(new_token) = try_refresh_token(&state).await {
                match discovery_handlers::browse_categories(&url, &new_token, &key).await {
                    Ok(response) => return HttpResponse::Ok().json(response),
                    Err(e) => return handler_error_to_response(e),
                }
            }
            handler_error_to_response(e)
        }
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/discovery/similar-profiles — Find users with similar interest fingerprints.
pub async fn similar_profiles(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    let (url, key) = discovery_config_or_return!(state);

    let auth_token = auth_token_or_return!(req);

    handler_result_to_response(
        discovery_handlers::similar_profiles(&node, &url, &auth_token, &key).await,
    )
}
