//! Shared System Handlers
//!
//! Framework-agnostic handlers for system operations.

use crate::fold_node::node::FoldNode;
use crate::fold_node::OperationProcessor;
use crate::handlers::handler_response;
use crate::handlers::response::{ApiResponse, HandlerResult, IntoHandlerError, IntoTypedHandlerError};

handler_response! {
    pub struct SystemStatusResponse {
        pub status: String,
        pub uptime: u64,
        pub version: String,
        /// Schema service URL configured on the backend (None = local/embedded)
        #[serde(skip_serializing_if = "Option::is_none")]
        pub schema_service_url: Option<String>,
    }
}

handler_response! {
    pub struct NodeKeyResponse {
        pub success: bool,
        pub key: String,
        pub message: String,
    }
}

handler_response! {
    pub struct IndexingStatusResponse {
        pub status: serde_json::Value,
    }
}

pub async fn get_system_status(
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<SystemStatusResponse> {
    Ok(ApiResponse::success_with_user(
        SystemStatusResponse {
            status: "running".to_string(),
            uptime: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            schema_service_url: node.schema_service_url(),
        },
        user_hash,
    ))
}

pub async fn get_indexing_status(
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<IndexingStatusResponse> {
    let status = OperationProcessor::new(node.clone())
        .get_indexing_status()
        .await
        .typed_handler_err()?;
    let status_json = serde_json::to_value(&status).handler_err("serialize indexing status")?;
    Ok(ApiResponse::success_with_user(
        IndexingStatusResponse {
            status: status_json,
        },
        user_hash,
    ))
}

handler_response! {
    pub struct SyncStatusResponse {
        /// Whether sync is enabled (cloud mode).
        pub enabled: bool,
        /// Current sync state: "idle", "dirty", "syncing", "offline". Null if disabled.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub state: Option<String>,
        /// Number of pending (unsynced) log entries. Null if disabled.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub pending_count: Option<usize>,
        /// Unix timestamp (seconds) of last successful sync. Null if never synced or disabled.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub last_sync_at: Option<u64>,
        /// Last sync error message. Null if no error or disabled.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub last_error: Option<String>,
    }
}

pub async fn get_sync_status(
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<SyncStatusResponse> {
    let db = node.get_fold_db().await.typed_handler_err()?;
    let enabled = db.is_sync_enabled();
    let response = if enabled {
        let state = db.sync_state().await;
        let pending = db.sync_pending_count().await;
        let state_str = state.map(|s| match s {
            fold_db::sync::SyncState::Idle => "idle",
            fold_db::sync::SyncState::Dirty => "dirty",
            fold_db::sync::SyncState::Syncing => "syncing",
            fold_db::sync::SyncState::Offline => "offline",
        });
        SyncStatusResponse {
            enabled: true,
            state: state_str.map(String::from),
            pending_count: pending,
            last_sync_at: None,
            last_error: None,
        }
    } else {
        SyncStatusResponse {
            enabled: false,
            state: None,
            pending_count: None,
            last_sync_at: None,
            last_error: None,
        }
    };
    Ok(ApiResponse::success_with_user(response, user_hash))
}

pub async fn get_node_public_key(
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<NodeKeyResponse> {
    Ok(ApiResponse::success_with_user(
        NodeKeyResponse {
            success: true,
            key: node.get_node_public_key().to_string(),
            message: "Node public key retrieved successfully".to_string(),
        },
        user_hash,
    ))
}
