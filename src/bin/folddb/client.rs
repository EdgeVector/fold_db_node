//! HTTP client for communicating with the folddb daemon.
//!
//! All CLI data commands go through this client instead of accessing Sled directly.
//! The daemon owns the Sled database exclusively.

use crate::error::CliError;
use serde_json::Value;

pub struct FoldDbClient {
    base_url: String,
    user_hash: String,
    client: reqwest::Client,
}

impl FoldDbClient {
    pub fn new(port: u16, user_hash: &str) -> Self {
        Self {
            base_url: format!("http://127.0.0.1:{}", port),
            user_hash: user_hash.to_string(),
            // 10 min timeout — LLM agent queries and large ingestion can be slow
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(600))
                .build()
                .unwrap(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    async fn get(&self, path: &str) -> Result<Value, CliError> {
        let resp = self
            .client
            .get(self.url(path))
            .header("X-User-Hash", &self.user_hash)
            .send()
            .await
            .map_err(|e| {
                if e.is_connect() {
                    CliError::new("Daemon not responding")
                        .with_hint("Run `folddb daemon start` first")
                } else {
                    CliError::new(format!("HTTP request failed: {}", e))
                }
            })?;

        self.parse_response(resp).await
    }

    async fn post(&self, path: &str, body: &Value) -> Result<Value, CliError> {
        let resp = self
            .client
            .post(self.url(path))
            .header("X-User-Hash", &self.user_hash)
            .json(body)
            .send()
            .await
            .map_err(|e| {
                if e.is_connect() {
                    CliError::new("Daemon not responding")
                        .with_hint("Run `folddb daemon start` first")
                } else {
                    CliError::new(format!("HTTP request failed: {}", e))
                }
            })?;

        self.parse_response(resp).await
    }

    async fn parse_response(&self, resp: reqwest::Response) -> Result<Value, CliError> {
        let status = resp.status();
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body = resp
            .text()
            .await
            .map_err(|e| CliError::new(format!("Failed to read response: {}", e)))?;

        // Detect non-JSON responses (HTML error pages from proxy/crash)
        if !content_type.contains("json") && body.trim_start().starts_with('<') {
            let msg = if status.is_success() {
                "Daemon returned HTML instead of JSON (possible misconfiguration)"
            } else {
                "Daemon returned an error page"
            };
            return Err(CliError::new(format!("{} (HTTP {})", msg, status))
                .with_hint("Check daemon logs: ~/.folddb/server.log"));
        }

        let json: Value =
            serde_json::from_str(&body).unwrap_or_else(|_| serde_json::json!({ "error": body }));

        if !status.is_success() {
            let msg = json
                .get("error")
                .and_then(|v| v.as_str())
                .or_else(|| json.get("message").and_then(|v| v.as_str()))
                .unwrap_or("Unknown error");
            return Err(CliError::new(format!("Server error ({}): {}", status, msg)));
        }

        Ok(json)
    }

    // --- Schema commands ---

    pub async fn schema_list(&self) -> Result<Value, CliError> {
        self.get("/api/schemas").await
    }

    pub async fn schema_get(&self, name: &str) -> Result<Value, CliError> {
        self.get(&format!("/api/schema/{}", name)).await
    }

    pub async fn schema_approve(&self, name: &str) -> Result<Value, CliError> {
        self.post(
            &format!("/api/schema/{}/approve", name),
            &serde_json::json!({}),
        )
        .await
    }

    pub async fn schema_block(&self, name: &str) -> Result<Value, CliError> {
        self.post(
            &format!("/api/schema/{}/block", name),
            &serde_json::json!({}),
        )
        .await
    }

    pub async fn schema_load(&self) -> Result<Value, CliError> {
        self.post("/api/schemas/load", &serde_json::json!({})).await
    }

    // --- Query ---

    pub async fn query(
        &self,
        schema: &str,
        fields: &[String],
        hash: Option<&str>,
        range: Option<&str>,
    ) -> Result<Value, CliError> {
        let mut body = serde_json::json!({
            "schema_name": schema,
            "fields": fields,
        });
        if let Some(h) = hash {
            body["filter"] = serde_json::json!({ "hash_key": h });
        }
        if let Some(r) = range {
            if let Some(filter) = body.get_mut("filter") {
                filter["range_key"] = serde_json::json!(r);
            } else {
                body["filter"] = serde_json::json!({ "range_key": r });
            }
        }
        self.post("/api/query", &body).await
    }

    // --- Search ---

    pub async fn search(&self, term: &str) -> Result<Value, CliError> {
        self.get(&format!(
            "/api/native-index/search?term={}",
            urlencoding::encode(term)
        ))
        .await
    }

    // --- Mutations ---

    pub async fn mutate(
        &self,
        schema: &str,
        mutation_type: &str,
        fields: &Value,
        hash: Option<&str>,
        range: Option<&str>,
    ) -> Result<Value, CliError> {
        let mut body = serde_json::json!({
            "schema_name": schema,
            "mutation_type": mutation_type,
            "fields_and_values": fields,
        });
        if let Some(h) = hash {
            body["key_value"] = serde_json::json!({ "hash_key": h });
        }
        if let Some(r) = range {
            if let Some(kv) = body.get_mut("key_value") {
                kv["range_key"] = serde_json::json!(r);
            } else {
                body["key_value"] = serde_json::json!({ "range_key": r });
            }
        }
        self.post("/api/mutation", &body).await
    }

    pub async fn mutate_batch(&self, mutations: &Value) -> Result<Value, CliError> {
        self.post("/api/mutations/batch", mutations).await
    }

    // --- Ingestion ---

    pub async fn ingest_json(&self, data: &Value) -> Result<Value, CliError> {
        self.post("/api/ingest", data).await
    }

    /// Process records through the ingestion pipeline (used by Apple ingestion).
    #[cfg(target_os = "macos")]
    pub async fn ingest_process(&self, records: &[Value]) -> Result<Value, CliError> {
        let payload = serde_json::json!({
            "data": records,
            "auto_execute": true,
            "pub_key": "default",
        });
        self.post("/api/ingestion/process", &payload).await
    }

    pub async fn smart_scan(
        &self,
        path: &str,
        max_depth: usize,
        max_files: usize,
    ) -> Result<Value, CliError> {
        self.post(
            "/api/ingestion/smart-folder/scan",
            &serde_json::json!({
                "path": path,
                "max_depth": max_depth,
                "max_files": max_files,
            }),
        )
        .await
    }

    pub async fn smart_ingest(&self, path: &str, auto_execute: bool) -> Result<Value, CliError> {
        self.post(
            "/api/ingestion/smart-folder/ingest",
            &serde_json::json!({
                "path": path,
                "auto_execute": auto_execute,
            }),
        )
        .await
    }

    // --- LLM Ask ---

    pub async fn ask(&self, query: &str, max_iterations: usize) -> Result<Value, CliError> {
        self.post(
            "/api/llm-query/agent",
            &serde_json::json!({
                "query": query,
                "max_iterations": max_iterations,
            }),
        )
        .await
    }

    // --- Organizations ---

    pub async fn org_list(&self) -> Result<Value, CliError> {
        self.get("/api/org").await
    }

    pub async fn org_create(&self, name: &str) -> Result<Value, CliError> {
        self.post("/api/org", &serde_json::json!({ "name": name }))
            .await
    }

    pub async fn org_pending_invites(&self) -> Result<Value, CliError> {
        self.get("/api/org/invites/pending").await
    }

    pub async fn org_join(&self, invite_bundle: &Value) -> Result<Value, CliError> {
        self.post("/api/org/join", invite_bundle).await
    }

    // --- Discovery ---

    pub async fn discovery_opt_ins(&self) -> Result<Value, CliError> {
        self.get("/api/discovery/opt-ins").await
    }

    pub async fn discovery_interests(&self) -> Result<Value, CliError> {
        self.get("/api/discovery/interests").await
    }

    pub async fn discovery_publish(&self) -> Result<Value, CliError> {
        self.post("/api/discovery/publish", &serde_json::json!({}))
            .await
    }

    // --- Setup ---

    pub async fn apply_setup(&self, storage: &Value) -> Result<Value, CliError> {
        self.post(
            "/api/system/setup",
            &serde_json::json!({ "storage": storage }),
        )
        .await
    }

    // --- Sync ---

    pub async fn sync_status(&self) -> Result<Value, CliError> {
        self.get("/api/sync/status").await
    }

    pub async fn sync_trigger(&self) -> Result<Value, CliError> {
        self.post("/api/sync/trigger", &serde_json::json!({})).await
    }

    // --- System ---

    pub async fn status(&self) -> Result<Value, CliError> {
        self.get("/api/system/status").await
    }

    pub async fn database_config(&self) -> Result<Value, CliError> {
        self.get("/api/system/database-config").await
    }

    pub async fn reset(&self) -> Result<Value, CliError> {
        self.post(
            "/api/system/reset-database",
            &serde_json::json!({
                "confirm": true,
                "user_hash": self.user_hash,
            }),
        )
        .await
    }
}
