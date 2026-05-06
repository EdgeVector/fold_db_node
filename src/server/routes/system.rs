use crate::handlers::system::NodeKeyResponse;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_result_to_response, node_or_return};
use actix_web::{web, HttpResponse, Responder};
use serde::Serialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;
use utoipa::ToSchema;

// Note: /api/system/sync-status reuses super::sync::get_sync_status (same handler).

/// Server start time. Initialized in `FoldHttpServer::run()`; read by
/// `/api/health` to report uptime. Kept as a process-global `OnceLock` so
/// the liveness probe can answer without depending on any per-request node
/// or user context.
static SERVER_START: OnceLock<Instant> = OnceLock::new();

/// Record server start time. Called exactly once from `FoldHttpServer::run()`.
pub fn mark_server_start() {
    let _ = SERVER_START.set(Instant::now());
}

/// Unauthenticated liveness endpoint. Returns `{ok, version, uptime_s}` with
/// no middleware gating. Use this — not `/api/system/status` — for uptime
/// monitors, load balancers, and `curl`-style probes. `/api/system/status`
/// leaks node state and therefore requires `X-User-Hash`.
#[utoipa::path(
    get,
    path = "/api/health",
    tag = "system",
    responses(
        (status = 200, description = "Server is alive", body = serde_json::Value)
    )
)]
pub async fn health_check() -> impl Responder {
    let uptime_s = SERVER_START
        .get()
        .map(|t| t.elapsed().as_secs())
        .unwrap_or(0);
    // Use FOLDDB_BUILD_VERSION (stamped from GITHUB_REF_NAME / git describe
    // in build.rs) for parity with `folddb --version`. CARGO_PKG_VERSION
    // reads the workspace manifest, which drifts: v0.3.6 tarballs reported
    // CARGO_PKG_VERSION=0.3.1 via this endpoint during brew-install-dogfood-
    // run-3 even though the binary itself correctly reported 0.3.6.
    HttpResponse::Ok().json(json!({
        "ok": true,
        "version": env!("FOLDDB_BUILD_VERSION"),
        "uptime_s": uptime_s,
    }))
}

/// Get system status information
#[utoipa::path(
    get,
    path = "/api/system/status",
    tag = "system",
    responses(
        (status = 200, description = "System status", body = serde_json::Value)
    )
)]
pub async fn get_system_status(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(crate::handlers::system::get_system_status(&user_hash, &node).await)
}

/// Wire shape of `GET /api/system/sample-data-availability`. UI gates the
/// "Try sample data" shortcut on `available`, and feeds `path` directly to
/// the scan endpoint so it works regardless of the daemon's CWD (relevant
/// to brew-installed users, who launch from `~` rather than the repo root).
#[derive(Serialize, ToSchema)]
pub struct SampleDataAvailability {
    pub available: bool,
    /// Absolute, canonicalized path to the resolved `sample_data/` directory.
    /// `None` when no candidate matched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Resolution chain for the `sample_data/` directory. Returns the first
/// existing absolute directory; `None` when none match.
///
/// Priority:
///   a. `$FOLDDB_SAMPLE_DATA_DIR` (explicit override; if set but invalid,
///      we do NOT fall through — the user asked for that path specifically).
///   b. `<CWD>/sample_data` — what `run.sh` hits in dev.
///   c. `<exe_dir>/sample_data` — sibling-of-binary brew layout.
///   d. `<exe_dir>/../share/folddb/sample_data` — canonical Homebrew prefix.
pub(crate) fn resolve_sample_data_dir() -> Option<PathBuf> {
    if let Some(raw) = std::env::var_os("FOLDDB_SAMPLE_DATA_DIR") {
        return canonical_dir(Path::new(&raw));
    }

    if let Ok(cwd) = std::env::current_dir() {
        if let Some(p) = canonical_dir(&cwd.join("sample_data")) {
            return Some(p);
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            for candidate in [
                exe_dir.join("sample_data"),
                exe_dir.join("../share/folddb/sample_data"),
            ] {
                if let Some(p) = canonical_dir(&candidate) {
                    return Some(p);
                }
            }
        }
    }

    None
}

fn canonical_dir(p: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(p).ok().filter(|c| c.is_dir())
}

/// Build the wire shape from a resolved (or unresolved) sample-data path.
/// Pulled out of the handler so it can be unit-tested without spinning up
/// an actix runtime — the handler is a thin shell over this.
pub(crate) fn build_sample_data_availability(resolved: Option<PathBuf>) -> SampleDataAvailability {
    match resolved {
        Some(path) => SampleDataAvailability {
            available: true,
            path: Some(path.to_string_lossy().into_owned()),
        },
        None => SampleDataAvailability {
            available: false,
            path: None,
        },
    }
}

/// Probe whether the bundled `sample_data/` directory ships alongside the
/// running binary so the UI can show the "Try sample data" onboarding
/// shortcut. Unauthenticated — purely environment introspection, no node
/// or user state involved.
#[utoipa::path(
    get,
    path = "/api/system/sample-data-availability",
    tag = "system",
    responses(
        (status = 200, description = "Sample data availability probe", body = SampleDataAvailability)
    )
)]
pub async fn sample_data_availability() -> impl Responder {
    HttpResponse::Ok().json(build_sample_data_availability(resolve_sample_data_dir()))
}

/// Get the node's public key
///
/// This endpoint returns the node's public key for verification purposes.
/// The public key is generated automatically when the node is created.
#[utoipa::path(
    get,
    path = "/api/system/public-key",
    tag = "system",
    responses(
        (status = 200, description = "Node public key", body = NodeKeyResponse)
    )
)]
pub async fn get_node_public_key(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);
    let response = NodeKeyResponse {
        success: true,
        public_key: node.get_node_public_key().to_string(),
        message: "Node public key retrieved successfully".to_string(),
    };
    tracing::info!(
        target: "fold_node::http_server",
        "Node public key retrieved successfully"
    );
    HttpResponse::Ok().json(&response)
}

#[cfg(test)]
mod sample_data_tests {
    //! Tests for `resolve_sample_data_dir` and the
    //! `/api/system/sample-data-availability` endpoint. Lives in its own
    //! module so the `actix_web::test` import in the sibling `tests` module
    //! (which is the actix-web `test` *macro* — shadows `#[test]`) doesn't
    //! interfere with synchronous `#[test]` functions here.
    use super::*;
    use std::ffi::OsString;
    use tempfile::tempdir;

    /// Serializes write access to the process-global env vars / CWD that
    /// `resolve_sample_data_dir` reads. Without this, parallel test threads
    /// stomp each other (set + canonicalize is not atomic) and produce flaky
    /// failures.
    static SAMPLE_DATA_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct EnvGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &Path) -> Self {
            let previous = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, previous }
        }

        fn unset(key: &'static str) -> Self {
            let previous = std::env::var_os(key);
            std::env::remove_var(key);
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    struct CwdGuard {
        previous: PathBuf,
    }

    impl CwdGuard {
        fn change_to(path: &Path) -> Self {
            let previous = std::env::current_dir().expect("current_dir");
            std::env::set_current_dir(path).expect("chdir");
            Self { previous }
        }
    }

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.previous);
        }
    }

    /// (a) Explicit env-var override — pointing at a real directory wins
    /// over every other candidate.
    #[test]
    fn resolve_sample_data_prefers_env_override() {
        let _lock = SAMPLE_DATA_ENV_LOCK.lock().unwrap();
        let td = tempdir().unwrap();
        let override_dir = td.path().join("custom_sample_data");
        std::fs::create_dir(&override_dir).unwrap();

        let _guard = EnvGuard::set("FOLDDB_SAMPLE_DATA_DIR", &override_dir);

        let resolved = resolve_sample_data_dir().expect("override resolves");
        assert_eq!(
            resolved.canonicalize().unwrap(),
            override_dir.canonicalize().unwrap(),
        );
    }

    /// (a, negative) Env override pointing at a missing path returns None
    /// and does NOT fall through to CWD or exe-relative candidates — the
    /// user explicitly chose that path; honor their choice including the
    /// "no sample data" implication.
    #[test]
    fn resolve_sample_data_env_override_invalid_returns_none() {
        let _lock = SAMPLE_DATA_ENV_LOCK.lock().unwrap();
        let td = tempdir().unwrap();
        let bogus = td.path().join("does_not_exist");
        let _env = EnvGuard::set("FOLDDB_SAMPLE_DATA_DIR", &bogus);

        // Stage a CWD candidate that *would* resolve if fall-through happened.
        let cwd_td = tempdir().unwrap();
        std::fs::create_dir(cwd_td.path().join("sample_data")).unwrap();
        let _cwd = CwdGuard::change_to(cwd_td.path());

        assert!(resolve_sample_data_dir().is_none());
    }

    /// (b) No env override — falls back to `<CWD>/sample_data` (the
    /// `run.sh` dev path).
    #[test]
    fn resolve_sample_data_falls_back_to_cwd() {
        let _lock = SAMPLE_DATA_ENV_LOCK.lock().unwrap();
        let _env = EnvGuard::unset("FOLDDB_SAMPLE_DATA_DIR");
        let td = tempdir().unwrap();
        let sample_dir = td.path().join("sample_data");
        std::fs::create_dir(&sample_dir).unwrap();

        let _cwd = CwdGuard::change_to(td.path());

        let resolved = resolve_sample_data_dir().expect("cwd resolves");
        assert_eq!(
            resolved.canonicalize().unwrap(),
            sample_dir.canonicalize().unwrap(),
        );
    }

    /// (d / fallthrough) No env, no CWD candidate, and the exe-adjacent
    /// candidates also miss → None. Locks the contract that an empty
    /// environment really does return `available:false`, so the UI hides
    /// the button instead of showing a broken shortcut.
    #[test]
    fn resolve_sample_data_returns_none_when_nothing_matches() {
        let _lock = SAMPLE_DATA_ENV_LOCK.lock().unwrap();
        let _env = EnvGuard::unset("FOLDDB_SAMPLE_DATA_DIR");
        let td = tempdir().unwrap();
        // Empty CWD with no sample_data subdir, and the test binary's
        // sibling/share path is overwhelmingly unlikely to have one either.
        let _cwd = CwdGuard::change_to(td.path());

        assert!(resolve_sample_data_dir().is_none());
    }

    /// Wire-shape smoke test — when a resolved path is supplied, the JSON
    /// is `{available:true, path:"<absolute>"}`. Tests the builder rather
    /// than spinning up actix because the handler is a one-line shell over
    /// `build_sample_data_availability`.
    #[test]
    fn build_response_reports_resolved_path() {
        let td = tempdir().unwrap();
        let dir = td.path().join("explicit_sample_data");
        std::fs::create_dir(&dir).unwrap();
        let canonical = dir.canonicalize().unwrap();

        let response = build_sample_data_availability(Some(canonical.clone()));
        let body = serde_json::to_value(&response).unwrap();

        assert_eq!(body["available"], true);
        let path = body["path"].as_str().expect("path is a string");
        assert!(
            Path::new(path).is_absolute(),
            "response must carry absolute path, got {path}"
        );
        assert_eq!(std::fs::canonicalize(path).unwrap(), canonical,);
    }

    /// Wire-shape smoke test — when nothing resolves, the JSON is
    /// `{available:false}` with `path` omitted. Locks `skip_serializing_if`.
    #[test]
    fn build_response_omits_path_when_unavailable() {
        let response = build_sample_data_availability(None);
        let body = serde_json::to_value(&response).unwrap();

        assert_eq!(body["available"], false);
        assert!(
            body.get("path").is_none(),
            "path must be omitted when unavailable, got {body}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::middleware::auth::UserContextMiddleware;
    use crate::server::routes::common::test_helpers::create_test_state;
    use actix_web::{test, App};
    use tempfile::tempdir;

    /// `/api/health` must answer 200 with no auth, no `X-User-Hash`, and no
    /// `AppState`. Regression guard for the brew-install dogfood papercut
    /// (2026-04-20) where external monitors had no unauth liveness probe.
    #[actix_web::test]
    async fn health_endpoint_is_unauthenticated() {
        let app = test::init_service(
            App::new()
                .wrap(UserContextMiddleware)
                .route("/api/health", actix_web::web::get().to(health_check)),
        )
        .await;

        let req = test::TestRequest::get().uri("/api/health").to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 200, "health must be 200 without credentials");

        let body: serde_json::Value = test::read_body_json(resp).await;
        assert_eq!(body["ok"], true);
        assert!(body["version"].is_string(), "version must be a string");
        // Must match the compile-time build stamp (FOLDDB_BUILD_VERSION),
        // NOT Cargo.toml — see brew-install-dogfood-run-3 BLOCKER.
        assert_eq!(body["version"], env!("FOLDDB_BUILD_VERSION"));
        assert!(body["uptime_s"].is_u64(), "uptime_s must be a u64");
    }

    #[tokio::test]
    async fn test_system_status() {
        let temp_dir = tempdir().unwrap();
        let state = create_test_state(&temp_dir).await;

        // Need to run with user context since routes now require authentication
        fold_db::user_context::run_with_user("test_user", async move {
            let req = test::TestRequest::get().to_http_request();
            let resp = get_system_status(state).await.respond_to(&req);
            assert_eq!(resp.status(), 200);
        })
        .await;
    }

    #[tokio::test]
    async fn test_get_node_public_key() {
        let temp_dir = tempdir().unwrap();
        let state = create_test_state(&temp_dir).await;

        fold_db::user_context::run_with_user("test_user", async move {
            let req = test::TestRequest::get().to_http_request();
            let resp = get_node_public_key(state).await.respond_to(&req);
            assert_eq!(resp.status(), 200);

            // Parse the response to verify it contains the public key
            let body = resp.into_body();
            let bytes = actix_web::body::to_bytes(body).await.unwrap_or_default();
            let response: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();

            assert!(response["success"].as_bool().unwrap_or(false));
            assert!(response["public_key"].as_str().is_some());
            assert!(!response["public_key"].as_str().unwrap_or("").is_empty());
            // Regression guard: the route used to rename `key` -> `public_key`
            // via `serde_json::json!`, so the typed handler struct lied about
            // the wire shape. After the typed-struct refactor the wire field
            // is `public_key` and `key` MUST NOT appear.
            assert!(
                response.get("key").is_none(),
                "wire payload must not include legacy `key` field, got: {response}"
            );
            assert_eq!(
                response["message"],
                "Node public key retrieved successfully"
            );
        })
        .await;
    }
}
