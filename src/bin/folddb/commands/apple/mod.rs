pub mod notes;
pub mod photos;
pub mod reminders;

use crate::error::CliError;
use reqwest::Client;
use serde_json::Value;
use sha2::{Digest, Sha256};

const DEFAULT_BASE_URL: &str = "http://localhost:9001";

fn build_client(user_hash: &str) -> Result<(Client, String), CliError> {
    let base_url = std::env::var("FOLD_NODE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .default_headers({
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(
                "x-user-hash",
                reqwest::header::HeaderValue::from_str(user_hash)
                    .map_err(|e| CliError::new(format!("Invalid user hash header: {}", e)))?,
            );
            headers
        })
        .build()
        .map_err(|e| CliError::new(format!("Failed to build HTTP client: {}", e)))?;
    Ok((client, base_url))
}

fn content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let result = hasher.finalize();
    hex::encode(&result[..8])
}

async fn post_ingestion_batch(
    client: &Client,
    base_url: &str,
    records: Vec<Value>,
) -> Result<Vec<String>, CliError> {
    let record_count = records.len();
    let payload = serde_json::json!({
        "data": records,
        "auto_execute": true,
        "pub_key": "default",
    });

    let resp = client
        .post(format!("{}/api/ingestion/process", base_url))
        .json(&payload)
        .send()
        .await
        .map_err(|e| {
            CliError::new(format!("Failed to reach local node: {}", e))
                .with_hint("Is the node running? Start it with: ./run.sh --local")
        })?;

    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .map_err(|e| CliError::new(format!("Invalid response from node: {}", e)))?;

    if !status.is_success() {
        let msg = body["error"]
            .as_str()
            .unwrap_or("Unknown error from ingestion API");
        return Err(CliError::new(format!("Ingestion failed ({}): {}", status, msg)));
    }

    // The ingestion API is async — it returns a progress_id for tracking.
    // Poll until completion so the CLI can report accurate counts.
    let progress_id = match body["progress_id"].as_str() {
        Some(id) => id.to_string(),
        None => {
            return Err(CliError::new(
                "Ingestion response missing progress_id".to_string(),
            ));
        }
    };

    let poll_url = format!(
        "{}/api/ingestion/progress?progress_id={}",
        base_url, progress_id
    );
    let poll_interval = std::time::Duration::from_secs(2);
    let timeout = std::time::Duration::from_secs(300);
    let start = std::time::Instant::now();

    loop {
        if start.elapsed() > timeout {
            return Err(CliError::new(format!(
                "Ingestion timed out after {}s for batch of {} records",
                timeout.as_secs(),
                record_count
            )));
        }

        tokio::time::sleep(poll_interval).await;

        let poll_resp = client
            .get(&poll_url)
            .send()
            .await
            .map_err(|e| CliError::new(format!("Failed to poll progress: {}", e)))?;

        let poll_body: Value = poll_resp
            .json()
            .await
            .map_err(|e| CliError::new(format!("Invalid progress response: {}", e)))?;

        let progress = match poll_body["progress"].as_array().and_then(|a| a.first()) {
            Some(p) => p,
            None => continue,
        };

        if progress["is_failed"].as_bool() == Some(true) {
            let err_msg = progress["error_message"]
                .as_str()
                .unwrap_or("Unknown ingestion error");
            return Err(CliError::new(format!("Ingestion failed: {}", err_msg)));
        }

        if progress["is_complete"].as_bool() == Some(true) {
            let mutations_executed = progress["results"]["mutations_executed"]
                .as_u64()
                .unwrap_or(0) as usize;
            // Return one ID per mutation executed so the caller can count them
            let ids: Vec<String> = (0..mutations_executed)
                .map(|i| format!("{}:{}", progress_id, i))
                .collect();
            return Ok(ids);
        }
    }
}

fn run_osascript(script: &str) -> Result<String, CliError> {
    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|e| CliError::new(format!("Failed to run osascript: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::new(format!("AppleScript error: {}", stderr)));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
