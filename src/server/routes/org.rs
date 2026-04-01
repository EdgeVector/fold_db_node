//! HTTP routes for organization management.

use crate::handlers::org as org_handlers;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_result_to_response, node_or_return};
use actix_web::{web, Responder};
use fold_db::org::types::OrgInviteBundle;

/// POST /api/org — create a new organization
pub async fn create_org(
    body: web::Json<org_handlers::CreateOrgRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(org_handlers::create_org(&body, &user_hash, &node).await)
}

/// POST /api/org/join — join an organization with an invite bundle
pub async fn join_org(
    body: web::Json<OrgInviteBundle>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(org_handlers::join_org(&body, &user_hash, &node).await)
}

/// GET /api/org — list all organizations this node belongs to
pub async fn list_orgs(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(org_handlers::list_orgs(&user_hash, &node).await)
}

/// GET /api/org/{org_hash} — get a single organization
pub async fn get_org(path: web::Path<String>, state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let org_hash = path.into_inner();
    handler_result_to_response(org_handlers::get_org(&org_hash, &user_hash, &node).await)
}

/// DELETE /api/org/{org_hash} — delete an organization
pub async fn delete_org(path: web::Path<String>, state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let org_hash = path.into_inner();
    handler_result_to_response(org_handlers::delete_org(&org_hash, &user_hash, &node).await)
}

/// POST /api/org/{org_hash}/members — add a member to an organization
pub async fn add_member(
    path: web::Path<String>,
    body: web::Json<org_handlers::AddMemberRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let org_hash = path.into_inner();
    handler_result_to_response(org_handlers::add_member(&org_hash, &body, &user_hash, &node).await)
}

/// DELETE /api/org/{org_hash}/members/{node_public_key} — remove a member
pub async fn remove_member(
    path: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let (org_hash, node_public_key) = path.into_inner();
    handler_result_to_response(
        org_handlers::remove_member(&org_hash, &node_public_key, &user_hash, &node).await,
    )
}

/// POST /api/org/{org_hash}/invite — generate an invite bundle
pub async fn generate_invite(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let org_hash = path.into_inner();
    handler_result_to_response(org_handlers::generate_invite(&org_hash, &user_hash, &node).await)
}
