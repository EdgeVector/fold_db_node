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
    include!(concat!(env!("OUT_DIR"), "/environments_generated.rs"));
}

use std::sync::OnceLock;

/// Service environment — dev or prod.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    Dev,
    Prod,
}

static RESOLVED_ENV: OnceLock<Environment> = OnceLock::new();

impl Environment {
    /// Resolve from `EXEMEM_ENV` env var. When unset, the default depends on
    /// the build profile: debug → Dev (so `cargo run` and `tauri dev` stay
    /// pointed at us-west-2), release → Prod (so the shipped Tauri bundle
    /// hits us-east-1 without needing a runtime env var).
    ///
    /// The resolution (and the accompanying log line announcing it) runs
    /// exactly once per process — every endpoint helper calls this on every
    /// request, so without the cache the "EXEMEM_ENV not set, defaulting to
    /// prod" line drowns the rest of `observability.jsonl`.
    pub fn from_env() -> Self {
        *RESOLVED_ENV.get_or_init(Self::resolve_once)
    }

    fn resolve_once() -> Self {
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

fn service_url(env_var: &str, dev: &'static str, prod: &'static str) -> String {
    std::env::var(env_var).unwrap_or_else(|_| match Environment::from_env() {
        Environment::Dev => dev.to_string(),
        Environment::Prod => prod.to_string(),
    })
}

/// Schema service URL.
pub fn schema_service_url() -> String {
    service_url(
        "FOLD_SCHEMA_SERVICE_URL",
        gen::DEV_SCHEMA_SERVICE,
        gen::PROD_SCHEMA_SERVICE,
    )
}

/// Exemem API URL (auth, sync, etc.).
pub fn exemem_api_url() -> String {
    service_url("EXEMEM_API_URL", gen::DEV_EXEMEM_API, gen::PROD_EXEMEM_API)
}

/// Discovery service URL.
pub fn discovery_service_url() -> String {
    service_url(
        "DISCOVERY_SERVICE_URL",
        gen::DEV_DISCOVERY,
        gen::PROD_DISCOVERY,
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use observability::layers::ring::build_ring_layer;
    use tracing_subscriber::{layer::SubscriberExt, Registry};

    /// `Environment::from_env()` resolves the env exactly once per process,
    /// no matter how many times callers invoke it. Before the OnceLock cache,
    /// every endpoint helper (`schema_service_url`, `exemem_api_url`,
    /// `discovery_service_url`) called this and emitted a fresh
    /// "EXEMEM_ENV not set..." line — drowning real signal in
    /// `observability.jsonl`. We assert *at most* one because the OnceLock
    /// is process-global: another test in the same binary may have triggered
    /// resolution first, in which case our 1000 calls produce zero new lines.
    #[test]
    fn from_env_logs_at_most_once_across_many_calls() {
        let (ring_layer, ring) = build_ring_layer(4096);
        let subscriber = Registry::default().with(ring_layer);

        tracing::subscriber::with_default(subscriber, || {
            for _ in 0..1000 {
                let _ = Environment::from_env();
            }
        });

        let count = ring
            .query(None, None)
            .into_iter()
            .filter(|e| e.message.contains("EXEMEM_ENV"))
            .count();
        assert!(
            count <= 1,
            "expected at most one EXEMEM_ENV log line, got {count}"
        );
    }
}
