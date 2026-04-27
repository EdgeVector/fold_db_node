//! Centralized endpoint registry for Exemem services.
//!
//! All external service URLs are defined here with dev/prod variants.
//! The active environment is selected by `EXEMEM_ENV` (default: "dev").

/// Service environment — dev or prod.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    Dev,
    Prod,
}

impl Environment {
    /// Resolve from `EXEMEM_ENV` env var. Defaults to Dev to prevent
    /// accidental production registration during development.
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
                // EXEMEM_ENV is unset on every local dev boot — noisy if
                // logged at warn-level. Keep at debug; release builds set
                // EXEMEM_ENV=prod via build config.
                tracing::debug!("EXEMEM_ENV not set, defaulting to dev");
                Self::Dev
            }
        }
    }
}

/// Schema service URL.
pub fn schema_service_url() -> String {
    std::env::var("FOLD_SCHEMA_SERVICE_URL").unwrap_or_else(|_| match Environment::from_env() {
        Environment::Dev => "https://y0q3m6vk75.execute-api.us-west-2.amazonaws.com".to_string(),
        Environment::Prod => "https://axo709qs11.execute-api.us-east-1.amazonaws.com".to_string(),
    })
}

/// Exemem API URL (auth, sync, etc.).
pub fn exemem_api_url() -> String {
    std::env::var("EXEMEM_API_URL").unwrap_or_else(|_| match Environment::from_env() {
        Environment::Dev => "https://ygyu7ritx8.execute-api.us-west-2.amazonaws.com".to_string(),
        Environment::Prod => "https://jdsx4ixk2i.execute-api.us-east-1.amazonaws.com".to_string(),
    })
}

/// Discovery service URL.
pub fn discovery_service_url() -> String {
    std::env::var("DISCOVERY_SERVICE_URL").unwrap_or_else(|_| match Environment::from_env() {
        Environment::Dev => {
            "https://ygyu7ritx8.execute-api.us-west-2.amazonaws.com/api".to_string()
        }
        Environment::Prod => {
            "https://jdsx4ixk2i.execute-api.us-east-1.amazonaws.com/api".to_string()
        }
    })
}
