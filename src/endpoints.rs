//! Centralized endpoint registry for Exemem services.
//!
//! All cross-environment URLs live in [`environments.json`] at the repo
//! root — that is the **single source of truth**. `build.rs` parses it at
//! compile time and emits per-(env, key) constants in `OUT_DIR`, included
//! below via `gen::*`. Edit URLs in `environments.json`; do NOT add
//! hardcoded gateway hostnames in Rust, shell, or config files.
//! `scripts/lint-no-hardcoded-urls.sh` enforces this in CI.
//!
//! The active environment is selected by `EXEMEM_ENV`. Per-call overrides
//! still work (`FOLD_SCHEMA_SERVICE_URL`, `EXEMEM_API_URL`,
//! `DISCOVERY_SERVICE_URL`) for ad-hoc testing.

mod gen {
    // `region` constants are exposed for downstream tooling and aren't
    // referenced inside fold_db_node itself.
    #![allow(dead_code)]
    include!(concat!(env!("OUT_DIR"), "/environments_generated.rs"));
}

/// Service environment — dev or prod.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    Dev,
    Prod,
}

impl Environment {
    /// Resolve from `EXEMEM_ENV` env var. When unset, the default depends on
    /// the build profile: debug → Dev (so `cargo run` and `tauri dev` stay
    /// pointed at us-west-2), release → Prod (so the shipped Tauri bundle
    /// hits us-east-1 without needing a runtime env var).
    pub fn from_env() -> Self {
        match std::env::var("EXEMEM_ENV").as_deref() {
            Ok("prod") | Ok("production") => Self::Prod,
            Ok("dev") | Ok("development") => Self::Dev,
            Ok(other) => {
                tracing::error!(
                    "EXEMEM_ENV has unknown value '{}', defaulting to dev",
                    other
                );
                Self::Dev
            }
            Err(_) => {
                if cfg!(debug_assertions) {
                    tracing::debug!("EXEMEM_ENV not set, defaulting to dev (debug build)");
                    Self::Dev
                } else {
                    tracing::info!("EXEMEM_ENV not set, defaulting to prod (release build)");
                    Self::Prod
                }
            }
        }
    }
}

/// Schema service URL.
pub fn schema_service_url() -> String {
    std::env::var("FOLD_SCHEMA_SERVICE_URL").unwrap_or_else(|_| match Environment::from_env() {
        Environment::Dev => gen::DEV_SCHEMA_SERVICE.to_string(),
        Environment::Prod => gen::PROD_SCHEMA_SERVICE.to_string(),
    })
}

/// Exemem API URL (auth, sync, etc.).
pub fn exemem_api_url() -> String {
    std::env::var("EXEMEM_API_URL").unwrap_or_else(|_| match Environment::from_env() {
        Environment::Dev => gen::DEV_EXEMEM_API.to_string(),
        Environment::Prod => gen::PROD_EXEMEM_API.to_string(),
    })
}

/// Discovery service URL.
pub fn discovery_service_url() -> String {
    std::env::var("DISCOVERY_SERVICE_URL").unwrap_or_else(|_| match Environment::from_env() {
        Environment::Dev => gen::DEV_DISCOVERY.to_string(),
        Environment::Prod => gen::PROD_DISCOVERY.to_string(),
    })
}

/// Schema service URL for a specific environment, ignoring `EXEMEM_ENV`.
/// Used by tooling (e.g. the daemon launcher's `--dev` flag) that needs to
/// pin the dev URL regardless of the calling process's env.
pub fn schema_service_url_for(env: Environment) -> &'static str {
    match env {
        Environment::Dev => gen::DEV_SCHEMA_SERVICE,
        Environment::Prod => gen::PROD_SCHEMA_SERVICE,
    }
}
