use super::config::DiscoveryOptIn;
use super::connection;
use super::pseudonym;
use super::types::*;
use fold_db::db_operations::native_index::anonymity::{default_privacy_class, FieldPrivacyClass};
use fold_db::storage::traits::KvStore;
use serde::Deserialize;
use uuid::Uuid;

const EMB_PREFIX: &str = "emb:";

/// Sled key prefix for tracking pseudonyms that have been uploaded to the
/// discovery service. This is the authoritative source for opt-out — it
/// remains complete even if the underlying `emb:{schema}:*` entries have
/// been deleted from the local DB after publication.
///
/// Key shape: `discovery:uploaded:{schema}:{pseudonym}` (value is an empty
/// marker — presence is all that matters).
const UPLOADED_PREFIX: &str = "discovery:uploaded:";

/// Compute the Sled key tracking one uploaded pseudonym under a schema.
pub fn uploaded_key(schema: &str, pseudonym: &uuid::Uuid) -> Vec<u8> {
    format!("{}{}:{}", UPLOADED_PREFIX, schema, pseudonym).into_bytes()
}

/// Record that a pseudonym has been uploaded for `schema`. Idempotent.
pub async fn record_uploaded(
    store: &dyn KvStore,
    schema: &str,
    pseudonym: &uuid::Uuid,
) -> Result<(), String> {
    store
        .put(&uploaded_key(schema, pseudonym), b"[]".to_vec())
        .await
        .map_err(|e| format!("Failed to record uploaded pseudonym: {e}"))
}

/// List all tracked uploaded pseudonyms, optionally filtered by schema.
/// Returns `(schema, pseudonym)` pairs.
pub async fn list_uploaded_pseudonyms(
    store: &dyn KvStore,
    schema: Option<&str>,
) -> Result<Vec<(String, uuid::Uuid)>, String> {
    let prefix = match schema {
        Some(s) => format!("{}{}:", UPLOADED_PREFIX, s),
        None => UPLOADED_PREFIX.to_string(),
    };
    let raw = store
        .scan_prefix(prefix.as_bytes())
        .await
        .map_err(|e| format!("Failed to scan uploaded tracking: {e}"))?;

    let mut out = Vec::with_capacity(raw.len());
    for (key_bytes, _value) in raw {
        let key_str = std::str::from_utf8(&key_bytes)
            .map_err(|e| format!("Invalid UTF-8 in uploaded tracking key: {e}"))?;
        // Strip UPLOADED_PREFIX, then split on the FINAL ':' — schema names
        // may contain ':'-free characters in practice, but we split from the
        // right to be safe because the pseudonym (UUID) has a fixed format.
        let remainder = key_str
            .strip_prefix(UPLOADED_PREFIX)
            .ok_or_else(|| format!("Tracking key missing prefix: {key_str}"))?;
        let (schema_part, pseudo_part) = remainder
            .rsplit_once(':')
            .ok_or_else(|| format!("Malformed uploaded tracking key: {key_str}"))?;
        let pseudonym = uuid::Uuid::parse_str(pseudo_part)
            .map_err(|e| format!("Invalid pseudonym in tracking key {key_str}: {e}"))?;
        out.push((schema_part.to_string(), pseudonym));
    }
    Ok(out)
}

/// Delete tracking entries. When `schema` is `Some`, only that schema's
/// entries are deleted; when `None`, all tracking entries are deleted.
///
/// NOTE: this only clears the local tracking table — it does NOT delete
/// anything on the discovery lambda. Callers must send the lambda delete
/// request first, confirm success, THEN clear tracking.
pub async fn clear_uploaded(store: &dyn KvStore, schema: Option<&str>) -> Result<(), String> {
    let entries = list_uploaded_pseudonyms(store, schema).await?;
    for (schema_name, pseudonym) in entries {
        store
            .delete(&uploaded_key(&schema_name, &pseudonym))
            .await
            .map_err(|e| format!("Failed to delete uploaded tracking entry: {e}"))?;
    }
    Ok(())
}

/// Stored embedding entry — mirrors fold_db's StoredEmbedding.
/// Deserialized from Sled to read existing embeddings.
#[derive(Deserialize)]
struct StoredEmbedding {
    #[allow(dead_code)]
    pub schema: String,
    #[allow(dead_code)]
    pub key: fold_db::schema::types::key_value::KeyValue,
    #[serde(default)]
    #[allow(dead_code)]
    pub field_names: Vec<String>,
    #[serde(default)]
    pub field_name: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub fragment_idx: usize,
    #[serde(default)]
    pub fragment_text: Option<String>,
    pub embedding: Vec<f32>,
}

impl StoredEmbedding {
    /// Returns true if this is a legacy embedding (pre-fragmentation format).
    fn is_legacy(&self) -> bool {
        self.field_name.is_empty()
    }
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
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
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
        disabled_categories: &[String],
    ) -> Result<PublishResult, String> {
        // Skip schemas whose category has been disabled in interest profile
        if disabled_categories.iter().any(|c| c == &config.category) {
            return Ok(PublishResult {
                accepted: 0,
                quarantined: 0,
                total: 0,
                skipped: 0,
            });
        }

        let prefix = format!("{}{}:", EMB_PREFIX, config.schema_name);
        let raw_entries = embedding_store
            .scan_prefix(prefix.as_bytes())
            .await
            .map_err(|e| format!("Failed to scan embeddings: {}", e))?;

        let mut upload_entries = Vec::new();
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

            // Determine field name: new format uses field_name, legacy uses schema name
            let field_name = if stored.is_legacy() {
                config.schema_name.clone()
            } else {
                stored.field_name.clone()
            };

            // Skip face embeddings in the text loop — handled separately below
            if field_name == "__face__" {
                // Face embeddings are published in the face-specific pass below
                continue;
            }

            // Per `preferences/discovery-no-anonymity-gating`, client-side
            // anonymity gates (NER, token entropy, min-length) are disabled.
            // Only `FieldPrivacyClass::NeverPublish` — explicit user opt-out —
            // is honored here. All other fragments are accepted unconditionally.
            let privacy_class = config
                .field_privacy
                .get(&field_name)
                .copied()
                .unwrap_or_else(|| default_privacy_class(&field_name));

            if privacy_class == FieldPrivacyClass::NeverPublish {
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
                stored.fragment_text.clone()
            } else {
                None
            };

            let fragment_type = if stored.is_legacy() {
                "field".to_string()
            } else {
                "fragment".to_string()
            };

            let public_key = Some(connection::get_pseudonym_public_key_b64(
                &self.master_key,
                &pseudo,
            ));

            upload_entries.push(DiscoveryUploadEntry {
                pseudonym: pseudo,
                embedding: stored.embedding,
                category: config.category.clone(),
                content_preview: preview,
                fragment_type,
                public_key,
                embedding_space: "text".to_string(),
            });
        }

        // Second pass: face embeddings (only if publish_faces is enabled)
        if config.publish_faces {
            for (_key_bytes, value_bytes) in &raw_entries {
                let stored: StoredEmbedding = match serde_json::from_slice(value_bytes) {
                    Ok(s) => s,
                    Err(_) => continue,
                };

                if stored.embedding.is_empty() || stored.field_name != "__face__" {
                    continue;
                }

                // Face embeddings are published unconditionally (client-side anonymity gates are disabled).
                let embedding_bytes: Vec<u8> = stored
                    .embedding
                    .iter()
                    .flat_map(|f| f.to_le_bytes())
                    .collect();
                let content_hash = pseudonym::content_hash_bytes(&embedding_bytes);
                let pseudo = pseudonym::derive_pseudonym(&self.master_key, &content_hash);

                let public_key = Some(connection::get_pseudonym_public_key_b64(
                    &self.master_key,
                    &pseudo,
                ));

                upload_entries.push(DiscoveryUploadEntry {
                    pseudonym: pseudo,
                    embedding: stored.embedding,
                    category: config.category.clone(),
                    content_preview: None,
                    fragment_type: "face".to_string(),
                    public_key,
                    embedding_space: "face".to_string(),
                });
            }
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
        // Snapshot the pseudonyms we are about to upload so we can record
        // them as tracked after a successful upload. Opt-out uses this
        // tracking table as the authoritative enumeration of what the
        // lambda holds for this schema, independent of whether the local
        // `emb:` entries still exist.
        let uploaded_pseudonyms: Vec<uuid::Uuid> =
            upload_entries.iter().map(|e| e.pseudonym).collect();
        let request = DiscoveryUploadRequest {
            entries: upload_entries,
            owner_entries: Vec::new(), // no longer stored server-side (privacy)
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

        // Record tracking entries for every pseudonym that was accepted or
        // quarantined by the lambda. The lambda response only returns
        // counts, not a per-pseudonym acceptance list — since the non-2xx
        // path already errored out above, every pseudonym in the batch is
        // now either stored or held in quarantine on the lambda, and all
        // of them must be enumerable for opt-out.
        for pseudo in &uploaded_pseudonyms {
            record_uploaded(embedding_store, &config.schema_name, pseudo).await?;
        }

        Ok(PublishResult {
            accepted: upload_response.accepted,
            quarantined: upload_response.quarantined,
            total,
            skipped,
        })
    }

    /// Derive all pseudonyms for a schema's embeddings (for client-side opt-out).
    pub async fn derive_schema_pseudonyms(
        &self,
        embedding_store: &dyn KvStore,
        schema_name: &str,
    ) -> Result<Vec<uuid::Uuid>, String> {
        let prefix = format!("{}{}:", EMB_PREFIX, schema_name);
        let raw_entries = embedding_store
            .scan_prefix(prefix.as_bytes())
            .await
            .map_err(|e| format!("Failed to scan embeddings: {e}"))?;

        let mut pseudonyms = Vec::new();
        for (_key_bytes, value_bytes) in &raw_entries {
            let stored: StoredEmbedding = match serde_json::from_slice(value_bytes) {
                Ok(s) => s,
                Err(_) => continue,
            };
            if stored.embedding.is_empty() {
                continue;
            }
            let embedding_bytes: Vec<u8> = stored
                .embedding
                .iter()
                .flat_map(|f| f.to_le_bytes())
                .collect();
            let content_hash = pseudonym::content_hash_bytes(&embedding_bytes);
            pseudonyms.push(pseudonym::derive_pseudonym(&self.master_key, &content_hash));
        }
        Ok(pseudonyms)
    }

    /// Remove published records by pseudonym list (client-side enumeration).
    /// No server-side pseudonym-to-user mapping — client derives pseudonyms
    /// locally from master_key and sends the list directly.
    pub async fn unpublish_pseudonyms(&self, pseudonyms: Vec<uuid::Uuid>) -> Result<(), String> {
        let request = DiscoveryOptOutRequest { pseudonyms };

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
        offset: Option<usize>,
    ) -> Result<Vec<DiscoverySearchResult>, String> {
        self.search_with_threshold(
            query_embedding,
            top_k,
            category_filter,
            offset,
            None,
            "text".to_string(),
        )
        .await
    }

    pub async fn search_with_threshold(
        &self,
        query_embedding: Vec<f32>,
        top_k: usize,
        category_filter: Option<String>,
        offset: Option<usize>,
        similarity_threshold: Option<f32>,
        embedding_space: String,
    ) -> Result<Vec<DiscoverySearchResult>, String> {
        let request = DiscoverySearchRequest {
            embedding: query_embedding,
            top_k,
            category_filter,
            similarity_threshold,
            offset,
            embedding_space,
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

    /// Send an encrypted connection request to a pseudonym owner via the bulletin board.
    pub async fn connect(
        &self,
        target_pseudonym: Uuid,
        encrypted_blob: String,
        sender_pseudonym: Option<Uuid>,
    ) -> Result<(), String> {
        let request = DiscoveryConnectRequest {
            target_pseudonym,
            encrypted_blob,
            sender_pseudonym,
        };

        let response = self
            .http_client
            .post(format!("{}/messaging/connect", self.discovery_url))
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

    /// Poll for encrypted messages from the bulletin board, filtered by target pseudonyms.
    pub async fn poll_messages(
        &self,
        since: Option<&str>,
        target_pseudonyms: Option<&[Uuid]>,
    ) -> Result<Vec<EncryptedMessage>, String> {
        let mut url = format!("{}/messaging/messages", self.discovery_url);
        let mut params = Vec::new();
        if let Some(since_ts) = since {
            params.push(format!("since={}", since_ts));
        }
        if let Some(pseudonyms) = target_pseudonyms {
            let csv: String = pseudonyms
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join(",");
            params.push(format!("pseudonyms={}", csv));
        }
        if !params.is_empty() {
            url = format!("{}?{}", url, params.join("&"));
        }

        let response = self
            .http_client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.auth_token))
            .send()
            .await
            .map_err(|e| format!("Failed to poll messages: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!(
                "Poll messages failed with status {}: {}",
                status, body
            ));
        }

        let poll_response: EncryptedMessagesResponse = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse messages response: {}", e))?;

        Ok(poll_response.messages)
    }

    /// Browse categories on the discovery network.
    pub async fn browse_categories(&self) -> Result<Vec<BrowseCategory>, String> {
        let response = self
            .http_client
            .get(format!("{}/discover/browse/categories", self.discovery_url))
            .header("Authorization", format!("Bearer {}", self.auth_token))
            .send()
            .await
            .map_err(|e| format!("Failed to browse categories: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!(
                "Browse categories failed with status {}: {}",
                status, body
            ));
        }

        let browse_response: BrowseCategoriesResponse = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse browse response: {}", e))?;

        Ok(browse_response.categories)
    }

    /// Look up the X25519 public key for a target pseudonym.
    pub async fn get_public_key(&self, pseudonym: &Uuid) -> Result<Option<String>, String> {
        let url = format!(
            "{}/discover/public-key?pseudonym={}",
            self.discovery_url, pseudonym
        );

        let response = self
            .http_client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.auth_token))
            .send()
            .await
            .map_err(|e| format!("Failed to get public key: {}", e))?;

        if response.status().as_u16() == 404 {
            return Ok(None);
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!(
                "Public key lookup failed with status {}: {}",
                status, body
            ));
        }

        let pk_response: PublicKeyResponse = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse public key response: {}", e))?;

        Ok(Some(pk_response.public_key))
    }

    /// Legacy: Poll for incoming connection requests.
    pub async fn poll_requests(&self) -> Result<Vec<IncomingConnectionRequest>, String> {
        let response = self
            .http_client
            .get(format!("{}/messaging/requests", self.discovery_url))
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

    // ===== Trust Invite Relay =====

    /// Upload a trust invite token to the discovery service, returning a short invite ID.
    pub async fn store_trust_invite(&self, token: &str) -> Result<String, String> {
        let response = self
            .http_client
            .post(format!("{}/messaging/trust-invite", self.discovery_url))
            .header("Authorization", format!("Bearer {}", self.auth_token))
            .json(&serde_json::json!({ "token": token }))
            .send()
            .await
            .map_err(|e| format!("Failed to store trust invite: {e}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("Store trust invite failed ({status}): {body}"));
        }

        let resp: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {e}"))?;

        resp.get("invite_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "Response missing invite_id".to_string())
    }

    /// Fetch a trust invite token from the discovery service by ID. One-time use.
    pub async fn fetch_trust_invite(&self, invite_id: &str) -> Result<String, String> {
        let url = format!(
            "{}/messaging/trust-invite?id={}",
            self.discovery_url, invite_id
        );
        let response = self
            .http_client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.auth_token))
            .send()
            .await
            .map_err(|e| format!("Failed to fetch trust invite: {e}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("Fetch trust invite failed ({status}): {body}"));
        }

        let resp: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {e}"))?;

        resp.get("token")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "Response missing token".to_string())
    }

    /// Send an email-verified trust invite. Emails a verification code to the recipient.
    pub async fn send_verified_invite(
        &self,
        token: &str,
        recipient_email: &str,
        sender_name: &str,
    ) -> Result<String, String> {
        let response = self
            .http_client
            .post(format!(
                "{}/messaging/trust-invite/send",
                self.discovery_url
            ))
            .header("Authorization", format!("Bearer {}", self.auth_token))
            .json(&serde_json::json!({
                "token": token,
                "recipient_email": recipient_email,
                "sender_name": sender_name,
            }))
            .send()
            .await
            .map_err(|e| format!("Failed to send verified invite: {e}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("Send verified invite failed ({status}): {body}"));
        }

        let resp: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {e}"))?;

        resp.get("invite_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "Response missing invite_id".to_string())
    }

    /// Verify a code and fetch the trust invite token.
    pub async fn verify_invite_code(&self, invite_id: &str, code: &str) -> Result<String, String> {
        let response = self
            .http_client
            .post(format!(
                "{}/messaging/trust-invite/verify",
                self.discovery_url
            ))
            .header("Authorization", format!("Bearer {}", self.auth_token))
            .json(&serde_json::json!({
                "invite_id": invite_id,
                "code": code,
            }))
            .send()
            .await
            .map_err(|e| format!("Failed to verify invite code: {e}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("Verify invite code failed ({status}): {body}"));
        }

        let resp: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {e}"))?;

        resp.get("token")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "Response missing token".to_string())
    }
}

#[cfg(test)]
mod uploaded_tracking_tests {
    use super::*;
    use fold_db::storage::inmemory_backend::InMemoryKvStore;
    use uuid::Uuid;

    fn uuid(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[tokio::test]
    async fn record_and_list_round_trip() {
        let store = InMemoryKvStore::new();
        let kv: &dyn KvStore = &store;

        let p1 = uuid(1);
        let p2 = uuid(2);
        record_uploaded(kv, "Recipe", &p1).await.unwrap();
        record_uploaded(kv, "Recipe", &p2).await.unwrap();

        let mut listed = list_uploaded_pseudonyms(kv, Some("Recipe")).await.unwrap();
        listed.sort_by_key(|(_, p)| *p);
        assert_eq!(
            listed,
            vec![("Recipe".to_string(), p1), ("Recipe".to_string(), p2)]
        );
    }

    #[tokio::test]
    async fn list_filters_by_schema() {
        let store = InMemoryKvStore::new();
        let kv: &dyn KvStore = &store;

        record_uploaded(kv, "Recipe", &uuid(1)).await.unwrap();
        record_uploaded(kv, "Recipe", &uuid(2)).await.unwrap();
        record_uploaded(kv, "Note", &uuid(3)).await.unwrap();

        let recipe = list_uploaded_pseudonyms(kv, Some("Recipe")).await.unwrap();
        assert_eq!(recipe.len(), 2);
        assert!(recipe.iter().all(|(s, _)| s == "Recipe"));

        let note = list_uploaded_pseudonyms(kv, Some("Note")).await.unwrap();
        assert_eq!(note.len(), 1);
        assert_eq!(note[0].0, "Note");
    }

    #[tokio::test]
    async fn list_none_returns_all() {
        let store = InMemoryKvStore::new();
        let kv: &dyn KvStore = &store;

        record_uploaded(kv, "Recipe", &uuid(1)).await.unwrap();
        record_uploaded(kv, "Note", &uuid(2)).await.unwrap();
        record_uploaded(kv, "Photo", &uuid(3)).await.unwrap();

        let all = list_uploaded_pseudonyms(kv, None).await.unwrap();
        assert_eq!(all.len(), 3);
    }

    #[tokio::test]
    async fn clear_single_schema_only_touches_that_schema() {
        let store = InMemoryKvStore::new();
        let kv: &dyn KvStore = &store;

        record_uploaded(kv, "Recipe", &uuid(1)).await.unwrap();
        record_uploaded(kv, "Recipe", &uuid(2)).await.unwrap();
        record_uploaded(kv, "Note", &uuid(3)).await.unwrap();

        clear_uploaded(kv, Some("Recipe")).await.unwrap();

        let recipe = list_uploaded_pseudonyms(kv, Some("Recipe")).await.unwrap();
        assert!(recipe.is_empty());
        let note = list_uploaded_pseudonyms(kv, Some("Note")).await.unwrap();
        assert_eq!(note.len(), 1);
    }

    #[tokio::test]
    async fn clear_all_removes_everything() {
        let store = InMemoryKvStore::new();
        let kv: &dyn KvStore = &store;

        record_uploaded(kv, "Recipe", &uuid(1)).await.unwrap();
        record_uploaded(kv, "Note", &uuid(2)).await.unwrap();

        clear_uploaded(kv, None).await.unwrap();

        let all = list_uploaded_pseudonyms(kv, None).await.unwrap();
        assert!(all.is_empty());
    }

    /// Zombie-embedding regression: after publishing, deleting the source
    /// embedding locally must NOT remove the tracking entry — otherwise
    /// opt-out would miss it and leave data on the lambda.
    #[tokio::test]
    async fn tracking_survives_source_emb_deletion() {
        let store = InMemoryKvStore::new();
        let kv: &dyn KvStore = &store;

        // Simulate a published embedding: write emb:{schema}:{key}
        // AND a tracking entry.
        let schema = "Recipe";
        let pseudo = uuid(42);
        kv.put(b"emb:Recipe:some-key", b"{}".to_vec())
            .await
            .unwrap();
        record_uploaded(kv, schema, &pseudo).await.unwrap();

        // User deletes the source embedding locally.
        kv.delete(b"emb:Recipe:some-key").await.unwrap();

        // Tracking must still enumerate the uploaded pseudonym.
        let tracked = list_uploaded_pseudonyms(kv, Some(schema)).await.unwrap();
        assert_eq!(tracked, vec![(schema.to_string(), pseudo)]);
    }

    /// Fallback path: publisher's `derive_schema_pseudonyms` still works
    /// from live `emb:` entries when tracking is empty (pre-existing users).
    #[tokio::test]
    async fn fallback_derive_works_when_tracking_empty() {
        let store = InMemoryKvStore::new();
        let kv: &dyn KvStore = &store;

        let stored = serde_json::json!({
            "schema": "Recipe",
            "key": { "hash": "rec-1", "range": null },
            "field_names": ["title"],
            "field_name": "title",
            "fragment_idx": 0usize,
            "fragment_text": "hello world",
            "embedding": vec![0.1f32, 0.2, 0.3, 0.4],
        });
        kv.put(b"emb:Recipe:rec-1", serde_json::to_vec(&stored).unwrap())
            .await
            .unwrap();

        // Tracking table is empty.
        let tracked = list_uploaded_pseudonyms(kv, Some("Recipe")).await.unwrap();
        assert!(tracked.is_empty());

        // Fallback derive-from-live still produces a pseudonym.
        let publisher = DiscoveryPublisher::new(
            vec![1, 2, 3, 4],
            "http://mock".to_string(),
            "tok".to_string(),
        );
        let derived = publisher
            .derive_schema_pseudonyms(kv, "Recipe")
            .await
            .unwrap();
        assert_eq!(derived.len(), 1);
    }
}
