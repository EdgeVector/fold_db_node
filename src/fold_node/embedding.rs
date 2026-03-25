use fold_db::db_operations::native_index::{Embedder, FastEmbedModel};
use fold_db::schema::SchemaError;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Configuration for connecting to an Ollama embedding server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Ollama server base URL, e.g. "http://localhost:11434"
    pub base_url: String,
    /// Ollama model name, e.g. "qwen3-embedding:0.6b"
    pub model: String,
    /// Optional MRL truncation dimension. None = model default.
    pub dimensions: Option<u32>,
}

/// Selects which embedding backend to use.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EmbeddingProvider {
    FastEmbed,
    Ollama(EmbeddingConfig),
}

impl EmbeddingProvider {
    /// Default provider: Ollama with Qwen3-Embedding 4B.
    pub fn default_ollama() -> Self {
        EmbeddingProvider::Ollama(EmbeddingConfig {
            base_url: "http://localhost:11434".to_string(),
            model: "qwen3-embedding:4b".to_string(),
            dimensions: None,
        })
    }
}

/// Embedding model that calls a local Ollama server.
pub struct OllamaEmbedder {
    url: String,
    model: String,
    dimensions: Option<u32>,
    client: reqwest::Client,
}

impl OllamaEmbedder {
    pub fn new(config: &EmbeddingConfig) -> Result<Self, SchemaError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| SchemaError::InvalidData(format!("Failed to build HTTP client: {}", e)))?;
        Ok(Self {
            url: format!("{}/api/embed", config.base_url),
            model: config.model.clone(),
            dimensions: config.dimensions,
            client,
        })
    }
}

impl Embedder for OllamaEmbedder {
    fn embed_text(&self, text: &str) -> Result<Vec<f32>, SchemaError> {
        let mut body = serde_json::json!({
            "model": self.model,
            "input": text,
        });
        if let Some(dims) = self.dimensions {
            body["dimensions"] = serde_json::json!(dims);
        }

        let parsed: serde_json::Value = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let response = self.client
                    .post(&self.url)
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| SchemaError::InvalidData(format!("Ollama request failed: {}", e)))?;

                if !response.status().is_success() {
                    return Err(SchemaError::InvalidData(format!(
                        "Ollama returned status {}",
                        response.status()
                    )));
                }

                response
                    .json::<serde_json::Value>()
                    .await
                    .map_err(|e| SchemaError::InvalidData(format!("Failed to parse Ollama response: {}", e)))
            })
        })?;

        let embedding = parsed["embeddings"][0]
            .as_array()
            .ok_or_else(|| SchemaError::InvalidData("Ollama response missing embeddings[0]".to_string()))?
            .iter()
            .map(|v| {
                v.as_f64()
                    .map(|f| f as f32)
                    .ok_or_else(|| SchemaError::InvalidData("Non-float value in embedding".to_string()))
            })
            .collect::<Result<Vec<f32>, _>>()?;

        Ok(embedding)
    }
}

/// Build an `Arc<dyn Embedder>` from the given provider configuration.
pub fn build_embedder(provider: &EmbeddingProvider) -> Result<Arc<dyn Embedder>, SchemaError> {
    match provider {
        EmbeddingProvider::FastEmbed => Ok(Arc::new(FastEmbedModel::new())),
        EmbeddingProvider::Ollama(config) => Ok(Arc::new(OllamaEmbedder::new(config)?)),
    }
}
