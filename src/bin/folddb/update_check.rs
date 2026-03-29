//! Background update checker for the FoldDB CLI.
//!
//! On startup, spawns a task that queries the GitHub releases API for the
//! latest version of fold_db_node. If a newer version is available, prints
//! a one-line notice to stderr so it doesn't interfere with JSON output.

use std::time::Duration;

const GITHUB_RELEASES_URL: &str =
    "https://api.github.com/repos/shiba4life/fold_db_node/releases/latest";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const CHECK_TIMEOUT: Duration = Duration::from_secs(3);

/// Spawns a background task that checks for updates.
/// Fire-and-forget: prints to stderr if a newer version exists.
pub fn spawn_update_check() {
    tokio::spawn(async {
        let result = check_latest_version().await;
        if let Ok(Some(latest)) = result {
            if version_is_newer(&latest, CURRENT_VERSION) {
                eprintln!(
                    "\n  Update available: {} -> {} — run `brew upgrade folddb` to update\n",
                    CURRENT_VERSION, latest
                );
            }
        }
    });
}

async fn check_latest_version() -> Result<Option<String>, ()> {
    let client = reqwest::Client::builder()
        .timeout(CHECK_TIMEOUT)
        .user_agent("folddb-cli")
        .build()
        .map_err(|_| ())?;

    let resp = client
        .get(GITHUB_RELEASES_URL)
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await
        .map_err(|_| ())?;

    if !resp.status().is_success() {
        return Ok(None);
    }

    let body: serde_json::Value = resp.json().await.map_err(|_| ())?;
    let tag = body["tag_name"].as_str().unwrap_or("");
    let version = tag.strip_prefix('v').unwrap_or(tag);

    if version.is_empty() {
        return Ok(None);
    }

    Ok(Some(version.to_string()))
}

/// Simple semver comparison: returns true if `latest` > `current`.
fn version_is_newer(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.split('.')
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect()
    };
    let l = parse(latest);
    let c = parse(current);
    l > c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_version_detected() {
        assert!(version_is_newer("1.2.0", "1.1.0"));
        assert!(version_is_newer("0.2.0", "0.1.14"));
        assert!(version_is_newer("0.1.15", "0.1.14"));
    }

    #[test]
    fn same_or_older_version() {
        assert!(!version_is_newer("0.1.14", "0.1.14"));
        assert!(!version_is_newer("0.1.13", "0.1.14"));
        assert!(!version_is_newer("0.0.1", "0.1.0"));
    }

    #[test]
    fn version_parse_edge_cases() {
        assert!(version_is_newer("1.0.0", "0.99.99"));
        assert!(!version_is_newer("", "0.1.0"));
    }
}
