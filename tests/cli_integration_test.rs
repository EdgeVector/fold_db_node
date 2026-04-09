//! End-to-end integration tests for the `folddb` CLI binary.
//!
//! Tests start a real `folddb_server` daemon on a random port, then run CLI
//! commands against it via the `folddb` binary with `FOLDDB_PORT` set.
//!
//! Run with:
//!   cargo test --test cli_integration_test -- --nocapture
#![allow(deprecated)]

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::OnceLock;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Shared daemon — started once for all tests
// ---------------------------------------------------------------------------

static DAEMON: OnceLock<DaemonFixture> = OnceLock::new();

struct DaemonFixture {
    port: u16,
    _tmpdir: TempDir,
    config_path: PathBuf,
    server_process: Child,
}

impl DaemonFixture {
    fn start() -> Self {
        let tmpdir = TempDir::new().expect("create temp dir");
        let keypair = fold_db::security::Ed25519KeyPair::generate().expect("generate keypair");

        let db_path = tmpdir.path().join("db");
        let config = serde_json::json!({
            "database": {
                "path": db_path.to_str().unwrap()
            },
            "network_listen_address": "/ip4/0.0.0.0/tcp/0",
            "security_config": {
                "require_tls": false,
                "encrypt_at_rest": false
            },
            "schema_service_url": "test://mock",
            "public_key": keypair.public_key_base64(),
            "private_key": keypair.secret_key_base64()
        });

        let config_path = tmpdir.path().join("node_config.json");
        fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap())
            .expect("write config");

        // Write identity file so CLI doesn't trigger setup wizard
        let identity_dir = tmpdir.path().join("config");
        fs::create_dir_all(&identity_dir).expect("create identity dir");
        let identity = serde_json::json!({
            "private_key": keypair.secret_key_base64(),
            "public_key": keypair.public_key_base64(),
        });
        fs::write(
            identity_dir.join("node_identity.json"),
            serde_json::to_string_pretty(&identity).unwrap(),
        )
        .expect("write identity");

        // Find a random available port
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind random port");
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        // Start folddb_server
        let server_bin = assert_cmd::cargo::cargo_bin("folddb_server");
        let log_path = tmpdir.path().join("server.log");

        #[allow(clippy::zombie_processes)] // Managed in Drop impl
        let server_process = std::process::Command::new(server_bin)
            .arg("--port")
            .arg(port.to_string())
            .env("NODE_CONFIG", &config_path)
            .env("FOLDDB_HOME", tmpdir.path())
            .stdout(Stdio::from(
                fs::File::create(&log_path).expect("create log"),
            ))
            .stderr(Stdio::from(
                fs::File::create(tmpdir.path().join("server_err.log")).expect("create err log"),
            ))
            .spawn()
            .expect("start folddb_server");

        // Wait for health
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .unwrap();
        let health_url = format!("http://127.0.0.1:{}/api/system/status", port);

        for i in 0..30 {
            if client.get(&health_url).send().is_ok() {
                eprintln!("Daemon healthy on port {} (took {}s)", port, i + 1);
                return Self {
                    port,
                    _tmpdir: tmpdir,
                    config_path,
                    server_process,
                };
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }

        // Read logs on failure
        let logs = fs::read_to_string(&log_path).unwrap_or_default();
        panic!(
            "Daemon failed to start on port {} within 30s.\nLogs:\n{}",
            port, logs
        );
    }
}

impl Drop for DaemonFixture {
    fn drop(&mut self) {
        let _ = self.server_process.kill();
        let _ = self.server_process.wait();
    }
}

fn get_daemon() -> &'static DaemonFixture {
    DAEMON.get_or_init(DaemonFixture::start)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return a `Command` pre-configured to talk to the test daemon.
fn cli() -> Command {
    let daemon = get_daemon();
    let mut cmd = Command::cargo_bin("folddb").expect("find folddb binary");
    cmd.arg("--json")
        .arg("--config")
        .arg(&daemon.config_path)
        .env("FOLDDB_PORT", daemon.port.to_string())
        .env("FOLDDB_HOME", daemon._tmpdir.path())
        .env("NODE_CONFIG", &daemon.config_path);
    cmd
}

fn parse_stdout(output: &std::process::Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "Failed to parse JSON: {}\nstdout: {}\nstderr: {}",
            e,
            stdout,
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

// ---------------------------------------------------------------------------
// Tests — daemon commands (these don't need the shared daemon)
// ---------------------------------------------------------------------------

#[test]
fn daemon_status_shows_not_running() {
    // Use a port that's definitely not our daemon
    let mut cmd = Command::cargo_bin("folddb").expect("find folddb binary");
    cmd.arg("--json")
        .arg("daemon")
        .arg("status")
        .env("FOLDDB_PORT", "19999")
        .env("FOLDDB_HOME", "/tmp/nonexistent-folddb-test");

    let output = cmd.output().expect("run daemon status");
    assert!(output.status.success());
    let json = parse_stdout(&output);
    let msg = json["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("not running"),
        "Expected 'not running', got: {}",
        msg
    );
}

// ---------------------------------------------------------------------------
// Tests — commands through daemon HTTP
// ---------------------------------------------------------------------------

#[test]
fn status_returns_json() {
    let output = cli().arg("status").output().expect("run status");
    assert!(
        output.status.success(),
        "status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Daemon returns JSON — verify it's parseable
    let json = parse_stdout(&output);
    assert!(json.is_object(), "status should return JSON object");
}

#[test]
fn config_path_returns_path() {
    let daemon = get_daemon();
    let output = cli()
        .arg("config")
        .arg("path")
        .output()
        .expect("run config path");

    assert!(output.status.success());
    let json = parse_stdout(&output);
    assert_eq!(json["ok"], true);
    let path = json["message"].as_str().unwrap_or("");
    assert!(
        path.contains(daemon._tmpdir.path().to_str().unwrap())
            || path == daemon.config_path.to_str().unwrap(),
        "path '{}' should reference config",
        path
    );
}

#[test]
fn schema_list_returns_json() {
    let output = cli()
        .arg("schema")
        .arg("list")
        .output()
        .expect("run schema list");

    assert!(
        output.status.success(),
        "schema list failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = parse_stdout(&output);
    assert!(json.is_object());
}

#[test]
fn search_returns_json() {
    let output = cli()
        .arg("search")
        .arg("nothing_matches")
        .output()
        .expect("run search");

    assert!(
        output.status.success(),
        "search failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = parse_stdout(&output);
    assert!(json.is_object());
}

#[test]
fn mutate_invalid_json_fields_fails() {
    cli()
        .arg("mutate")
        .arg("run")
        .arg("SomeSchema")
        .arg("--type")
        .arg("create")
        .arg("--fields")
        .arg("not-valid-json")
        .assert()
        .failure()
        .stdout(predicate::str::contains("Invalid fields JSON"));
}

#[test]
fn reset_requires_confirm_in_json_mode() {
    cli()
        .arg("reset")
        .assert()
        .failure()
        .stdout(predicate::str::contains("--confirm"));
}

#[test]
fn completions_bash() {
    let daemon = get_daemon();
    Command::cargo_bin("folddb")
        .expect("find folddb binary")
        .arg("--config")
        .arg(&daemon.config_path)
        .env("FOLDDB_HOME", daemon._tmpdir.path())
        .env("NODE_CONFIG", &daemon.config_path)
        .arg("completions")
        .arg("bash")
        .assert()
        .success()
        .stdout(predicate::str::contains("folddb"));
}

#[test]
fn dev_flag_parses() {
    // Just verify --dev doesn't crash (daemon status doesn't need daemon)
    let mut cmd = Command::cargo_bin("folddb").expect("find folddb binary");
    cmd.arg("--json")
        .arg("--dev")
        .arg("daemon")
        .arg("status")
        .env("FOLDDB_PORT", "19999")
        .env("FOLDDB_HOME", "/tmp/nonexistent-folddb-test");

    let output = cmd.output().expect("run --dev daemon status");
    assert!(output.status.success());
}
