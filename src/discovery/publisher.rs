use super::config::DiscoveryOptIn;
use super::pseudonym;
use super::types::*;
use fold_db::storage::traits::KvStore;
use serde::Deserialize;
use uuid::Uuid;

const EMB_PREFIX: &str = "emb:";

/// Stored embedding entry — mirrors fold_db's StoredEmbedding.
/// Deserialized from Sled to read existing embeddings.
#[derive(Deserialize)]
struct StoredEmbedding {
    #[allow(dead_code)]
    pub schema: String,
    #[allow(dead_code)]
    pub key: fold_db::schema::types::key_value::KeyValue,
    pub field_names: Vec<String>,
    pub embedding: Vec<f32>,
}

/// Publishes embeddings from the local native index to the discovery service.
pub struct DiscoveryPublisher {
    master_key: Vec<u8>,
    discovery_url: String,
    http_client: reqwest::Client,
    auth_token: String,
}

impl DiscoveryPublisher {
    pub fn new(master_key: Vec<u8>, discovery_url: String, auth_token: String) -> Self {
        Self {
            master_key,
            discovery_url,
            http_client: reqwest::Client::new(),
            auth_token,
        }
    }

    /// Publish all records for an opted-in schema.
    ///
    /// Reads StoredEmbeddings from Sled, derives pseudonyms, and POSTs to
    /// the discovery service in a single batch.
    pub async fn publish_schema(
        &self,
        config: &DiscoveryOptIn,
        embedding_store: &dyn KvStore,
    ) -> Result<PublishResult, String> {
        let prefix = format!("{}{}:", EMB_PREFIX, config.schema_name);
        let raw_entries = embedding_store
            .scan_prefix(prefix.as_bytes())
            .await
            .map_err(|e| format!("Failed to scan embeddings: {}", e))?;

        let mut upload_entries = Vec::new();
        let mut owner_entries = Vec::new();
        let mut skipped = 0;

        for (_key_bytes, value_bytes) in &raw_entries {
            let stored: StoredEmbedding = match serde_json::from_slice(value_bytes) {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("Failed to deserialize StoredEmbedding: {}", e);
                    skipped += 1;
                    continue;
                }
            };

            if stored.embedding.is_empty() {
                skipped += 1;
                continue;
            }

            // Derive pseudonym from master_key + SHA256(embedding bytes)
            let embedding_bytes: Vec<u8> = stored
                .embedding
                .iter()
                .flat_map(|f| f.to_le_bytes())
                .collect();
            let content_hash = pseudonym::content_hash_bytes(&embedding_bytes);
            let pseudo = pseudonym::derive_pseudonym(&self.master_key, &content_hash);

            let preview = if config.include_preview {
                Some(build_preview(
                    &stored.field_names,
                    &config.preview_excluded_fields,
                    config.preview_max_chars,
                ))
            } else {
                None
            };

            upload_entries.push(DiscoveryUploadEntry {
                pseudonym: pseudo,
                embedding: stored.embedding,
                category: config.category.clone(),
                content_preview: preview,
            });

            owner_entries.push(OwnerEntry {
                pseudonym: pseudo,
                schema_name: config.schema_name.clone(),
            });
        }

        if upload_entries.is_empty() {
            return Ok(PublishResult {
                accepted: 0,
                quarantined: 0,
                total: 0,
                skipped,
            });
        }

        let total = upload_entries.len();
        let request = DiscoveryUploadRequest {
            entries: upload_entries,
            owner_entries,
        };

        let response = self
            .http_client
            .post(format!("{}/discover/upload", self.discovery_url))
            .header("Authorization", format!("Bearer {}", self.auth_token))
            .json(&request)
            .send()
            .await
            .map_err(|e| format!("Failed to upload to discovery service: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!(
                "Discovery upload failed with status {}: {}",
                status, body
            ));
        }

        let upload_response: DiscoveryUploadResponse = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse upload response: {}", e))?;

        Ok(PublishResult {
            accepted: upload_response.accepted,
            quarantined: upload_response.quarantined,
            total,
            skipped,
        })
    }

    /// Remove all published records for a schema.
    pub async fn unpublish_schema(&self, schema_name: &str) -> Result<(), String> {
        let request = DiscoveryOptOutRequest {
            schema_name: schema_name.to_string(),
        };

        let response = self
            .http_client
            .post(format!("{}/discover/opt-out", self.discovery_url))
            .header("Authorization", format!("Bearer {}", self.auth_token))
            .json(&request)
            .send()
            .await
            .map_err(|e| format!("Failed to opt-out from discovery service: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!(
                "Discovery opt-out failed with status {}: {}",
                status, body
            ));
        }

        Ok(())
    }

    /// Search the discovery network.
    pub async fn search(
        &self,
        query_embedding: Vec<f32>,
        top_k: usize,
        category_filter: Option<String>,
    ) -> Result<Vec<DiscoverySearchResult>, String> {
        let request = DiscoverySearchRequest {
            embedding: query_embedding,
            top_k,
            category_filter,
            similarity_threshold: None,
        };

        let response = self
            .http_client
            .post(format!("{}/discover/search", self.discovery_url))
            .header("Authorization", format!("Bearer {}", self.auth_token))
            .json(&request)
            .send()
            .await
            .map_err(|e| format!("Failed to search discovery service: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!(
                "Discovery search failed with status {}: {}",
                status, body
            ));
        }

        let search_response: DiscoverySearchResponse = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse search response: {}", e))?;

        Ok(search_response.results)
    }

    /// Send a connection request to a pseudonym owner.
    pub async fn connect(&self, target_pseudonym: Uuid, message: String) -> Result<(), String> {
        // Generate a one-time requester pseudonym
        let requester_pseudo = pseudonym::derive_pseudonym(
            &self.master_key,
            &pseudonym::content_hash(&format!("connect:{}", target_pseudonym)),
        );

        let request = DiscoveryConnectRequest {
            target_pseudonym,
            requester_pseudonym: requester_pseudo,
            message,
        };

        let response = self
            .http_client
            .post(format!("{}/discover/connect", self.discovery_url))
            .header("Authorization", format!("Bearer {}", self.auth_token))
            .json(&request)
            .send()
            .await
            .map_err(|e| format!("Failed to send connection request: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!(
                "Connection request failed with status {}: {}",
                status, body
            ));
        }

        Ok(())
    }

    /// Poll for incoming connection requests.
    pub async fn poll_requests(&self) -> Result<Vec<IncomingConnectionRequest>, String> {
        let response = self
            .http_client
            .get(format!("{}/discover/requests", self.discovery_url))
            .header("Authorization", format!("Bearer {}", self.auth_token))
            .send()
            .await
            .map_err(|e| format!("Failed to poll connection requests: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!(
                "Poll requests failed with status {}: {}",
                status, body
            ));
        }

        let poll_response: ConnectionRequestsResponse = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse poll response: {}", e))?;

        Ok(poll_response.requests)
    }
}

/// Build a content preview from field names, excluding specified fields.
/// Note: We only have field names from StoredEmbedding, not the actual values.
/// A full implementation would read the record's field values from the database.
/// For now, returns a comma-separated list of field names as a placeholder.
fn build_preview(field_names: &[String], excluded: &[String], max_chars: usize) -> String {
    let included: Vec<&str> = field_names
        .iter()
        .filter(|f| !excluded.iter().any(|e| e == *f))
        .map(|s| s.as_str())
        .collect();

    let preview = included.join(", ");
    if preview.len() > max_chars {
        let truncated: String = preview.chars().take(max_chars.saturating_sub(3)).collect();
        format!("{}...", truncated)
    } else {
        preview
    }
}
