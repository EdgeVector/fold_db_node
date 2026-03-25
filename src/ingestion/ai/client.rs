//! Unified AI backend abstraction for Anthropic and Ollama.

use crate::ingestion::config::{
    AIProvider, AnthropicConfig, IngestionConfig, OllamaConfig, OllamaGenerationParams,
};
use crate::ingestion::{IngestionError, IngestionResult};
use async_trait::async_trait;
use fold_db::llm_registry::models;
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

/// Trait implemented by each AI provider backend.
#[async_trait]
pub trait AiBackend: Send + Sync {
    async fn call(&self, prompt: &str) -> IngestionResult<String>;
}

/// Check an HTTP response for errors, returning the response if successful or
/// a classified `IngestionError` if the status indicates failure.
async fn check_error_response(
    provider: &str,
    response: reqwest::Response,
) -> IngestionResult<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status().as_u16();
    let error_text = response
        .text()
        .await
        .unwrap_or_else(|_| "Unknown error".to_string());
    Err(crate::ingestion::error::classify_llm_error(
        provider,
        status,
        &error_text,
    ))
}

/// Build an HTTP client with standard settings (timeout, no proxy).
fn build_http_client(timeout_seconds: u64) -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(timeout_seconds))
        .no_proxy()
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))
}

// ---- Ollama ----

#[derive(Debug, Serialize)]
struct OllamaRequest {
    model: String,
    prompt: String,
    stream: bool,
    options: OllamaGenerationParams,
}

#[derive(Debug, Deserialize)]
struct OllamaResponse {
    response: String,
}

pub struct OllamaBackend {
    client: Client,
    config: OllamaConfig,
    max_retries: u32,
}

impl OllamaBackend {
    pub fn new(
        config: OllamaConfig,
        timeout_seconds: u64,
        max_retries: u32,
    ) -> IngestionResult<Self> {
        config.validate()?;
        let client = build_http_client(timeout_seconds).map_err(IngestionError::ollama_error)?;
        Ok(Self {
            client,
            config,
            max_retries,
        })
    }

    async fn make_request(&self, request: &OllamaRequest) -> IngestionResult<String> {
        let url = format!("{}/api/generate", self.config.base_url);
        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(request)
            .send()
            .await
            .map_err(|e| crate::ingestion::error::classify_transport_error("Ollama", &e))?;

        let response = check_error_response("Ollama", response).await?;
        let resp: OllamaResponse = response.json().await?;
        Ok(resp.response)
    }
}

#[async_trait]
impl AiBackend for OllamaBackend {
    async fn call(&self, prompt: &str) -> IngestionResult<String> {
        let request = OllamaRequest {
            model: self.config.model.clone(),
            prompt: prompt.to_string(),
            stream: false,
            options: self.config.generation_params.clone(),
        };
        super::helpers::call_with_retries(
            "Ollama API",
            self.max_retries,
            || IngestionError::ollama_error("All API attempts failed"),
            || self.make_request(&request),
        )
        .await
    }
}

// ---- Anthropic ----

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    messages: Vec<AnthropicMessage>,
    max_tokens: u32,
    temperature: Option<f32>,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContent>,
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Deserialize)]
struct AnthropicContent {
    text: String,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
}

pub struct AnthropicBackend {
    client: Client,
    config: AnthropicConfig,
    max_retries: u32,
}

impl AnthropicBackend {
    pub fn new(
        config: AnthropicConfig,
        timeout_seconds: u64,
        max_retries: u32,
    ) -> IngestionResult<Self> {
        config.validate()?;
        let client =
            build_http_client(timeout_seconds).map_err(IngestionError::configuration_error)?;
        Ok(Self {
            client,
            config,
            max_retries,
        })
    }

    async fn make_request(&self, request: &AnthropicRequest) -> IngestionResult<String> {
        let url = format!("{}/v1/messages", self.config.base_url);
        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", models::ANTHROPIC_API_VERSION)
            .header("Content-Type", "application/json")
            .json(request)
            .send()
            .await
            .map_err(|e| crate::ingestion::error::classify_transport_error("Anthropic", &e))?;

        let response = check_error_response("Anthropic", response).await?;
        let resp: AnthropicResponse = response.json().await?;
        if let Some(usage) = &resp.usage {
            log_feature!(
                LogFeature::Ingestion,
                info,
                "Anthropic usage - input: {:?}, output: {:?}",
                usage.input_tokens,
                usage.output_tokens
            );
        }
        if resp.content.is_empty() {
            return Err(IngestionError::configuration_error(
                "No content in Anthropic API response",
            ));
        }
        Ok(resp.content[0].text.clone())
    }
}

#[async_trait]
impl AiBackend for AnthropicBackend {
    async fn call(&self, prompt: &str) -> IngestionResult<String> {
        let request = AnthropicRequest {
            model: self.config.model.clone(),
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: prompt.to_string(),
            }],
            max_tokens: models::MAX_TOKENS_ANALYSIS,
            temperature: Some(models::TEMPERATURE_FOCUSED),
        };
        super::helpers::call_with_retries(
            "Anthropic API",
            self.max_retries,
            || IngestionError::configuration_error("All Anthropic API attempts failed"),
            || self.make_request(&request),
        )
        .await
    }
}

// ---- Factory ----

/// Build the correct backend from an `IngestionConfig`.
///
/// Returns `Ok(None)` when the configured provider fails validation so that
/// `IngestionService` can still be constructed and report status.
pub fn build_backend(config: &IngestionConfig) -> (Option<Arc<dyn AiBackend>>, Option<String>) {
    fn try_init<B: AiBackend + 'static>(
        name: &str,
        result: IngestionResult<B>,
    ) -> (Option<Arc<dyn AiBackend>>, Option<String>) {
        match result {
            Ok(b) => (Some(Arc::new(b)), None),
            Err(e) => {
                let msg = format!("{} init failed: {}", name, e);
                log::warn!("{}", msg);
                (None, Some(msg))
            }
        }
    }

    match config.provider {
        AIProvider::Ollama => try_init(
            "Ollama",
            OllamaBackend::new(
                config.ollama.clone(),
                config.timeout_seconds,
                config.max_retries,
            ),
        ),
        AIProvider::Anthropic => try_init(
            "Anthropic",
            AnthropicBackend::new(
                config.anthropic.clone(),
                config.timeout_seconds,
                config.max_retries,
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_timeout_configuration() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let base_url = format!("http://127.0.0.1:{}", port);

        let _handle = std::thread::spawn(move || {
            if let Ok((_stream, _addr)) = listener.accept() {
                std::thread::sleep(std::time::Duration::from_secs(10));
            }
        });

        let config = AnthropicConfig {
            api_key: "test-key".to_string(),
            base_url,
            ..Default::default()
        };

        let backend = AnthropicBackend::new(config, 1, 1).unwrap();
        let result = backend.call("test").await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, IngestionError::TimeoutError { .. }),
            "Expected TimeoutError, got: {:?}",
            err
        );
    }
}
