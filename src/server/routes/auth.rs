//! HTTP routes for authentication, registration, and identity restore.
//!
//! These are thin wrappers over `handlers::auth` — all business logic
//! (signed registration, credential refresh, BIP39 restore, bootstrap from
//! cloud) lives in the handler layer. These wrappers only extract data from
//! the HTTP request and convert handler results into `HttpResponse`s.

use actix_web::{web, HttpResponse};

use crate::handlers::auth as handlers_auth;
use crate::server::http_server::AppState;
use crate::server::routes::common::handler_error_to_response;

// Re-exports so existing `routes::auth::*` call sites keep compiling and so
// the binary/public API surface is unchanged.
pub use crate::handlers::auth::{
    build_auth_refresh_callback, check_bootstrap_pending, resume_bootstrap, write_bootstrap_marker,
    BootstrapPhase, BootstrapStatus, BootstrapStatusState, StoreCredentialsRequest,
};

// ============================================================================
// Credential routes — local credential management
// ============================================================================

/// GET /api/auth/credentials
/// Check if credentials exist locally.
pub async fn get_credentials() -> HttpResponse {
    match handlers_auth::get_credentials_response() {
        Ok(json) => HttpResponse::Ok().json(json),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/auth/credentials
/// Store credentials locally (called after verify).
pub async fn store_credentials(body: web::Json<StoreCredentialsRequest>) -> HttpResponse {
    match handlers_auth::store_credentials(body.into_inner()) {
        Ok(json) => HttpResponse::Ok().json(json),
        Err(e) => handler_error_to_response(e),
    }
}

/// DELETE /api/auth/credentials
/// Delete credentials from local storage (logout).
pub async fn delete_credentials() -> HttpResponse {
    match handlers_auth::delete_credentials() {
        Ok(json) => HttpResponse::Ok().json(json),
        Err(e) => handler_error_to_response(e),
    }
}

// ============================================================================
// Exemem config & registration
// ============================================================================

/// GET /api/auth/exemem-config
/// Return the Exemem API URL so the frontend doesn't need to hardcode it.
pub async fn get_exemem_config() -> HttpResponse {
    HttpResponse::Ok().json(handlers_auth::get_exemem_config())
}

/// POST /api/auth/register
/// Register this node's public key with Exemem to create a cloud account.
/// Signs the request with the node's Ed25519 private key to prove key ownership.
/// Idempotent: if already registered, returns a fresh session token.
/// Accepts optional JSON body with `invite_code` for new registrations.
pub async fn register_with_exemem(
    data: web::Data<AppState>,
    body: web::Json<serde_json::Value>,
) -> HttpResponse {
    let invite_code = body
        .get("invite_code")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    match handlers_auth::register_with_exemem(&data.node_manager, invite_code.as_deref()).await {
        Ok(json) => HttpResponse::Ok().json(json),
        Err(e) => HttpResponse::InternalServerError().json(serde_json::json!({
            "ok": false,
            "error": e
        })),
    }
}

/// Refresh the session token by calling the signed register endpoint.
///
/// Thin wrapper retained so existing `routes::auth::refresh_session_token`
/// call sites (http_server.rs, discovery routes) keep working.
pub async fn refresh_session_token(data: &web::Data<AppState>) -> Result<String, String> {
    handlers_auth::refresh_session_token(&data.node_manager).await
}

// ============================================================================
// Recovery phrase (BIP39 mnemonic for device transfer)
// ============================================================================

/// GET /api/auth/recovery-phrase
/// Returns the node's Ed25519 private key as 24 BIP39 mnemonic words.
/// Local-only endpoint — the key never leaves the device over the network.
pub async fn get_recovery_phrase(data: web::Data<AppState>) -> HttpResponse {
    match handlers_auth::get_recovery_phrase(&data.node_manager).await {
        Ok(words) => HttpResponse::Ok().json(serde_json::json!({
            "ok": true,
            "words": words,
        })),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/auth/restore
/// Restore node identity from a 24-word BIP39 recovery phrase.
///
/// Accepts `words` as either a space-separated string or a JSON array of
/// strings — the array shape matches what `GET /api/auth/recovery-phrase`
/// returns, so a user can round-trip the response verbatim.
pub async fn restore_from_phrase(
    data: web::Data<AppState>,
    body: web::Json<serde_json::Value>,
) -> HttpResponse {
    let words = match body.get("words") {
        Some(v) if v.is_string() => v.as_str().unwrap().to_string(),
        Some(v) if v.is_array() => {
            let arr = v.as_array().unwrap();
            let mut parts = Vec::with_capacity(arr.len());
            for item in arr {
                let Some(s) = item.as_str() else {
                    return HttpResponse::BadRequest().json(serde_json::json!({
                        "ok": false,
                        "error": "Expected 'words' array to contain only strings"
                    }));
                };
                parts.push(s);
            }
            parts.join(" ")
        }
        Some(_) => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "ok": false,
                "error": "Expected 'words' as a space-separated string or array of strings"
            }));
        }
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "ok": false,
                "error": "Missing 'words' field"
            }));
        }
    };

    match handlers_auth::restore_from_phrase(
        &data.node_manager,
        handlers_auth::RestoreFromPhraseInput { words },
    )
    .await
    {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/auth/restore/status
/// Returns the state of the most recent restore-triggered cloud bootstrap.
pub async fn restore_status() -> HttpResponse {
    HttpResponse::Ok().json(handlers_auth::restore_status())
}

#[cfg(test)]
mod tests {
    //! Integration tests exercising the route wrappers. Business-logic unit
    //! tests live in `handlers::auth`.
    use super::*;
    use crate::handlers::auth::{bootstrap_status_path, write_bootstrap_status};
    use std::sync::Mutex;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::OnceLock;
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("env lock poisoned")
    }

    fn setup_empty_home() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::env::set_var("FOLDDB_HOME", tmp.path());
        std::fs::create_dir_all(tmp.path().join("data")).expect("create data dir");
        tmp
    }

    async fn test_body_json(resp: HttpResponse) -> serde_json::Value {
        use actix_web::body::MessageBody;
        let body = resp.into_body();
        let bytes = body.try_into_bytes().expect("body bytes");
        serde_json::from_slice(&bytes).expect("json body")
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn restore_status_endpoint_reports_each_state() {
        let _guard = env_lock();
        let _tmp = setup_empty_home();

        // Idle (no files): reports complete.
        let resp = restore_status().await;
        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);
        let body = test_body_json(resp).await;
        assert_eq!(body["status"], "complete");

        // Pending marker present, no status file: reports in_progress.
        let marker = {
            let h = crate::utils::paths::folddb_home().expect("home");
            h.join("data").join(".bootstrap_pending")
        };
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(&marker, "{}").unwrap();
        let resp = restore_status().await;
        let body = test_body_json(resp).await;
        assert_eq!(body["status"], "in_progress");
        std::fs::remove_file(&marker).unwrap();

        // Explicit in_progress status file.
        write_bootstrap_status(&BootstrapStatus::in_progress());
        let body = test_body_json(restore_status().await).await;
        assert_eq!(body["status"], "in_progress");

        // Failed status with error message.
        write_bootstrap_status(&BootstrapStatus::failed("network down".to_string()));
        let body = test_body_json(restore_status().await).await;
        assert_eq!(body["status"], "failed");
        assert_eq!(body["error"], "network down");

        // Complete status.
        write_bootstrap_status(&BootstrapStatus::complete());
        let body = test_body_json(restore_status().await).await;
        assert_eq!(body["status"], "complete");

        // Clean up.
        if let Some(p) = bootstrap_status_path() {
            let _ = std::fs::remove_file(p);
        }
        std::env::remove_var("FOLDDB_HOME");
    }

    // ------------------------------------------------------------------
    // Restore rollback deletes identity file on finalize failure (G3)
    // ------------------------------------------------------------------

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn restore_rollback_removes_identity_on_register_failure() {
        // Point Exemem API at a non-routable address so signed_register fails.
        let _guard = env_lock();
        let tmp = setup_empty_home();
        std::fs::create_dir_all(tmp.path().join("config")).expect("create config dir");
        std::env::set_var("EXEMEM_API_URL", "http://127.0.0.1:1");

        // Build an AppState with a default NodeManager.
        let node_manager = std::sync::Arc::new(crate::server::node_manager::NodeManager::new(
            crate::server::node_manager::NodeManagerConfig {
                base_config: crate::fold_node::config::NodeConfig {
                    database: fold_db::storage::DatabaseConfig::local(tmp.path().join("data")),
                    storage_path: Some(tmp.path().join("data")),
                    network_listen_address: "/ip4/0.0.0.0/tcp/0".to_string(),
                    security_config: fold_db::security::SecurityConfig::from_env(),
                    schema_service_url: Some("test://mock".to_string()),
                    public_key: None,
                    private_key: None,
                    config_dir: Some(tmp.path().join("config")),
                },
            },
        ));

        let data = web::Data::new(AppState {
            node_manager: node_manager.clone(),
        });

        // Valid 24-word phrase. Non-zero entropy because ed25519-compact
        // rejects all-zero seeds when deriving the keypair at node-creation
        // time (triggered now that `ensure_default_identity` eagerly builds
        // the node on the fast path).
        let entropy = [0x42u8; 32];
        let mnemonic = bip39::Mnemonic::from_entropy(&entropy).expect("mnemonic");
        let words = mnemonic.words().collect::<Vec<_>>().join(" ");

        let resp =
            restore_from_phrase(data, web::Json(serde_json::json!({ "words": words }))).await;
        assert_eq!(
            resp.status(),
            actix_web::http::StatusCode::INTERNAL_SERVER_ERROR,
            "restore should fail when Exemem API is unreachable"
        );

        // Identity file must be gone after rollback.
        let id_path = tmp.path().join("config").join("node_identity.json");
        assert!(
            !id_path.exists(),
            "rollback should have deleted {:?}",
            id_path
        );

        std::env::remove_var("EXEMEM_API_URL");
        std::env::remove_var("FOLDDB_HOME");
    }

    // ------------------------------------------------------------------
    // Accept `words` as array (matches GET /api/auth/recovery-phrase shape)
    // ------------------------------------------------------------------

    fn test_app_state(tmp: &tempfile::TempDir) -> web::Data<AppState> {
        let node_manager = std::sync::Arc::new(crate::server::node_manager::NodeManager::new(
            crate::server::node_manager::NodeManagerConfig {
                base_config: crate::fold_node::config::NodeConfig {
                    database: fold_db::storage::DatabaseConfig::local(tmp.path().join("data")),
                    storage_path: Some(tmp.path().join("data")),
                    network_listen_address: "/ip4/0.0.0.0/tcp/0".to_string(),
                    security_config: fold_db::security::SecurityConfig::from_env(),
                    schema_service_url: Some("test://mock".to_string()),
                    public_key: None,
                    private_key: None,
                    config_dir: Some(tmp.path().join("config")),
                },
            },
        ));
        web::Data::new(AppState { node_manager })
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn restore_accepts_words_as_array() {
        // Round-trip the exact shape `GET /api/auth/recovery-phrase` emits:
        // `{"words": [...24 strings...]}`. Should proceed past input parsing
        // and attempt the restore (we assert it's NOT the "Missing 'words'"
        // BadRequest from the input-validation branch).
        let _guard = env_lock();
        let tmp = setup_empty_home();
        std::fs::create_dir_all(tmp.path().join("config")).expect("create config dir");
        std::env::set_var("EXEMEM_API_URL", "http://127.0.0.1:1");

        let data = test_app_state(&tmp);

        let entropy = [0x42u8; 32];
        let mnemonic = bip39::Mnemonic::from_entropy(&entropy).expect("mnemonic");
        let words_array: Vec<String> = mnemonic.words().map(|w| w.to_string()).collect();

        let resp =
            restore_from_phrase(data, web::Json(serde_json::json!({ "words": words_array }))).await;

        // It gets past input parsing (not 400) — restore itself fails because
        // EXEMEM_API_URL is unroutable, but that's the downstream path.
        assert_ne!(
            resp.status(),
            actix_web::http::StatusCode::BAD_REQUEST,
            "array shape must be accepted by input parsing"
        );

        std::env::remove_var("EXEMEM_API_URL");
        std::env::remove_var("FOLDDB_HOME");
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn restore_rejects_array_with_non_string_elements() {
        let _guard = env_lock();
        let tmp = setup_empty_home();
        let data = test_app_state(&tmp);

        let resp = restore_from_phrase(
            data,
            web::Json(serde_json::json!({ "words": ["nest", 42, "song"] })),
        )
        .await;

        assert_eq!(resp.status(), actix_web::http::StatusCode::BAD_REQUEST);
        let body = test_body_json(resp).await;
        assert_eq!(body["ok"], false);
        assert!(
            body["error"].as_str().unwrap_or("").contains("strings"),
            "error should explain the array-of-strings requirement, got {:?}",
            body["error"]
        );

        std::env::remove_var("FOLDDB_HOME");
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn restore_rejects_words_as_number() {
        let _guard = env_lock();
        let tmp = setup_empty_home();
        let data = test_app_state(&tmp);

        let resp = restore_from_phrase(data, web::Json(serde_json::json!({ "words": 42 }))).await;

        assert_eq!(resp.status(), actix_web::http::StatusCode::BAD_REQUEST);
        let body = test_body_json(resp).await;
        assert_eq!(body["ok"], false);
        assert!(
            body["error"]
                .as_str()
                .unwrap_or("")
                .contains("space-separated string or array"),
            "error should describe accepted shapes, got {:?}",
            body["error"]
        );

        std::env::remove_var("FOLDDB_HOME");
    }
}
