//! Ollama auto-setup: detect installation, start server, and pull missing models.
//!
//! Called on binary startup to ensure all required Ollama models are available.
//! Skips instantly when all models are already present.

use std::process::Command;
use std::time::Duration;

/// A model required by FoldDB.
struct RequiredModel {
    name: String,
    purpose: &'static str,
}

/// Check Ollama readiness and pull any missing models.
///
/// This runs synchronously at startup (before the async runtime matters for
/// model pulls). Returns `Ok(())` if all models are ready, or an `Err` with
/// a user-facing message if setup cannot complete.
pub fn ensure_ollama_ready() -> Result<(), String> {
    let config = super::IngestionConfig::load_or_default();

    // Only relevant when provider is Ollama
    if config.provider != super::config::AIProvider::Ollama {
        return Ok(());
    }

    // 1. Check Ollama is installed
    if !is_ollama_installed() {
        return Err(
            "Ollama is not installed. It's required for local AI features.\n\
             Install it from: https://ollama.com/download\n\
             Alternatively, switch to Anthropic in Settings."
                .to_string(),
        );
    }

    // 2. Ensure Ollama server is running
    ensure_ollama_running(&config.ollama.base_url)?;

    // 3. Check and pull missing models
    let required = vec![
        RequiredModel {
            name: config.ollama.model.clone(),
            purpose: "Text ingestion and queries",
        },
        RequiredModel {
            name: config.ollama.vision_model.clone(),
            purpose: "Image captioning and classification",
        },
        RequiredModel {
            name: config.ollama.ocr_model.clone(),
            purpose: "OCR text extraction from documents",
        },
    ];

    let installed = list_installed_models();
    let missing: Vec<&RequiredModel> = required
        .iter()
        .filter(|m| !installed.iter().any(|i| i == &m.name))
        .collect();

    if missing.is_empty() {
        println!("  Ollama: all {} required models present.", required.len());
        return Ok(());
    }

    println!();
    println!("==========================================");
    println!("  First-Time AI Model Setup");
    println!("==========================================");
    println!();
    println!(
        "FoldDB needs {} AI model(s) downloaded to your machine.",
        missing.len()
    );
    println!("This is a one-time setup. Models to download:");
    println!();
    for m in &missing {
        println!("  - {} — {}", m.name, m.purpose);
    }
    println!();
    println!("Models are cached locally — future starts will skip this step.");
    println!();

    let total = missing.len();
    for (i, m) in missing.iter().enumerate() {
        println!("------------------------------------------");
        println!("  [{}/{}] Pulling {}", i + 1, total, m.name);
        println!("  Purpose: {}", m.purpose);
        println!("------------------------------------------");

        pull_model(&m.name)?;

        println!("  Done: {}", m.name);
        println!();
    }

    println!("==========================================");
    println!("  All AI models ready!");
    println!("==========================================");
    println!();

    Ok(())
}

fn is_ollama_installed() -> bool {
    Command::new("ollama")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn ensure_ollama_running(base_url: &str) -> Result<(), String> {
    let tags_url = format!("{}/api/tags", base_url);

    // Quick check — is it already running?
    if check_ollama_health(&tags_url) {
        return Ok(());
    }

    println!("  Ollama is installed but not running. Starting it...");

    // Try to start ollama serve in the background
    Command::new("ollama")
        .arg("serve")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to start Ollama: {}", e))?;

    // Wait up to 10 seconds for it to become healthy
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(500));
        if check_ollama_health(&tags_url) {
            println!("  Ollama server started.");
            return Ok(());
        }
    }

    Err(format!(
        "Ollama failed to start within 10 seconds.\n\
         Please run 'ollama serve' manually, then restart FoldDB."
    ))
}

fn check_ollama_health(tags_url: &str) -> bool {
    // Use std::net::TcpStream instead of reqwest::blocking::Client to avoid
    // creating a nested tokio runtime (which panics inside an async context).
    // Parse host:port from URL like "http://192.168.1.195:11434/api/tags"
    let stripped = tags_url
        .strip_prefix("http://")
        .or_else(|| tags_url.strip_prefix("https://"))
        .unwrap_or(tags_url);
    let (host_port, path) = stripped.split_once('/').unwrap_or((stripped, ""));
    let path = format!("/{path}");
    let (host, port) = if let Some((h, p)) = host_port.rsplit_once(':') {
        (h, p.parse::<u16>().unwrap_or(11434))
    } else {
        (host_port, 11434u16)
    };

    let addr: std::net::SocketAddr = format!("{host}:{port}")
        .parse()
        .unwrap_or_else(|_| std::net::SocketAddr::from(([127, 0, 0, 1], port)));

    let stream = match std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(2)) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));

    use std::io::{Read, Write};
    let request = format!("GET {path} HTTP/1.0\r\nHost: {host}\r\n\r\n");
    let mut s = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return false,
    };
    if s.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut buf = [0u8; 32];
    let n = stream.take(32).read(&mut buf).unwrap_or(0);
    let response = String::from_utf8_lossy(&buf[..n]);
    response.starts_with("HTTP/1.") && response.contains("200")
}

fn list_installed_models() -> Vec<String> {
    Command::new("ollama")
        .arg("list")
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .skip(1) // header row
                .filter_map(|line| line.split_whitespace().next().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn pull_model(name: &str) -> Result<(), String> {
    let status = Command::new("ollama")
        .args(["pull", name])
        .status()
        .map_err(|e| format!("Failed to run 'ollama pull {}': {}", name, e))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "Failed to pull model '{}'. Check your internet connection.\n\
             You can also pull it manually: ollama pull {}",
            name, name
        ))
    }
}
