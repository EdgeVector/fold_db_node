//! HTTP routes for cross-user sharing.

use crate::handlers::sharing as sharing_handlers;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_result_to_response, node_or_return};
use actix_web::{web, Responder};

/// POST /api/sharing/rules — create a new share rule
pub async fn create_rule(
    body: web::Json<sharing_handlers::CreateRuleRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(sharing_handlers::create_rule(&body, &user_hash, &node).await)
}

/// GET /api/sharing/rules — list all active share rules
pub async fn list_rules(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(sharing_handlers::list_rules(&user_hash, &node).await)
}

/// DELETE /api/sharing/rules/{rule_id} — deactivate a share rule
pub async fn deactivate_rule(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let rule_id = path.into_inner();
    handler_result_to_response(sharing_handlers::deactivate_rule(&rule_id, &user_hash, &node).await)
}

/// POST /api/sharing/invite — generate a share invite
pub async fn generate_invite(
    body: web::Json<sharing_handlers::GenerateInviteRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(sharing_handlers::generate_invite(&body, &user_hash, &node).await)
}

/// POST /api/sharing/accept — accept a share invite
pub async fn accept_invite(
    body: web::Json<sharing_handlers::AcceptInviteRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(sharing_handlers::accept_invite(&body, &user_hash, &node).await)
}
