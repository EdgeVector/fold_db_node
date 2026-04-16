//! Sharing Handlers
//!
//! Handlers for cross-user sharing operations: rules, invites, and subscriptions.

use crate::fold_node::node::FoldNode;
use crate::handlers::current_caller_pubkey;
use crate::handlers::response::{ApiResponse, HandlerResult, IntoHandlerError};
use fold_db::sharing::store;
use fold_db::sharing::types::{ShareRule, ShareScope, ShareInvite, ShareSubscription};
use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_secs()
}

pub async fn get_sled_pool(
    node: &FoldNode,
) -> Result<std::sync::Arc<fold_db::storage::SledPool>, crate::handlers::HandlerError> {
    crate::handlers::org::get_sled_pool(node).await
}

crate::handlers::handler_response! {
    pub struct ShareRuleResponse {
        pub rule: ShareRule,
    }
}

crate::handlers::handler_response! {
    pub struct ListShareRulesResponse {
        pub rules: Vec<ShareRule>,
    }
}

crate::handlers::handler_response! {
    pub struct OkResponse {
        pub ok: bool,
    }
}

crate::handlers::handler_response! {
    pub struct ShareInviteResponse {
        pub invite: ShareInvite,
    }
}

crate::handlers::handler_response! {
    pub struct AcceptInviteResponse {
        pub subscription: ShareSubscription,
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateRuleRequest {
    pub recipient_pubkey: String,
    pub recipient_display_name: String,
    pub scope: ShareScope,
}

#[derive(Debug, Deserialize)]
pub struct GenerateInviteRequest {
    pub rule_id: String,
    pub scope_description: String,
}

#[derive(Debug, Deserialize)]
pub struct AcceptInviteRequest {
    pub invite: ShareInvite,
}

pub async fn create_rule(
    req: &CreateRuleRequest,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<ShareRuleResponse> {
    let pool = get_sled_pool(node).await?;
    let my_pubkey = current_caller_pubkey(node);
    
    let mut e2e_bytes = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut e2e_bytes);

    let my_hash = crate::utils::crypto::user_hash_from_pubkey(&my_pubkey);
    let recipient_hash = crate::utils::crypto::user_hash_from_pubkey(&req.recipient_pubkey);
    let share_prefix = format!("share:{}:{}", my_hash, recipient_hash);

    let rule = ShareRule {
        rule_id: uuid::Uuid::new_v4().to_string(),
        recipient_pubkey: req.recipient_pubkey.clone(),
        recipient_display_name: req.recipient_display_name.clone(),
        scope: req.scope.clone(),
        share_prefix,
        share_e2e_secret: e2e_bytes.to_vec(),
        active: true,
        created_at: now_secs(),
        writer_pubkey: my_pubkey.clone(),
        signature: String::new(), // In a fully secure model, node would sign this
    };

    store::create_share_rule(&pool, rule.clone()).handler_err("create share rule")?;

    // Trigger an immediate sync cycle in the background to propagate data
    // to the new recipient immediately (if the engine natively supports
    // retroactively capturing data for new rules, or just to get the push
    // wheels rolling).
    if let Ok(db) = node.get_fold_db() {
        tokio::spawn(async move {
            let _ = db.force_sync().await;
        });
    }

    Ok(ApiResponse::success_with_user(
        ShareRuleResponse { rule },
        user_hash,
    ))
}

pub async fn list_rules(
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<ListShareRulesResponse> {
    let pool = get_sled_pool(node).await?;
    let rules = store::list_share_rules(&pool).handler_err("list share rules")?;

    Ok(ApiResponse::success_with_user(
        ListShareRulesResponse { rules },
        user_hash,
    ))
}

pub async fn deactivate_rule(
    rule_id: &str,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<OkResponse> {
    let pool = get_sled_pool(node).await?;
    store::deactivate_share_rule(&pool, rule_id).handler_err("deactivate share rule")?;

    Ok(ApiResponse::success_with_user(
        OkResponse { ok: true },
        user_hash,
    ))
}

pub async fn generate_invite(
    req: &GenerateInviteRequest,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<ShareInviteResponse> {
    let pool = get_sled_pool(node).await?;
    
    let rules = store::list_share_rules(&pool).handler_err("list rules to generate invite")?;
    let rule = rules.into_iter().find(|r| r.rule_id == req.rule_id)
        .ok_or_else(|| crate::handlers::HandlerError::NotFound(format!("Rule not found: {}", req.rule_id)))?;

    // In a real flow, this could trigger the bulletin board send.
    // For now, we return the structured invite for the client/tests.
    
    let my_pubkey = current_caller_pubkey(node);
    let my_display_name = format!("node-{}", &my_pubkey[..8.min(my_pubkey.len())]);

    let invite = ShareInvite {
        sender_pubkey: my_pubkey,
        sender_display_name: my_display_name,
        share_prefix: rule.share_prefix.clone(),
        share_e2e_secret: rule.share_e2e_secret.clone(), // This should ideally be encrypted to recipient's pubkey
        scope_description: req.scope_description.clone(),
    };

    Ok(ApiResponse::success_with_user(
        ShareInviteResponse { invite },
        user_hash,
    ))
}

pub async fn accept_invite(
    req: &AcceptInviteRequest,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<AcceptInviteResponse> {
    let pool = get_sled_pool(node).await?;

    let sub = ShareSubscription {
        sender_pubkey: req.invite.sender_pubkey.clone(),
        share_prefix: req.invite.share_prefix.clone(),
        share_e2e_secret: req.invite.share_e2e_secret.clone(),
        accepted_at: now_secs(),
        active: true,
    };

    store::create_share_subscription(&pool, sub.clone()).handler_err("create share subscription")?;

    Ok(ApiResponse::success_with_user(
        AcceptInviteResponse { subscription: sub },
        user_hash,
    ))
}
