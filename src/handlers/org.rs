//! Shared Org Handlers
//!
//! Framework-agnostic handlers for organization CRUD operations.
//! These can be called by both HTTP server routes and Lambda handlers.

use crate::fold_node::node::FoldNode;
use crate::handlers::handler_response;
use crate::handlers::response::{ApiResponse, HandlerResult, IntoHandlerError};
use fold_db::org::operations as org_ops;
use fold_db::org::types::{OrgInviteBundle, OrgMemberInfo, OrgMembership};
use fold_db::NodeConfigStore;
use serde::Deserialize;
use std::sync::Arc;

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

/// Derive a short display name from a public key (e.g., "node-abc12345").
fn display_name_from_pubkey(public_key: &str) -> String {
    format!("node-{}", &public_key[..8.min(public_key.len())])
}

/// Get the SledPool from a FoldNode, falling back to a local pool if unavailable.
pub async fn get_sled_pool(
    node: &FoldNode,
) -> Result<std::sync::Arc<fold_db::storage::SledPool>, crate::handlers::HandlerError> {
    let db_guard = node.get_fold_db().handler_err("lock database")?;
    if let Some(pool) = db_guard.sled_pool().cloned() {
        Ok(pool)
    } else {
        let meta_path = node.config.get_storage_path().join("meta_db");
        Ok(std::sync::Arc::new(fold_db::storage::SledPool::new(
            meta_path,
        )))
    }
}

/// Helper to get an AuthClient.
///
/// Reads api_url from Sled config store or DatabaseConfig. Auth credentials
/// (session_token, api_key) come from credentials.json — the single source
/// of truth for per-device secrets.
async fn get_auth_client(node: &FoldNode) -> Option<fold_db::sync::auth::AuthClient> {
    // Load per-device credentials from credentials.json
    let creds = crate::keychain::load_credentials().ok().flatten()?;

    // Build auth: prefer session_token over api_key
    let auth = if !creds.session_token.is_empty() {
        fold_db::sync::auth::SyncAuth::BearerToken(creds.session_token)
    } else if !creds.api_key.is_empty() {
        fold_db::sync::auth::SyncAuth::ApiKey(creds.api_key)
    } else {
        return None;
    };

    // Get api_url: try Sled config store first, then DatabaseConfig
    let api_url = if let Ok(db_guard) = node.get_fold_db() {
        if let Some(pool) = db_guard.sled_pool().cloned() {
            drop(db_guard);
            NodeConfigStore::new(pool)
                .ok()
                .and_then(|store| store.get_cloud_config())
                .map(|cloud| cloud.api_url)
        } else {
            drop(db_guard);
            None
        }
    } else {
        None
    }
    .or_else(|| {
        node.config
            .database
            .cloud_sync
            .as_ref()
            .map(|cs| cs.api_url.clone())
    })?;

    let http = shared_http_client();
    Some(fold_db::sync::auth::AuthClient::new(http, api_url, auth))
}

/// Require Exemem cloud configuration, returning the AuthClient or a BadRequest error.
pub async fn require_exemem(
    node: &FoldNode,
) -> Result<fold_db::sync::auth::AuthClient, crate::handlers::HandlerError> {
    get_auth_client(node).await.ok_or_else(|| {
        crate::handlers::HandlerError::BadRequest(
            "Organizations require an Exemem account. Configure Exemem cloud sync to create or join orgs.".to_string(),
        )
    })
}

/// Create a new organization. The calling node becomes the admin.
/// Works in both local and Exemem modes. In Exemem mode, the org is also
/// registered with the cloud backend for invite distribution and sync.
pub async fn create_org(
    req: &CreateOrgRequest,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<CreateOrgResponse> {
    let cloud_client = get_auth_client(node).await;
    let pool = get_sled_pool(node).await?;

    let creator_public_key = node.get_node_public_key().to_string();
    let creator_display_name = display_name_from_pubkey(&creator_public_key);

    let membership =
        org_ops::create_org(&pool, &req.name, &creator_public_key, &creator_display_name)
            .handler_err("create org")?;

    // Generate an invite bundle so the creator can share it immediately
    let invite_bundle = org_ops::generate_invite(&pool, &membership.org_hash)
        .handler_err("generate initial invite")?;

    // Register the org on the Exemem backend (if connected)
    if let Some(client) = cloud_client {
        client
            .create_org(&membership.org_hash)
            .await
            .handler_err("sync create_org to cloud")?;
    }

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
/// Works in both local and Exemem modes. In Exemem mode, cloud sync is
/// configured and the invite is accepted on the backend.
pub async fn join_org(
    invite: &OrgInviteBundle,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<JoinOrgResponse> {
    let pool = get_sled_pool(node).await?;

    let my_public_key = node.get_node_public_key().to_string();
    let my_display_name = display_name_from_pubkey(&my_public_key);

    let membership = org_ops::join_org(&pool, invite, &my_public_key, &my_display_name)
        .handler_err("join org")?;

    // Notify cloud FIRST that we accepted (status → active), so the storage
    // backend recognizes us as a member before we try to download org data.
    if let Some(client) = get_auth_client(node).await {
        let org_hash = &membership.org_hash;
        if let Err(e) = client.accept_invite(org_hash).await {
            log::warn!("Failed to sync accept_invite to cloud: {}", e);
        }
        // Delete the invite from S3 inbox
        let file_name = format!("{}.enc", org_hash);
        delete_inbox_file(&client, &file_name).await;
    }

    // Now configure sync and trigger download — cloud already knows we're a member
    node.configure_org_sync_if_needed().await;
    node.trigger_immediate_sync().await;

    Ok(ApiResponse::success_with_user(
        JoinOrgResponse { org: membership },
        user_hash,
    ))
}

/// List all organizations this node belongs to.
pub async fn list_orgs(user_hash: &str, node: &FoldNode) -> HandlerResult<ListOrgsResponse> {
    let pool = get_sled_pool(node).await?;

    let orgs = org_ops::list_orgs(&pool).handler_err("list orgs")?;

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
    let pool = get_sled_pool(node).await?;

    let membership = org_ops::get_org(&pool, org_hash)
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
) -> HandlerResult<AddMemberResponse> {
    let pool = get_sled_pool(node).await?;

    let member = OrgMemberInfo {
        node_public_key: req.node_public_key.clone(),
        display_name: req.display_name.clone(),
        added_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock before UNIX epoch")
            .as_secs(),
        added_by: node.get_node_public_key().to_string(),
    };

    // Validate the public key can be used for encryption BEFORE modifying state.
    // This prevents adding a member locally/cloud but failing to deliver the invite.
    let invite_bundle =
        org_ops::generate_invite(&pool, org_hash).handler_err("generate invite for inbox")?;
    let invite_json = serde_json::to_vec(&invite_bundle).handler_err("serialize invite")?;
    let encrypted_invite =
        fold_db::crypto::inbox::seal_box_base64(&req.node_public_key, &invite_json)
            .handler_err("encrypt invite — is the public key valid base64 Ed25519?")?;

    org_ops::add_member(&pool, org_hash, member).handler_err("add member")?;

    if let Some(client) = get_auth_client(node).await {
        let target_user_hash = crate::utils::crypto::user_hash_from_pubkey(&req.node_public_key);
        client
            .add_member(org_hash, &target_user_hash, "Member")
            .await
            .handler_err("sync add_member to cloud")?;

        // Upload encrypted invite to the target's S3 inbox
        let file_name = format!("{}.enc", org_hash);
        let presigned = client
            .presign_inbox_upload(&target_user_hash, &file_name)
            .await
            .handler_err("presign inbox upload")?;
        let s3_client = fold_db::sync::s3::S3Client::new(shared_http_client());
        s3_client
            .upload(&presigned, encrypted_invite)
            .await
            .handler_err("upload invite to inbox")?;
    }

    // Return the invite bundle so the UI can show it for manual sharing
    // (especially useful in local mode where there's no cloud inbox)
    Ok(ApiResponse::success_with_user(
        AddMemberResponse {
            ok: true,
            invite_bundle,
        },
        user_hash,
    ))
}

/// Remove a member from an organization.
pub async fn remove_member(
    org_hash: &str,
    node_public_key: &str,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<OkResponse> {
    let pool = get_sled_pool(node).await?;

    org_ops::remove_member(&pool, org_hash, node_public_key).handler_err("remove member")?;

    // If we are removing ourselves, purge the org data and schemas locally
    let is_self_removal = node_public_key == node.get_node_public_key();
    if is_self_removal {
        purge_org_locally(org_hash, node, "removal").await?;
    }

    if let Some(client) = get_auth_client(node).await {
        client
            .remove_member(org_hash, node_public_key)
            .await
            .handler_err("sync remove_member to cloud")?;
    }

    Ok(ApiResponse::success_with_user(
        OkResponse { ok: true },
        user_hash,
    ))
}

/// Leave an organization (remove self).
pub async fn leave_org(
    org_hash: &str,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<OkResponse> {
    let node_public_key = node.get_node_public_key();
    remove_member(org_hash, node_public_key, user_hash, node).await
}

/// Generate an invite bundle for an organization.
pub async fn generate_invite(
    org_hash: &str,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<GenerateInviteResponse> {
    let pool = get_sled_pool(node).await?;

    let invite_bundle = org_ops::generate_invite(&pool, org_hash).handler_err("generate invite")?;

    Ok(ApiResponse::success_with_user(
        GenerateInviteResponse { invite_bundle },
        user_hash,
    ))
}

handler_response! {
    /// Response for adding a member
    pub struct AddMemberResponse {
        pub ok: bool,
        pub invite_bundle: OrgInviteBundle,
    }
}

handler_response! {
    /// Response for removing a member or deleting an org
    pub struct OkResponse {
        pub ok: bool,
    }
}

handler_response! {
    /// Response for declining an invite
    pub struct DeclineInviteResponse {
        pub declined: String,
    }
}

handler_response! {
    /// Response for cloud member list
    pub struct CloudMembersResponse {
        pub members: Vec<serde_json::Value>,
    }
}

/// Purge all local data and schemas for an org, then reconfigure sync.
///
/// Used by both `remove_member` (self-removal) and `delete_org`.
async fn purge_org_locally(
    org_hash: &str,
    node: &FoldNode,
    context: &str,
) -> Result<(), crate::handlers::HandlerError> {
    {
        let fold_db = node.get_fold_db().handler_err("get fold_db")?;
        let db_ops = fold_db.get_db_ops();
        db_ops
            .purge_org_data(org_hash)
            .await
            .handler_err(&format!("purge org data after {context}"))?;
        fold_db
            .schema_manager()
            .purge_org_schemas(org_hash)
            .await
            .handler_err(&format!("purge org schemas after {context}"))?;
        // fold_db guard dropped here before configure_org_sync_if_needed
    }
    node.configure_org_sync_if_needed().await;
    Ok(())
}

/// Shared HTTP client for org operations. Uses a single connection pool across all requests.
fn shared_http_client() -> Arc<reqwest::Client> {
    use std::sync::LazyLock;
    static CLIENT: LazyLock<Arc<reqwest::Client>> =
        LazyLock::new(|| Arc::new(reqwest::Client::new()));
    Arc::clone(&CLIENT)
}

/// Delete an encrypted file from the S3 inbox.
async fn delete_inbox_file(client: &fold_db::sync::auth::AuthClient, file_name: &str) {
    if let Ok(presigned) = client.presign_inbox_delete(file_name).await {
        let s3 = fold_db::sync::s3::S3Client::new(shared_http_client());
        if let Err(e) = s3.delete(&presigned).await {
            log::warn!("Failed to delete {} from inbox: {}", file_name, e);
        }
    }
}

/// Fetch the current member list from the Exemem cloud for an org.
/// This returns the authoritative cloud membership (user_hash, role, status)
/// which may include members added after this node joined.
pub async fn get_cloud_members(
    org_hash: &str,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<CloudMembersResponse> {
    let client = require_exemem(node).await?;

    let members = client
        .list_members(org_hash)
        .await
        .handler_err("fetch cloud members")?;

    Ok(ApiResponse::success_with_user(
        CloudMembersResponse { members },
        user_hash,
    ))
}

/// Delete an organization from local storage.
pub async fn delete_org(
    org_hash: &str,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<OkResponse> {
    let pool = get_sled_pool(node).await?;

    org_ops::delete_org(&pool, org_hash).handler_err("delete org")?;

    // Purge local data and schemas since the org is completely gone
    purge_org_locally(org_hash, node, "deletion").await?;

    Ok(ApiResponse::success_with_user(
        OkResponse { ok: true },
        user_hash,
    ))
}
handler_response! {
    /// Response for pending org invites
    pub struct PendingInvitesResponse {
        pub invites: Vec<OrgInviteBundle>,
    }
}

/// Fetch pending org invitations from the S3 inbox.
pub async fn get_pending_invites(
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<PendingInvitesResponse> {
    let mut invites = Vec::new();

    if let Some(client) = get_auth_client(node).await {
        let s3_client = fold_db::sync::s3::S3Client::new(shared_http_client());

        // 1. List objects in inbox/org_invites/
        let objects = client
            .list_objects("inbox/org_invites/")
            .await
            .handler_err("list inbox objects")?;

        for obj in objects {
            if obj.key.ends_with(".enc") {
                let file_name = match obj.key.split('/').next_back() {
                    Some(name) => name,
                    None => {
                        log::error!("Unexpected empty S3 key in inbox listing");
                        continue;
                    }
                };

                // 2. Request download URL
                let presigned = match client.presign_inbox_download(file_name).await {
                    Ok(p) => p,
                    Err(e) => {
                        log::error!("Failed to presign download for invite {}: {}", file_name, e);
                        continue;
                    }
                };
                // 3. Download encrypted blob
                let encrypted_bytes = match s3_client.download(&presigned).await {
                    Ok(Some(bytes)) => bytes,
                    Ok(None) => {
                        log::error!(
                            "Invite {} exists in listing but download returned empty",
                            file_name
                        );
                        continue;
                    }
                    Err(e) => {
                        log::error!("Failed to download invite {}: {}", file_name, e);
                        continue;
                    }
                };
                // 4. Decrypt using node's secret key
                let my_sec = node.get_node_private_key();
                let plaintext =
                    match fold_db::crypto::inbox::open_box_base64(my_sec, &encrypted_bytes) {
                        Ok(p) => p,
                        Err(e) => {
                            log::error!("Failed to decrypt invite {}: {}", file_name, e);
                            continue;
                        }
                    };
                match serde_json::from_slice::<OrgInviteBundle>(&plaintext) {
                    Ok(bundle) => invites.push(bundle),
                    Err(e) => {
                        log::error!("Failed to deserialize invite {}: {}", file_name, e);
                    }
                }
            }
        }
    }

    Ok(ApiResponse::success_with_user(
        PendingInvitesResponse { invites },
        user_hash,
    ))
}
/// Decline an org invitation. Updates DDB status to "declined" and deletes
/// the encrypted invite from the S3 inbox so it doesn't reappear on poll.
pub async fn decline_invite(
    org_hash: &str,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<DeclineInviteResponse> {
    let client = require_exemem(node).await?;

    // Update DDB membership status → declined
    client
        .decline_invite(org_hash)
        .await
        .handler_err("decline invite")?;

    // Delete the invite from S3 inbox
    let file_name = format!("{}.enc", org_hash);
    delete_inbox_file(&client, &file_name).await;

    Ok(ApiResponse::success_with_user(
        DeclineInviteResponse {
            declined: org_hash.to_string(),
        },
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
