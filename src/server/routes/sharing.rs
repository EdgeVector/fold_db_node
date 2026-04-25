//! HTTP routes for cross-user sharing.

use crate::handlers::sharing as sharing_handlers;
use crate::server::http_server::AppState;
use crate::server::routes::discovery::{
    get_auth_token, get_discovery_config, is_auth_error, try_refresh_token,
};
use crate::server::routes::{
    handler_error_to_response, handler_result_to_response, node_or_return,
};
use actix_web::{web, HttpRequest, HttpResponse, Responder};

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

/// POST /api/sharing/invite — generate a share invite.
///
/// If discovery is configured on the node, the invite is also pushed to the
/// recipient's messaging pseudonym via the encrypted bulletin board so their
/// inbound poller picks it up automatically. If discovery is not configured
/// (pure local mode), the endpoint falls back to returning the invite for
/// manual out-of-band delivery.
pub async fn generate_invite(
    req: HttpRequest,
    body: web::Json<sharing_handlers::GenerateInviteRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);

    // Try the bulletin-board-delivery path. Fall back to plain generate if
    // discovery is not configured or the caller lacks an auth token.
    let disc = get_discovery_config(&state).await;
    let tok = get_auth_token(&req);

    if let (Ok((url, key)), Ok(auth_token)) = (disc, tok) {
        match sharing_handlers::generate_and_send_invite(
            &body,
            &user_hash,
            &node,
            &url,
            &auth_token,
            &key,
        )
        .await
        {
            Ok(response) => return HttpResponse::Ok().json(response),
            Err(e) if is_auth_error(&e) => {
                if let Some(new_token) = try_refresh_token(&state).await {
                    return handler_result_to_response(
                        sharing_handlers::generate_and_send_invite(
                            &body, &user_hash, &node, &url, &new_token, &key,
                        )
                        .await,
                    );
                }
                return handler_error_to_response(e);
            }
            Err(e) => return handler_error_to_response(e),
        }
    }

    // Fallback: discovery not available — just return the invite.
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

/// GET /api/sharing/pending-invites — list invites received via the
/// bulletin board that are awaiting user acceptance.
pub async fn list_pending_invites(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(sharing_handlers::list_pending_invites(&user_hash, &node).await)
}
