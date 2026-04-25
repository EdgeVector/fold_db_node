//! Face discovery endpoints — list local embeddings, search network by face.

use super::{auth_token_or_return, discovery_config_or_return, is_auth_error, try_refresh_token};
use crate::handlers::discovery as discovery_handlers;
use crate::server::http_server::AppState;
use crate::server::routes::{
    handler_error_to_response, handler_result_to_response, node_or_return,
};
use actix_web::{web, HttpRequest, HttpResponse, Responder};

/// GET /api/discovery/faces/{schema}/{key} — List face embeddings for a record.
pub async fn list_faces(
    path: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    let (schema, key) = path.into_inner();

    handler_result_to_response(discovery_handlers::list_faces(&node, &schema, &key).await)
}

/// POST /api/discovery/face-search — Search discovery network by face embedding.
pub async fn face_search(
    req: HttpRequest,
    body: web::Json<discovery_handlers::FaceSearchRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    let (url, key) = discovery_config_or_return!(state);

    let auth_token = auth_token_or_return!(req);

    match discovery_handlers::face_search(&body, &node, &url, &auth_token, &key).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) if is_auth_error(&e) => {
            if let Some(new_token) = try_refresh_token(&state).await {
                match discovery_handlers::face_search(&body, &node, &url, &new_token, &key).await {
                    Ok(response) => return HttpResponse::Ok().json(response),
                    Err(e) => return handler_error_to_response(e),
                }
            }
            handler_error_to_response(e)
        }
        Err(e) => handler_error_to_response(e),
    }
}
