//! Notification handlers, extracted from the discovery module.

use super::get_metadata_store;
use crate::fold_node::node::FoldNode;
use crate::handlers::response::{ApiResponse, HandlerError, HandlerResult};

// === Notification handlers ===

/// List all notifications stored in the metadata store.
pub async fn list_notifications(node: &FoldNode) -> HandlerResult<serde_json::Value> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {e}")))?;
    let store = get_metadata_store(&db);

    let entries = store
        .scan_prefix(b"notification:")
        .await
        .map_err(|e| HandlerError::Internal(format!("Failed to scan notifications: {e}")))?;

    let notifications: Vec<serde_json::Value> = entries
        .iter()
        .filter_map(|(key, value)| {
            let key_str = String::from_utf8_lossy(key);
            let mut notif: serde_json::Value = serde_json::from_slice(value).ok()?;
            notif
                .as_object_mut()?
                .insert("id".to_string(), serde_json::json!(key_str));
            Some(notif)
        })
        .collect();

    Ok(ApiResponse::success(serde_json::json!({
        "notifications": notifications,
        "count": notifications.len(),
    })))
}

/// Return the count of notifications without loading all bodies.
pub async fn notification_count(node: &FoldNode) -> HandlerResult<serde_json::Value> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {e}")))?;
    let store = get_metadata_store(&db);
    let entries = store
        .scan_prefix(b"notification:")
        .await
        .map_err(|e| HandlerError::Internal(format!("Failed to scan notifications: {e}")))?;
    Ok(ApiResponse::success(serde_json::json!({
        "count": entries.len(),
    })))
}

/// Dismiss (delete) a single notification by its ID.
pub async fn dismiss_notification(
    node: &FoldNode,
    notification_id: &str,
) -> HandlerResult<serde_json::Value> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {e}")))?;
    let store = get_metadata_store(&db);

    store
        .delete(notification_id.as_bytes())
        .await
        .map_err(|e| HandlerError::Internal(format!("Failed to dismiss notification: {e}")))?;

    Ok(ApiResponse::success(serde_json::json!({"dismissed": true})))
}
