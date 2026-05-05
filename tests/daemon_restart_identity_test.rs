//! P0 regression test: daemon restart MUST preserve node identity.
//!
//! Bug context: in production, `folddb daemon stop && folddb daemon start`
//! was silently rotating user_hash across restarts. The Sled `node_identity`
//! tree got rewritten with a fresh keypair, orphaning every previously-
//! ingested record under the old hash. See `src/identity.rs` for the fix —
//! this test exercises the property at the process level.
//!
//! Run with:
//!   cargo test --test daemon_restart_identity_test -- --nocapture

#![allow(deprecated)]

use std::fs;
use std::path::Path;
use std::process::{Child, Stdio};
use tempfile::TempDir;

/// Spawn `folddb_server` against `home`, wait for `/api/system/status` to
/// answer, and return the running child + the resolved port.
fn spawn_daemon(home: &Path, port: u16) -> Child {
    let server_bin = assert_cmd::cargo::cargo_bin("folddb_server");
    let log_path = home.join("server.log");
    let err_path = home.join("server_err.log");

    // A minimal node config — local-only, mock schema service, plaintext
    // storage at $HOME/data. No identity fields: those live in the Sled
    // `node_identity` tree which is exactly what we're testing.
    let config = serde_json::json!({
        "database": { "path": home.join("data").to_str().unwrap() },
        "storage_path": home.join("data").to_str().unwrap(),
        "network_listen_address": "/ip4/0.0.0.0/tcp/0",
        "schema_service_url": "test://mock",
    });
    let config_path = home.join("config").join("node_config.json");
    fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();

    #[allow(clippy::zombie_processes)]
    let child = std::process::Command::new(server_bin)
        .arg("--port")
        .arg(port.to_string())
        .env("NODE_CONFIG", &config_path)
        .env("FOLDDB_HOME", home)
        .stdout(Stdio::from(fs::File::create(&log_path).unwrap()))
        .stderr(Stdio::from(fs::File::create(&err_path).unwrap()))
        .spawn()
        .expect("start folddb_server");

    let url = format!("http://127.0.0.1:{port}/api/system/status");
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .unwrap();
    for _ in 0..30 {
        if client.get(&url).send().is_ok() {
            return child;
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    let logs = fs::read_to_string(&log_path).unwrap_or_default();
    let errs = fs::read_to_string(&err_path).unwrap_or_default();
    panic!("daemon never became healthy on port {port}\nstdout:\n{logs}\nstderr:\n{errs}");
}

fn fetch_user_hash(port: u16) -> String {
    let url = format!("http://127.0.0.1:{port}/api/system/auto-identity");
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();
    let resp = client.get(&url).send().expect("auto-identity");
    assert!(
        resp.status().is_success(),
        "auto-identity returned {}",
        resp.status()
    );
    let body: serde_json::Value = resp.json().expect("parse json");
    body.get("user_hash")
        .and_then(|v| v.as_str())
        .expect("user_hash in response")
        .to_string()
}

fn pick_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

fn stop_daemon(mut child: Child) {
    // SIGTERM first, mirror what `folddb daemon stop` does.
    #[cfg(unix)]
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    for _ in 0..30 {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(200)),
            Err(_) => break,
        }
    }
    // Fall back to SIGKILL so the test doesn't leak a zombie if
    // graceful shutdown stalls — same as `folddb daemon stop`.
    let _ = child.kill();
    let _ = child.wait();
}

/// The acceptance criterion: `folddb daemon stop && folddb daemon start`
/// preserves the user_hash on a brew-style install. This shells out to
/// folddb_server, captures the user_hash via /api/system/auto-identity,
/// stops the daemon, restarts it against the same FOLDDB_HOME, and
/// asserts the user_hash is identical. Anything else is the silent
/// identity-rotation bug fixed in this commit.
#[test]
fn user_hash_survives_daemon_restart() {
    let home = TempDir::new().expect("tempdir");

    let port = pick_port();
    let child = spawn_daemon(home.path(), port);
    let first_hash = fetch_user_hash(port);
    stop_daemon(child);

    // Restart against the same FOLDDB_HOME — same Sled directory.
    // Pick a fresh port so the test isn't sensitive to TIME_WAIT.
    let port2 = pick_port();
    let child2 = spawn_daemon(home.path(), port2);
    let second_hash = fetch_user_hash(port2);
    stop_daemon(child2);

    assert_eq!(
        first_hash, second_hash,
        "user_hash rotated across daemon restart — that's the P0 bug"
    );
}
