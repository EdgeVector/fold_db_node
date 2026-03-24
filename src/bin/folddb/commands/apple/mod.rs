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

    let mut ids = Vec::new();
    if let Some(arr) = body["mutations_executed"].as_array() {
        for item in arr {
            if let Some(id) = item["id"].as_str() {
                ids.push(id.to_string());
            }
        }
    }
    Ok(ids)
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
