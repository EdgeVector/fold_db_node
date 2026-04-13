//! Data-sharing endpoint — send records to a contact via the bulletin board.

use super::{get_auth_token, get_discovery_config, is_auth_error, try_refresh_token};
use crate::handlers::discovery as discovery_handlers;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_error_to_response, node_or_return};
use actix_web::{web, HttpRequest, HttpResponse, Responder};

/// POST /api/discovery/share — Send records to a contact via the encrypted bulletin board.
pub async fn share_data(
    req: HttpRequest,
    body: web::Json<discovery_handlers::DataShareRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    let (url, key) = match get_discovery_config(&state).await {
        Ok(c) => c,
        Err(response) => return response,
    };

    let auth_token = match get_auth_token(&req) {
        Ok(t) => t,
        Err(response) => return response,
    };

    match discovery_handlers::send_data_share(&body, &node, &url, &auth_token, &key).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) if is_auth_error(&e) => {
            if let Some(new_token) = try_refresh_token(&state).await {
                match discovery_handlers::send_data_share(&body, &node, &url, &new_token, &key)
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
