//! folddb-test-actions — complex E2E test actions not easily expressed in bash.
//!
//! v1 subcommands:
//!   - poll-until-match: poll /api/discovery/connection-requests until pending >= N
//!   - noop: prints `{"ok": true}`

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use std::thread::sleep;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(name = "folddb-test-actions")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Poll connection requests until pending count >= expect_pending or timeout.
    PollUntilMatch {
        #[arg(long)]
        port: u16,
        #[arg(long)]
        hash: String,
        #[arg(long)]
        expect_pending: usize,
        #[arg(long, default_value = "60")]
        timeout_seconds: u64,
    },
    /// No-op; prints `{"ok": true}`.
    Noop,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Noop => {
            println!("{{\"ok\": true}}");
            Ok(())
        }
        Cmd::PollUntilMatch {
            port,
            hash,
            expect_pending,
            timeout_seconds,
        } => poll_until_match(port, &hash, expect_pending, timeout_seconds),
    }
}

fn poll_until_match(
    port: u16,
    hash: &str,
    expect_pending: usize,
    timeout_seconds: u64,
) -> Result<()> {
    let url = format!("http://127.0.0.1:{port}/api/discovery/connection-requests");
    let client = reqwest::blocking::Client::new();
    let deadline = Instant::now() + Duration::from_secs(timeout_seconds);

    while Instant::now() < deadline {
        let resp = client
            .get(&url)
            .header("X-User-Hash", hash)
            .send();

        if let Ok(r) = resp {
            if r.status().is_success() {
                let body: serde_json::Value = r.json().unwrap_or(serde_json::Value::Null);
                let pending = body
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter(|e| e.get("status").and_then(|s| s.as_str()) == Some("pending"))
                            .count()
                    })
                    .unwrap_or(0);
                if pending >= expect_pending {
                    println!("{}", serde_json::to_string(&body)?);
                    return Ok(());
                }
            }
        }
        sleep(Duration::from_millis(500));
    }

    Err(anyhow!(
        "timeout: did not reach {} pending requests in {}s",
        expect_pending,
        timeout_seconds
    ))
}
