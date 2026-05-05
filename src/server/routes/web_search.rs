//! HTTP route handlers for the persistent Brave Search API key.
//!
//! Backed by [`web_search_key_store`](crate::fold_node::llm_query::service::web_search_key_store);
//! the LLM agent's `web_search` tool reads the same on-disk key (with an env-
//! var fallback) so users no longer need to export `WEB_SEARCH_API_KEY` from
//! their shell.

use crate::fold_node::llm_query::service::web_search_key_store;
use crate::server::startup::ConfigDir;
use actix_web::{web, HttpResponse, Responder};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Response body for `GET /api/web_search/key`. We expose only a presence flag
/// so the saved key never leaves the server — even back to the user who set
/// it.
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct WebSearchKeyStatus {
    /// `true` when a non-empty key is persisted on disk.
    pub has_key: bool,
}

/// Request body for `POST /api/web_search/key`. An empty `api_key` deletes the
/// saved key — no separate DELETE route needed. `Serialize` is derived only so
/// the test harness can `set_json(...)` without an extra wrapper struct.
#[derive(Debug, Deserialize, Serialize, utoipa::ToSchema)]
pub struct SaveWebSearchKeyRequest {
    pub api_key: String,
}

/// Whether a Brave Search API key is currently stored on disk.
#[utoipa::path(
    get,
    path = "/api/web_search/key",
    tag = "web_search",
    responses(
        (status = 200, description = "Web search key status", body = WebSearchKeyStatus),
    )
)]
pub async fn get_web_search_key_status(config_dir: web::Data<ConfigDir>) -> impl Responder {
    let has_key = web_search_key_store::has_key(config_dir.as_path());
    HttpResponse::Ok().json(WebSearchKeyStatus { has_key })
}

/// Persist (or clear) the Brave Search API key. The key is stored encrypted
/// when the `os-keychain` feature is on, and as a `0o600` plaintext file
/// otherwise.
#[utoipa::path(
    post,
    path = "/api/web_search/key",
    tag = "web_search",
    request_body = SaveWebSearchKeyRequest,
    responses(
        (status = 200, description = "Saved (or cleared)", body = WebSearchKeyStatus),
        (status = 500, description = "Failed to persist the key"),
    )
)]
pub async fn save_web_search_key(
    request: web::Json<SaveWebSearchKeyRequest>,
    config_dir: web::Data<ConfigDir>,
) -> impl Responder {
    match web_search_key_store::save(config_dir.as_path(), &request.api_key) {
        Ok(()) => {
            let has_key = web_search_key_store::has_key(config_dir.as_path());
            HttpResponse::Ok().json(WebSearchKeyStatus { has_key })
        }
        Err(e) => HttpResponse::InternalServerError().json(json!({
            "success": false,
            "error": format!("Failed to save web search key: {}", e),
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{test, App};
    use tempfile::TempDir;

    fn config_dir_data(tmp: &TempDir) -> web::Data<ConfigDir> {
        web::Data::new(ConfigDir(tmp.path().to_path_buf()))
    }

    #[actix_web::test]
    async fn get_returns_false_when_no_key() {
        let tmp = TempDir::new().expect("tempdir");
        let app = test::init_service(
            App::new()
                .app_data(config_dir_data(&tmp))
                .route("/key", web::get().to(get_web_search_key_status)),
        )
        .await;

        let req = test::TestRequest::get().uri("/key").to_request();
        let resp: WebSearchKeyStatus = test::call_and_read_body_json(&app, req).await;
        assert!(!resp.has_key);
    }

    #[actix_web::test]
    async fn post_then_get_reflects_saved_key() {
        // POST goes through `web_search_key_store::save -> sensitive_io ->
        // secure_store::encrypt_and_write`, which now requires a master
        // key. Acquire the global env-var lock + set FOLDDB_MASTER_KEY.
        let _master = crate::secure_store::test_master_key::with_set();
        let tmp = TempDir::new().expect("tempdir");
        let app = test::init_service(
            App::new()
                .app_data(config_dir_data(&tmp))
                .route("/key", web::get().to(get_web_search_key_status))
                .route("/key", web::post().to(save_web_search_key)),
        )
        .await;

        let post = test::TestRequest::post()
            .uri("/key")
            .set_json(SaveWebSearchKeyRequest {
                api_key: "BRAVE_TEST".to_string(),
            })
            .to_request();
        let post_resp: WebSearchKeyStatus = test::call_and_read_body_json(&app, post).await;
        assert!(post_resp.has_key);

        let get = test::TestRequest::get().uri("/key").to_request();
        let get_resp: WebSearchKeyStatus = test::call_and_read_body_json(&app, get).await;
        assert!(get_resp.has_key);
    }

    #[actix_web::test]
    async fn post_empty_body_deletes_key() {
        let _master = crate::secure_store::test_master_key::with_set();
        let tmp = TempDir::new().expect("tempdir");
        let app = test::init_service(
            App::new()
                .app_data(config_dir_data(&tmp))
                .route("/key", web::get().to(get_web_search_key_status))
                .route("/key", web::post().to(save_web_search_key)),
        )
        .await;

        let post = test::TestRequest::post()
            .uri("/key")
            .set_json(SaveWebSearchKeyRequest {
                api_key: "BRAVE_TEST".to_string(),
            })
            .to_request();
        let _: WebSearchKeyStatus = test::call_and_read_body_json(&app, post).await;

        let clear = test::TestRequest::post()
            .uri("/key")
            .set_json(SaveWebSearchKeyRequest {
                api_key: String::new(),
            })
            .to_request();
        let cleared: WebSearchKeyStatus = test::call_and_read_body_json(&app, clear).await;
        assert!(!cleared.has_key);

        let get = test::TestRequest::get().uri("/key").to_request();
        let get_resp: WebSearchKeyStatus = test::call_and_read_body_json(&app, get).await;
        assert!(!get_resp.has_key);
    }
}
