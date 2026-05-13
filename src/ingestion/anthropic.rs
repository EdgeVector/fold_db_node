//! One-shot Anthropic API key validation.
//!
//! [`validate_key`] performs a cheap GET against `/v1/models` (zero-token,
//! zero-cost) so the config-save and bootstrap routes can refuse to persist
//! a key that Anthropic will reject when the user later tries to ingest. A
//! definitive 401/403 returns [`AnthropicValidationError::Invalid`]; any
//! transient failure (DNS, 5xx, timeout, etc.) returns
//! [`AnthropicValidationError::Transient`] so the caller can soft-warn
//! instead of blocking save.
//!
//! Why a separate probe instead of reusing `/v1/messages`: the messages
//! endpoint costs tokens and rate-limits per request. `/v1/models` is the
//! documented auth check — it returns the catalog when the key is valid
//! and a structured 401 when it isn't.
//!
//! [`MODELS_PATH`] is exposed so tests can hit `<mock_server>/v1/models`
//! through [`validate_key_with_base`].

use fold_db::llm_registry::models;
use reqwest::Client;
use std::time::Duration;

/// Path on the Anthropic API used as a zero-cost auth probe.
pub const MODELS_PATH: &str = "/v1/models";

/// Default upstream Anthropic API base. Tests override via
/// [`validate_key_with_base`] to point at a wiremock server.
pub const ANTHROPIC_API_BASE: &str = "https://api.anthropic.com";

/// Wall-clock cap for the probe. The /v1/models endpoint usually answers in
/// under 300ms; 8s leaves room for slow networks without making the user
/// stare at a frozen "Save" button.
const PROBE_TIMEOUT_SECS: u64 = 8;

/// Outcome of a single [`validate_key`] call.
#[derive(Debug)]
pub enum AnthropicValidationError {
    /// Anthropic returned 401 or 403 — the key is definitively bad. Callers
    /// MUST refuse to persist the config and surface the upstream message to
    /// the user.
    Invalid {
        /// Upstream HTTP status (401 or 403).
        status: u16,
        /// Anthropic's response body, truncated. Echoed back to the user so
        /// they can tell whether the key is expired vs. wrong-account vs.
        /// disabled.
        upstream_message: String,
    },
    /// Network failure, 5xx, timeout, etc. The save handler should soft-warn
    /// (200 + `warning`) and still persist the config — we can't know
    /// whether the key is good, but we know it isn't *demonstrably* bad.
    Transient { detail: String },
}

impl std::fmt::Display for AnthropicValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid {
                status,
                upstream_message,
            } => {
                write!(
                    f,
                    "Anthropic key rejected (HTTP {status}): {upstream_message}"
                )
            }
            Self::Transient { detail } => {
                write!(f, "Could not validate Anthropic key: {detail}")
            }
        }
    }
}

impl std::error::Error for AnthropicValidationError {}

/// Probe the Anthropic API with `key`. Public entry point used by the routes.
///
/// In test builds (`cfg(debug_assertions)`) the optional
/// `FOLD_ANTHROPIC_PROBE_BASE_URL` env var overrides the upstream so route-level
/// tests can point the probe at a wiremock server without threading a base URL
/// argument through every handler signature. Release builds ignore the env var
/// and always hit `api.anthropic.com`.
pub async fn validate_key(key: &str) -> Result<(), AnthropicValidationError> {
    #[cfg(debug_assertions)]
    if let Ok(base) = std::env::var("FOLD_ANTHROPIC_PROBE_BASE_URL") {
        if !base.is_empty() {
            return validate_key_with_base(key, &base).await;
        }
    }
    validate_key_with_base(key, ANTHROPIC_API_BASE).await
}

/// Variant of [`validate_key`] that lets tests point at a mock server.
/// `base` is the URL prefix (no trailing slash needed) — `MODELS_PATH`
/// is appended to form the request.
pub async fn validate_key_with_base(key: &str, base: &str) -> Result<(), AnthropicValidationError> {
    let url = format!("{}{}", base.trim_end_matches('/'), MODELS_PATH);

    // trace-egress: skip-3p (Anthropic API; third-party, does not honour
    // W3C traceparent — no inject_w3c wrap).
    let client = Client::builder()
        .timeout(Duration::from_secs(PROBE_TIMEOUT_SECS))
        .no_proxy()
        .build()
        .map_err(|e| AnthropicValidationError::Transient {
            detail: format!("Failed to build HTTP client: {e}"),
        })?;

    let response = client
        .get(&url)
        .header("x-api-key", key)
        .header("anthropic-version", models::ANTHROPIC_API_VERSION)
        .send()
        .await
        .map_err(|e| AnthropicValidationError::Transient {
            detail: format!("Network error contacting Anthropic: {e}"),
        })?;

    let status = response.status();
    if status.is_success() {
        return Ok(());
    }

    // 401/403 are the documented "key invalid / forbidden" signals.
    // Everything else — 5xx, 429, unexpected status — is treated as
    // transient so we don't refuse to save when Anthropic is having a
    // bad day.
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        let upstream_message = extract_upstream_message(response).await;
        return Err(AnthropicValidationError::Invalid {
            status: status.as_u16(),
            upstream_message,
        });
    }

    let detail = match response.text().await {
        Ok(body) if !body.is_empty() => format!("HTTP {status}: {}", truncate(&body)),
        _ => format!("HTTP {status}"),
    };
    Err(AnthropicValidationError::Transient { detail })
}

/// Parse Anthropic's error JSON (`{"error":{"message":"...","type":"..."}}`)
/// and return a short, user-facing string. Falls back to the raw body when
/// the JSON shape doesn't match — Anthropic occasionally returns plain-text
/// or differently-shaped bodies during incidents.
async fn extract_upstream_message(response: reqwest::Response) -> String {
    let body = response.text().await.unwrap_or_default();
    if body.is_empty() {
        return "Anthropic returned no body".to_string();
    }
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&body) {
        if let Some(msg) = parsed
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
        {
            return msg.to_string();
        }
    }
    truncate(&body)
}

fn truncate(body: &str) -> String {
    if body.len() > 240 {
        format!("{}...", &body[..240])
    } else {
        body.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn returns_ok_on_200() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(MODELS_PATH))
            .and(header("x-api-key", "sk-ant-good"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [],
                "has_more": false
            })))
            .mount(&server)
            .await;
        validate_key_with_base("sk-ant-good", &server.uri())
            .await
            .expect("good key must pass");
    }

    #[tokio::test]
    async fn returns_invalid_on_401() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(MODELS_PATH))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "type": "error",
                "error": {
                    "type": "authentication_error",
                    "message": "invalid x-api-key"
                }
            })))
            .mount(&server)
            .await;
        let err = validate_key_with_base("sk-ant-bad", &server.uri())
            .await
            .expect_err("bad key must fail");
        match err {
            AnthropicValidationError::Invalid {
                status,
                upstream_message,
            } => {
                assert_eq!(status, 401);
                assert!(
                    upstream_message.contains("invalid x-api-key"),
                    "should extract upstream message, got: {upstream_message}"
                );
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn returns_invalid_on_403() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(MODELS_PATH))
            .respond_with(ResponseTemplate::new(403).set_body_string("forbidden"))
            .mount(&server)
            .await;
        let err = validate_key_with_base("sk-ant-revoked", &server.uri())
            .await
            .expect_err("revoked key must fail");
        assert!(matches!(
            err,
            AnthropicValidationError::Invalid { status: 403, .. }
        ));
    }

    #[tokio::test]
    async fn returns_transient_on_500() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(MODELS_PATH))
            .respond_with(ResponseTemplate::new(500).set_body_string("upstream blew up"))
            .mount(&server)
            .await;
        let err = validate_key_with_base("sk-ant-any", &server.uri())
            .await
            .expect_err("5xx must surface as transient");
        assert!(
            matches!(err, AnthropicValidationError::Transient { .. }),
            "5xx must be transient, got {err:?}"
        );
    }

    #[tokio::test]
    async fn returns_transient_on_dns_failure() {
        // Unrouteable scheme so the request fails before any TCP — no
        // sleeping on a real connect timeout.
        let err = validate_key_with_base("sk-ant-any", "http://does.not.exist.invalid:1")
            .await
            .expect_err("unreachable host must surface as transient");
        assert!(
            matches!(err, AnthropicValidationError::Transient { .. }),
            "DNS failure must be transient, got {err:?}"
        );
    }

    #[tokio::test]
    async fn returns_transient_on_429_so_we_dont_block_save() {
        // Rate-limit during config save shouldn't block the user from
        // saving — soft-warn and let the actual ingestion call retry
        // through the AI client's existing retry path.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(MODELS_PATH))
            .respond_with(ResponseTemplate::new(429).set_body_string("rate limited"))
            .mount(&server)
            .await;
        let err = validate_key_with_base("sk-ant-good", &server.uri())
            .await
            .expect_err("429 must surface as transient, not Invalid");
        assert!(
            matches!(err, AnthropicValidationError::Transient { .. }),
            "429 must be transient (so we don't refuse to save), got {err:?}"
        );
    }
}
