//! Connection request endpoints — connect, list, respond, poll, referral.

use super::{auth_token_or_return, discovery_config_or_return, is_auth_error, try_refresh_token};
use crate::handlers::discovery as discovery_handlers;
use crate::server::http_server::AppState;
use crate::server::routes::{
    handler_error_to_response, handler_result_to_response, node_or_return,
};
use actix_web::{web, HttpRequest, HttpResponse, Responder};

/// POST /api/discovery/connect — Send an E2E encrypted connection request.
pub async fn connect(
    req: HttpRequest,
    body: web::Json<discovery_handlers::ConnectRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    let (url, key) = discovery_config_or_return!(state);

    let auth_token = auth_token_or_return!(req);

    match discovery_handlers::connect(&body, &node, &url, &auth_token, &key).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) if is_auth_error(&e) => {
            if let Some(new_token) = try_refresh_token(&state).await {
                match discovery_handlers::connect(&body, &node, &url, &new_token, &key).await {
                    Ok(response) => return HttpResponse::Ok().json(response),
                    Err(e) => return handler_error_to_response(e),
                }
            }
            handler_error_to_response(e)
        }
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/discovery/connection-requests — Poll, decrypt, and list received connection requests.
pub async fn connection_requests(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    let (url, key) = discovery_config_or_return!(state);

    let auth_token = auth_token_or_return!(req);

    handler_result_to_response(
        discovery_handlers::poll_and_decrypt_requests(&node, &url, &auth_token, &key).await,
    )
}

/// POST /api/discovery/connection-requests/respond — Accept or decline a connection request.
pub async fn respond_to_request(
    req: HttpRequest,
    body: web::Json<discovery_handlers::RespondToRequestPayload>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    let (url, key) = discovery_config_or_return!(state);

    let auth_token = auth_token_or_return!(req);

    handler_result_to_response(
        discovery_handlers::respond_to_request(&body, &node, &url, &auth_token, &key).await,
    )
}

/// POST /api/discovery/connection-requests/check-network — Ask contacts if they know the requester.
pub async fn check_network(
    req: HttpRequest,
    body: web::Json<discovery_handlers::CheckNetworkRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    let (url, key) = discovery_config_or_return!(state);

    let auth_token = auth_token_or_return!(req);

    match discovery_handlers::initiate_referral_query(&body, &node, &url, &auth_token, &key).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) if is_auth_error(&e) => {
            if let Some(new_token) = try_refresh_token(&state).await {
                match discovery_handlers::initiate_referral_query(
                    &body, &node, &url, &new_token, &key,
                )
                .await
                {
                    Ok(response) => return HttpResponse::Ok().json(response),
                    Err(e) => return handler_error_to_response(e),
                }
            }
            handler_error_to_response(e)
        }
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/discovery/sent-requests — List sent connection requests with status.
pub async fn sent_requests(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    handler_result_to_response(discovery_handlers::list_sent_requests(&node).await)
}

/// GET /api/discovery/requests — Legacy: Poll for incoming connection requests.
pub async fn poll_requests(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, _node) = node_or_return!(state);

    let (url, key) = discovery_config_or_return!(state);

    let auth_token = auth_token_or_return!(req);

    handler_result_to_response(discovery_handlers::poll_requests(&url, &auth_token, &key).await)
}
