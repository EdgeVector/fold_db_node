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

/// Return a `Command` pre-configured to talk to a daemon (human-readable mode).
fn cli_with_daemon(daemon: &DaemonFixture) -> Command {
    let mut cmd = Command::cargo_bin("folddb").expect("find folddb binary");
    cmd.arg("--config")
        .arg(&daemon.config_path)
        .env("FOLDDB_PORT", daemon.port.to_string())
        .env("FOLDDB_HOME", daemon._tmpdir.path())
        .env("NODE_CONFIG", &daemon.config_path);
    cmd
}

/// Return a `Command` pre-configured to talk to a daemon (JSON mode).
fn cli_json_with_daemon(daemon: &DaemonFixture) -> Command {
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

// ---------------------------------------------------------------------------
// Org commands
// ---------------------------------------------------------------------------

#[test]
fn org_list_returns_json() {
    let daemon = get_daemon();
    let assert = cli_with_daemon(daemon)
        .arg("org")
        .arg("list")
        .assert()
        .success();
    let output = String::from_utf8_lossy(&assert.get_output().stdout);
    // Should show either "No organizations" or a list
    assert!(
        output.contains("organization") || output.contains("No organizations"),
        "org list should show org info or empty message: {}",
        output
    );
}

#[test]
fn org_list_json_mode() {
    let daemon = get_daemon();
    let assert = cli_json_with_daemon(daemon)
        .arg("org")
        .arg("list")
        .assert()
        .success();
    let output = String::from_utf8_lossy(&assert.get_output().stdout);
    let json: Value = serde_json::from_str(&output).expect("org list should return valid JSON");
    assert!(json.get("ok").is_some() || json.get("data").is_some());
}

#[test]
fn org_invites_returns_response() {
    let daemon = get_daemon();
    let assert = cli_with_daemon(daemon)
        .arg("org")
        .arg("invites")
        .assert()
        .success();
    let output = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(
        output.contains("invitation")
            || output.contains("pending")
            || output.contains("No pending"),
        "org invites should show invite info: {}",
        output
    );
}

#[test]
fn org_join_invalid_json_fails() {
    let daemon = get_daemon();
    cli_with_daemon(daemon)
        .arg("org")
        .arg("join")
        .arg("not-valid-json")
        .assert()
        .failure()
        .stderr(predicate::str::contains("Invalid invite JSON"));
}

// ---------------------------------------------------------------------------
// Cloud commands
// ---------------------------------------------------------------------------

#[test]
fn cloud_status_returns_response() {
    let daemon = get_daemon();
    let assert = cli_with_daemon(daemon)
        .arg("cloud")
        .arg("status")
        .assert()
        .success();
    let output = String::from_utf8_lossy(&assert.get_output().stdout);
    // In local mode, should show "disabled"
    assert!(
        output.contains("sync") || output.contains("Cloud") || output.contains("disabled"),
        "cloud status should show sync info: {}",
        output
    );
}

#[test]
fn cloud_sync_in_local_mode_fails() {
    let daemon = get_daemon();
    // In local mode (no cloud config), sync trigger should fail
    cli_with_daemon(daemon)
        .arg("cloud")
        .arg("sync")
        .assert()
        .failure();
}

// ---------------------------------------------------------------------------
// Rich status command
// ---------------------------------------------------------------------------

#[test]
fn status_human_shows_version() {
    let daemon = get_daemon();
    let assert = cli_with_daemon(daemon).arg("status").assert().success();
    let output = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(
        output.contains("FoldDB v"),
        "human status should show version: {}",
        output
    );
}

#[test]
fn status_human_shows_node_hash() {
    let daemon = get_daemon();
    let assert = cli_with_daemon(daemon).arg("status").assert().success();
    let output = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(
        output.contains("Node:"),
        "human status should show node hash: {}",
        output
    );
}

#[test]
fn status_json_mode_returns_raw_json() {
    let daemon = get_daemon();
    let assert = cli_json_with_daemon(daemon)
        .arg("status")
        .assert()
        .success();
    let output = String::from_utf8_lossy(&assert.get_output().stdout);
    let json: Value =
        serde_json::from_str(&output).expect("--json status should return valid JSON");
    assert!(json.get("data").is_some() || json.get("status").is_some());
}

// ---------------------------------------------------------------------------
// Regression: --config should NOT re-enter the setup wizard in non-TTY
// when the node already has an identity on disk / a daemon running.
//
// Repro: `run.sh` writes `node_config.json` without identity keys (those live
// in `node_identity.json`). Pre-fix, any CLI call with --config in a non-TTY
// context (CI, cron, background agent) would trip the interactive wizard and
// fail with "Input cancelled: IO error: not a terminal".
//
// Filed as Alpha dogfood papercut 2026-04-19 (kanban e2db0).
// ---------------------------------------------------------------------------

#[test]
fn config_without_public_key_uses_identity_file() {
    let daemon = get_daemon();

    // Write a stripped config — identical to what `run.sh` produces: NO identity keys.
    let stripped_config = serde_json::json!({
        "database": {
            "path": daemon._tmpdir.path().join("db").to_str().unwrap()
        },
        "network_listen_address": "/ip4/0.0.0.0/tcp/0",
        "security_config": {
            "require_tls": false,
            "encrypt_at_rest": false
        },
        "schema_service_url": "test://mock"
    });
    let stripped_path = daemon._tmpdir.path().join("stripped_config.json");
    fs::write(
        &stripped_path,
        serde_json::to_string_pretty(&stripped_config).unwrap(),
    )
    .expect("write stripped config");

    // Run a command that would otherwise hit the wizard. stdin is not a TTY
    // under assert_cmd, so a regression would reproduce the original error.
    let mut cmd = Command::cargo_bin("folddb").expect("find folddb binary");
    cmd.arg("--config")
        .arg(&stripped_path)
        .arg("status")
        .env("FOLDDB_PORT", daemon.port.to_string())
        .env("FOLDDB_HOME", daemon._tmpdir.path())
        .env_remove("NODE_CONFIG");

    let output = cmd.output().expect("run status with stripped config");
    assert!(
        output.status.success(),
        "status with stripped config should succeed via identity-file hydration.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("FoldDB v"),
        "status output should contain version banner: {}",
        stdout
    );
    // The original repro explicitly printed this wizard banner.
    assert!(
        !stdout.contains("Welcome to FoldDB!")
            && !String::from_utf8_lossy(&output.stderr).contains("Welcome to FoldDB!"),
        "CLI must not enter the setup wizard when identity is on disk"
    );
}

#[test]
fn no_identity_and_no_daemon_fails_cleanly_in_non_tty() {
    // Isolated tmpdir — no identity file, no daemon. Non-TTY execution must
    // fail with the new actionable hint, NOT drop into the wizard.
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let config = serde_json::json!({
        "database": { "path": tmp.path().join("db").to_str().unwrap() },
        "network_listen_address": "/ip4/0.0.0.0/tcp/0",
        "security_config": { "require_tls": false, "encrypt_at_rest": false },
        "schema_service_url": "test://mock"
    });
    let config_path = tmp.path().join("config.json");
    fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()).expect("write config");

    let mut cmd = Command::cargo_bin("folddb").expect("find folddb binary");
    cmd.arg("--config")
        .arg(&config_path)
        .arg("status")
        // Point at a port nothing is listening on so the daemon probe returns None.
        .env("FOLDDB_PORT", "59999")
        .env("FOLDDB_HOME", tmp.path())
        .env_remove("NODE_CONFIG");

    let output = cmd.output().expect("run status with no identity");
    assert!(
        !output.status.success(),
        "should fail when no identity source is available"
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("stdin is not a terminal") || combined.contains("daemon start"),
        "should produce actionable non-TTY hint, got: {}",
        combined
    );
    assert!(
        !combined.contains("Welcome to FoldDB!"),
        "must not drop into wizard: {}",
        combined
    );
}

// ---------------------------------------------------------------------------
// Regression: the CLI must NOT auto-spawn a `folddb_server` daemon.
//
// Pre-fix (alpha dogfood run-5, kanban 4f115), running any data command from
// a worktree / CI / scratch session with `FOLDDB_PORT` unset caused the CLI
// to silently spawn a production daemon on port 9001 from `~/.folddb` — a
// safety risk that could corrupt real user state. The CLI now refuses to
// spawn and tells the operator to start the daemon explicitly.
// ---------------------------------------------------------------------------
#[test]
fn cli_does_not_auto_spawn_daemon_when_port_unset() {
    // Isolated tmpdir with a valid identity so we bypass the setup wizard
    // and land on the `ensure_running` guard. Identity now lives in the
    // Sled `node_identity` tree, not in `node_config.json`.
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let keypair = fold_db::security::Ed25519KeyPair::generate().expect("generate keypair");
    let data_path = tmp.path().join("db");
    fold_db_node::identity::save_standalone(
        &data_path,
        &fold_db_node::identity::identity_from_keypair(&keypair),
    )
    .expect("seed identity");

    let config = serde_json::json!({
        "database": { "path": data_path.to_str().unwrap() },
        "storage_path": data_path.to_str().unwrap(),
        "network_listen_address": "/ip4/0.0.0.0/tcp/0",
        "security_config": { "require_tls": false, "encrypt_at_rest": false },
        "schema_service_url": "test://mock"
    });
    let config_path = tmp.path().join("config.json");
    fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()).expect("write config");

    let mut cmd = Command::cargo_bin("folddb").expect("find folddb binary");
    cmd.arg("--config")
        .arg(&config_path)
        .arg("status")
        // Deliberately leave FOLDDB_PORT unset.
        .env_remove("FOLDDB_PORT")
        .env("FOLDDB_HOME", tmp.path())
        .env_remove("NODE_CONFIG");

    let output = cmd.output().expect("run status without FOLDDB_PORT");
    assert!(
        !output.status.success(),
        "CLI must refuse to auto-spawn when FOLDDB_PORT is unset.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("FOLDDB_PORT") && combined.contains("daemon start"),
        "error must point at FOLDDB_PORT and `folddb daemon start`, got: {}",
        combined
    );
    // Safety net: no PID file should have been written into the isolated
    // FOLDDB_HOME (auto-spawn would write one).
    let pid_path = tmp.path().join("folddb.pid");
    assert!(
        !pid_path.exists(),
        "CLI must not have spawned a daemon (pid file present at {})",
        pid_path.display()
    );
}

#[test]
fn cli_reports_missing_daemon_when_port_explicit() {
    // When FOLDDB_PORT *is* set but nothing is listening, the CLI should
    // still fail with a clear message — no retry storm, no spawn attempt.
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let keypair = fold_db::security::Ed25519KeyPair::generate().expect("generate keypair");
    let data_path = tmp.path().join("db");
    fold_db_node::identity::save_standalone(
        &data_path,
        &fold_db_node::identity::identity_from_keypair(&keypair),
    )
    .expect("seed identity");

    let config = serde_json::json!({
        "database": { "path": data_path.to_str().unwrap() },
        "storage_path": data_path.to_str().unwrap(),
        "network_listen_address": "/ip4/0.0.0.0/tcp/0",
        "security_config": { "require_tls": false, "encrypt_at_rest": false },
        "schema_service_url": "test://mock"
    });
    let config_path = tmp.path().join("config.json");
    fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()).expect("write config");

    // Grab-and-release a free port — minimises the chance of colliding with
    // a leaked daemon from another test run or worktree.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind free port");
    let free_port = listener.local_addr().unwrap().port();
    drop(listener);

    let mut cmd = Command::cargo_bin("folddb").expect("find folddb binary");
    cmd.arg("--config")
        .arg(&config_path)
        .arg("status")
        .env("FOLDDB_PORT", free_port.to_string())
        .env("FOLDDB_HOME", tmp.path())
        .env_remove("NODE_CONFIG");

    let output = cmd.output().expect("run status with dead port");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "CLI must fail when the explicit port has no daemon.\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        combined.contains(&free_port.to_string()) && combined.contains("daemon start"),
        "error should mention the requested port and suggest `daemon start`, got: {}",
        combined
    );
    let pid_path = tmp.path().join("folddb.pid");
    assert!(!pid_path.exists(), "CLI must not have spawned a daemon");
}

// ---------------------------------------------------------------------------
// Regression: `folddb ingest file` must target an endpoint that exists.
//
// Repro (alpha dogfood run 4, kanban 27e5f): pre-fix the CLI posted to
// `/api/ingest`, which the daemon doesn't route, so every invocation
// returned 404 regardless of daemon state. The fix routes the command at
// `/api/ingestion/process` with the proper `IngestionRequest` payload.
//
// The test daemon has no AI provider configured, so the endpoint answers
// 503 `ingestion_unavailable`. That's fine — a 503 proves the route exists
// and the request was well-formed; a 404 would reproduce the bug.
// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// Trigger commands
//
// Smoke tests for `folddb trigger log`. The TriggerFiring schema is registered
// at daemon startup by fold_db core, so hitting `/api/query` for it must
// succeed even with zero firing rows — an unknown view yields the
// "No firings found" message, and `--json` mode proxies through the raw query
// envelope. Proves the CLI → daemon wiring end-to-end without depending on
// any trigger having actually fired during the test run.
// ---------------------------------------------------------------------------
#[test]
fn trigger_log_empty_for_unknown_view_human_mode() {
    let daemon = get_daemon();
    let assert = cli_with_daemon(daemon)
        .arg("trigger")
        .arg("log")
        .arg("nonexistent_trigger_log_view")
        .arg("--last")
        .arg("1h")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(
        stdout.contains("No firings found"),
        "expected 'No firings found' for unknown view, got: {}",
        stdout
    );
    assert!(
        stdout.contains("nonexistent_trigger_log_view"),
        "message should name the view: {}",
        stdout
    );
    assert!(
        stdout.contains("1h"),
        "message should echo the --last window: {}",
        stdout
    );
}

#[test]
fn trigger_log_json_mode_returns_query_envelope() {
    let daemon = get_daemon();
    let assert = cli_json_with_daemon(daemon)
        .arg("trigger")
        .arg("log")
        .arg("nonexistent_trigger_log_view")
        .arg("--last")
        .arg("24h")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    let json: Value =
        serde_json::from_str(&stdout).expect("trigger log --json should return valid JSON");
    assert_eq!(
        json.get("ok").and_then(|v| v.as_bool()),
        Some(true),
        "envelope should be ok:true, got: {}",
        stdout
    );
    assert!(
        json.get("results").is_some(),
        "envelope should carry a results array, got: {}",
        stdout
    );
}

#[test]
fn trigger_log_rejects_invalid_last() {
    let daemon = get_daemon();
    cli_with_daemon(daemon)
        .arg("trigger")
        .arg("log")
        .arg("any_view")
        .arg("--last")
        .arg("not-a-duration")
        .assert()
        .failure()
        .stderr(predicate::str::contains("--last"));
}

#[test]
fn ingest_file_targets_ingestion_process_endpoint() {
    let daemon = get_daemon();

    let payload = serde_json::json!({ "title": "hello", "body": "world" });
    let input_path = daemon._tmpdir.path().join("ingest-regression.json");
    fs::write(&input_path, serde_json::to_string(&payload).unwrap()).expect("write ingest input");

    let output = cli_with_daemon(daemon)
        .arg("ingest")
        .arg("file")
        .arg(&input_path)
        .output()
        .expect("run ingest file");

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !combined.contains("404"),
        "ingest file must not 404 on a nonexistent endpoint: {}",
        combined
    );
}
