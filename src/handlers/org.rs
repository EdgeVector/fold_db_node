//! Shared Org Handlers
//!
//! Framework-agnostic handlers for organization CRUD operations.
//! These can be called by both HTTP server routes and Lambda handlers.

use crate::fold_node::node::FoldNode;
use crate::handlers::handler_response;
use crate::handlers::response::{ApiResponse, HandlerResult, IntoHandlerError};
use fold_db::org::operations as org_ops;
use fold_db::org::types::{OrgInviteBundle, OrgMemberInfo, OrgMembership};
use serde::Deserialize;

handler_response! {
    /// Response for org creation (returns membership + invite bundle)
    pub struct CreateOrgResponse {
        pub org: OrgMembership,
        pub invite_bundle: OrgInviteBundle,
    }
}

handler_response! {
    /// Response for joining an org
    pub struct JoinOrgResponse {
        pub org: OrgMembership,
    }
}

handler_response! {
    /// Response for listing orgs
    pub struct ListOrgsResponse {
        pub orgs: Vec<OrgMembership>,
    }
}

handler_response! {
    /// Response for getting a single org
    pub struct GetOrgResponse {
        pub org: OrgMembership,
    }
}

handler_response! {
    /// Response for invite generation
    pub struct GenerateInviteResponse {
        pub invite_bundle: OrgInviteBundle,
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateOrgRequest {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct AddMemberRequest {
    pub node_public_key: String,
    pub display_name: String,
}

/// Get the sled::Db from a FoldNode, returning a handler error if unavailable.
async fn get_sled_db(node: &FoldNode) -> Result<sled::Db, crate::handlers::HandlerError> {
    let db_guard = node.get_fold_db().await.handler_err("lock database")?;
    db_guard.sled_db().cloned().ok_or_else(|| {
        crate::handlers::HandlerError::ServiceUnavailable(
            "Org operations require a Sled backend".to_string(),
        )
    })
}

/// Create a new organization. The calling node becomes the admin.
pub async fn create_org(
    req: &CreateOrgRequest,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<CreateOrgResponse> {
    let sled_db = get_sled_db(node).await?;

    let creator_public_key = node.get_node_public_key().to_string();
    // Use a short display name derived from the public key
    let creator_display_name = format!(
        "node-{}",
        &creator_public_key[..8.min(creator_public_key.len())]
    );

    let membership = org_ops::create_org(
        &sled_db,
        &req.name,
        &creator_public_key,
        &creator_display_name,
    )
    .handler_err("create org")?;

    // Generate an invite bundle so the creator can share it immediately
    let invite_bundle = org_ops::generate_invite(&sled_db, &membership.org_hash)
        .handler_err("generate initial invite")?;

    // Reconfigure org sync with the new org
    node.configure_org_sync_if_needed().await;

    Ok(ApiResponse::success_with_user(
        CreateOrgResponse {
            org: membership,
            invite_bundle,
        },
        user_hash,
    ))
}

/// Join an existing organization using an invite bundle.
pub async fn join_org(
    invite: &OrgInviteBundle,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<JoinOrgResponse> {
    let sled_db = get_sled_db(node).await?;

    let my_public_key = node.get_node_public_key().to_string();
    let my_display_name = format!("node-{}", &my_public_key[..8.min(my_public_key.len())]);

    let membership = org_ops::join_org(&sled_db, invite, &my_public_key, &my_display_name)
        .handler_err("join org")?;

    // Reconfigure org sync with the joined org
    node.configure_org_sync_if_needed().await;

    Ok(ApiResponse::success_with_user(
        JoinOrgResponse { org: membership },
        user_hash,
    ))
}

/// List all organizations this node belongs to.
pub async fn list_orgs(user_hash: &str, node: &FoldNode) -> HandlerResult<ListOrgsResponse> {
    let sled_db = get_sled_db(node).await?;

    let orgs = org_ops::list_orgs(&sled_db).handler_err("list orgs")?;

    Ok(ApiResponse::success_with_user(
        ListOrgsResponse { orgs },
        user_hash,
    ))
}

/// Get a single organization by its hash.
pub async fn get_org(
    org_hash: &str,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<GetOrgResponse> {
    let sled_db = get_sled_db(node).await?;

    let membership = org_ops::get_org(&sled_db, org_hash)
        .handler_err("get org")?
        .ok_or_else(|| {
            crate::handlers::HandlerError::NotFound(format!(
                "Organization '{}' not found",
                org_hash
            ))
        })?;

    Ok(ApiResponse::success_with_user(
        GetOrgResponse { org: membership },
        user_hash,
    ))
}

/// Add a member to an organization.
pub async fn add_member(
    org_hash: &str,
    req: &AddMemberRequest,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<serde_json::Value> {
    let sled_db = get_sled_db(node).await?;

    let member = OrgMemberInfo {
        node_public_key: req.node_public_key.clone(),
        display_name: req.display_name.clone(),
        added_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock before UNIX epoch")
            .as_secs(),
        added_by: node.get_node_public_key().to_string(),
    };

    org_ops::add_member(&sled_db, org_hash, member).handler_err("add member")?;

    Ok(ApiResponse::success_with_user(
        serde_json::json!({"ok": true}),
        user_hash,
    ))
}

/// Remove a member from an organization.
pub async fn remove_member(
    org_hash: &str,
    node_public_key: &str,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<serde_json::Value> {
    let sled_db = get_sled_db(node).await?;

    org_ops::remove_member(&sled_db, org_hash, node_public_key).handler_err("remove member")?;

    Ok(ApiResponse::success_with_user(
        serde_json::json!({"ok": true}),
        user_hash,
    ))
}

/// Generate an invite bundle for an organization.
pub async fn generate_invite(
    org_hash: &str,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<GenerateInviteResponse> {
    let sled_db = get_sled_db(node).await?;

    let invite_bundle =
        org_ops::generate_invite(&sled_db, org_hash).handler_err("generate invite")?;

    Ok(ApiResponse::success_with_user(
        GenerateInviteResponse { invite_bundle },
        user_hash,
    ))
}

/// Delete an organization from local storage.
pub async fn delete_org(
    org_hash: &str,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<serde_json::Value> {
    let sled_db = get_sled_db(node).await?;

    org_ops::delete_org(&sled_db, org_hash).handler_err("delete org")?;

    // Reconfigure org sync without the deleted org
    node.configure_org_sync_if_needed().await;

    Ok(ApiResponse::success_with_user(
        serde_json::json!({"ok": true}),
        user_hash,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_org_request_deserialize() {
        let json = r#"{"name": "Test Org"}"#;
        let req: CreateOrgRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "Test Org");
    }

    #[test]
    fn test_add_member_request_deserialize() {
        let json = r#"{"node_public_key": "abc123", "display_name": "Alice"}"#;
        let req: AddMemberRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.node_public_key, "abc123");
        assert_eq!(req.display_name, "Alice");
    }
}
