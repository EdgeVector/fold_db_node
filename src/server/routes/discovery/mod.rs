//! Discovery route handlers, split by domain.
//!
//! This module is a thin HTTP-extraction wrapper over `handlers::discovery`.
//! Sub-modules group endpoints by concern (config/opt-ins, publish, search,
//! connections, interests, calendar, moments, faces, share, notifications).
//!
//! Shared helpers (discovery config resolution, auth token extraction,
//! auth-error detection, token refresh) live in this file because multiple
//! sub-modules need them.

use crate::server::http_server::AppState;
use actix_web::{web, HttpRequest, HttpResponse};

pub mod calendar;
pub mod config;
pub mod connections;
pub mod faces;
pub mod interests;
pub mod moments;
pub mod notifications;
pub mod publish;
pub mod search;
pub mod share;

// Re-export every route function so external callers (http_server.rs) can
// continue to reference `routes::discovery::<fn>` without changes.
pub use calendar::{
    calendar_sharing_opt_in, calendar_sharing_opt_out, calendar_sharing_status, get_shared_events,
    store_peer_events, sync_calendar_events,
};
pub use config::{list_opt_ins, my_pseudonyms, opt_in, opt_out, opt_out_all};
pub use connections::{
    check_network, connect, connection_requests, poll_requests, respond_to_request, sent_requests,
};
pub use faces::{face_search, list_faces};
pub use interests::{detect_interests, get_interests, toggle_interest};
pub use moments::{
    moment_detect, moment_list, moment_opt_in, moment_opt_in_list, moment_opt_out,
    moment_receive_hashes, moment_scan,
};
pub use notifications::{dismiss_notification, list_notifications, notification_count};
pub use publish::publish;
pub use search::{browse_categories, search, similar_profiles};
pub use share::share_data;

/// Helper to get discovery config via the explicit `AppState` resolver.
/// Returns (discovery_url, master_key) or a 503 response when the node has
/// not been registered with Exemem yet.
pub(crate) async fn get_discovery_config(
    state: &AppState,
) -> Result<(String, Vec<u8>), HttpResponse> {
    match state.discovery_config().await {
        Some(cfg) => Ok((cfg.url, cfg.master_key)),
        None => Err(HttpResponse::ServiceUnavailable().json(serde_json::json!({
            "ok": false,
            "error": "Discovery not available. Register with Exemem to enable.",
            "code": "DISCOVERY_NOT_CONFIGURED"
        }))),
    }
}

/// Macro that calls `get_discovery_config` and returns early on error.
///
/// Replaces the 4-line match boilerplate:
/// ```ignore
/// let (url, key) = match get_discovery_config(&state).await {
///     Ok(c) => c,
///     Err(response) => return response,
/// };
/// ```
macro_rules! discovery_config_or_return {
    ($state:expr) => {
        match $crate::server::routes::discovery::get_discovery_config(&$state).await {
            Ok(c) => c,
            Err(response) => return response,
        }
    };
}
pub(crate) use discovery_config_or_return;

/// Macro that calls `get_auth_token` and returns early on error.
///
/// Replaces the 4-line match boilerplate:
/// ```ignore
/// let auth_token = match get_auth_token(&req) {
///     Ok(t) => t,
///     Err(response) => return response,
/// };
/// ```
macro_rules! auth_token_or_return {
    ($req:expr) => {
        match $crate::server::routes::discovery::get_auth_token(&$req) {
            Ok(t) => t,
            Err(response) => return response,
        }
    };
}
pub(crate) use auth_token_or_return;

/// Extract the auth token from env var, local credential store, or the incoming request's
/// Authorization header. Env var is checked first to avoid unnecessary file reads
/// in dev/CLI mode.
pub(crate) fn get_auth_token(req: &HttpRequest) -> Result<String, HttpResponse> {
    // 1. Env var (no credential file read needed — preferred for dev/CLI)
    if let Ok(token) = std::env::var("DISCOVERY_AUTH_TOKEN") {
        return Ok(token);
    }

    // 2. Local credentials (where register/refresh store the token in desktop app)
    if let Ok(Some(creds)) = crate::keychain::load_credentials() {
        if !creds.session_token.is_empty() {
            return Ok(creds.session_token);
        }
    }

    // 3. Fall back to incoming request's Authorization header
    let auth = req
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.to_string());

    auth.ok_or_else(|| {
        HttpResponse::Unauthorized().json(serde_json::json!({
            "ok": false,
            "error": "No auth token available. Register with Exemem first (POST /api/auth/register).",
            "code": "AUTH_REQUIRED"
        }))
    })
}

/// Check if a discovery handler error is an auth failure.
/// Matches the HandlerError variant directly instead of fragile string matching.
pub(crate) fn is_auth_error(err: &crate::handlers::HandlerError) -> bool {
    matches!(err, crate::handlers::HandlerError::Unauthorized(_)) || err.to_string().contains("401")
}

/// Try to refresh the session token via signed register, then return the new token.
/// Returns None if refresh is not possible (no node, no exemem API, etc.).
pub(crate) async fn try_refresh_token(state: &web::Data<AppState>) -> Option<String> {
    match crate::server::routes::auth::refresh_session_token(state).await {
        Ok(token) => {
            tracing::info!("Discovery auth: refreshed session token after 401");
            Some(token)
        }
        Err(e) => {
            tracing::warn!("Discovery auth: token refresh failed: {}", e);
            None
        }
    }
}

/// Resolve discovery config for use in other route modules.
/// Returns (discovery_url, master_key, auth_token) or a HandlerError.
pub async fn resolve_discovery_config(
    state: &AppState,
    node: &crate::fold_node::node::FoldNode,
    req: Option<&HttpRequest>,
) -> Result<(String, Vec<u8>, String), crate::handlers::HandlerError> {
    let (url, key) = match state.discovery_config().await {
        Some(cfg) => (cfg.url, cfg.master_key),
        None => {
            let msg =
                "Discovery not configured. Register with Exemem to enable (folddb cloud enable).";
            return Err(crate::handlers::HandlerError::ServiceUnavailable(
                msg.to_string(),
            ));
        }
    };

    // Try to get auth token from env, keychain, or dummy for the request
    let token = if let Some(r) = req {
        get_auth_token(r).map_err(|_| {
            crate::handlers::HandlerError::Unauthorized("No auth token available".to_string())
        })?
    } else {
        // Without a request, try env var and keychain
        std::env::var("DISCOVERY_AUTH_TOKEN")
            .ok()
            .or_else(|| {
                crate::keychain::load_credentials()
                    .ok()
                    .flatten()
                    .filter(|c| !c.session_token.is_empty())
                    .map(|c| c.session_token)
            })
            .ok_or_else(|| {
                crate::handlers::HandlerError::Unauthorized("No auth token available".to_string())
            })?
    };

    let _ = node; // may be used in future for node-specific config
    Ok((url, key, token))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::HandlerError;

    #[test]
    fn is_auth_error_matches_unauthorized_variant() {
        let err = HandlerError::Unauthorized("token expired".to_string());
        assert!(is_auth_error(&err));
    }

    #[test]
    fn is_auth_error_matches_401_in_internal_error() {
        let err = HandlerError::Internal(
            "Failed to search discovery network: Discovery search failed with status 401: Unauthorized".to_string(),
        );
        assert!(is_auth_error(&err));
    }

    #[test]
    fn is_auth_error_does_not_match_other_errors() {
        let err = HandlerError::Internal("network timeout".to_string());
        assert!(!is_auth_error(&err));
    }

    #[test]
    fn is_auth_error_does_not_match_not_found() {
        let err = HandlerError::NotFound("schema not found".to_string());
        assert!(!is_auth_error(&err));
    }
}
