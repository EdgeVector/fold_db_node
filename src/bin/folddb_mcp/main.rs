mod client;
mod error;
mod protocol;
mod tools;

use std::io::BufRead;

use clap::Parser;

/// FoldDB MCP Server — exposes FoldDB tools to AI agents via the Model Context Protocol.
///
/// Connects to a running FoldDB HTTP server on localhost and handles Ed25519 signing
/// transparently. Communicates via JSON-RPC 2.0 over stdin/stdout.
#[derive(Parser, Debug)]
#[command(name = "folddb-mcp", version = env!("FOLDDB_BUILD_VERSION"), about)]
struct Args {
    /// Port of the FoldDB HTTP server (also reads FOLDDB_PORT env var)
    #[arg(long, default_value = "9001")]
    port: u16,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let port = std::env::var("FOLDDB_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(args.port);

    let client = match client::FoldDbClient::connect(port).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[folddb-mcp] Fatal: {}", e);
            std::process::exit(1);
        }
    };

    eprintln!("[folddb-mcp] Ready, listening on stdin");

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[folddb-mcp] stdin read error: {}", e);
                break;
            }
        };

        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let request: protocol::JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let resp = protocol::JsonRpcResponse::error(
                    None,
                    error::PARSE_ERROR,
                    format!("Invalid JSON: {}", e),
                );
                println!("{}", serde_json::to_string(&resp).unwrap());
                continue;
            }
        };

        if let Some(response) = protocol::route(request, &client).await {
            println!("{}", serde_json::to_string(&response).unwrap());
        }
    }
}
