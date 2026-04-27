use crate::error::CliError;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Get the FOLDDB_HOME directory
fn folddb_home() -> PathBuf {
    std::env::var("FOLDDB_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".folddb")
        })
}

/// Get the PID file path
fn pid_file() -> PathBuf {
    folddb_home().join("folddb.pid")
}

/// Get the server log file path
fn log_file() -> PathBuf {
    folddb_home().join("server.log")
}

/// Read the PID from the PID file, if it exists and the process is alive
pub fn read_running_pid() -> Option<u32> {
    let pid_path = pid_file();
    let pid_str = fs::read_to_string(&pid_path).ok()?;
    let pid: u32 = pid_str.trim().parse().ok()?;

    if is_process_alive(pid) {
        Some(pid)
    } else {
        // Stale PID file
        let _ = fs::remove_file(&pid_path);
        None
    }
}

/// Check if a process with the given PID is alive
fn is_process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Check if the daemon is healthy by hitting the unauthenticated `/api/health`
/// endpoint. `/api/system/status` requires `X-User-Hash` and returns 401
/// without it, which conflates "auth missing" with "daemon missing" — use the
/// dedicated health route instead so this probe doubles as the route external
/// uptime monitors and load balancers hit.
pub async fn check_daemon_health(port: u16) -> bool {
    let url = format!("http://127.0.0.1:{}/api/health", port);
    // trace-egress: loopback (CLI -> local daemon health probe; inject_w3c wrapping deferred — pending fold_db rev bump)
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .unwrap();
    match client.get(url).send().await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

/// Resolve the daemon port from FOLDDB_PORT env var or default 9001.
pub fn default_port() -> u16 {
    std::env::var("FOLDDB_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(9001)
}

/// Whether the caller explicitly set FOLDDB_PORT. Used by `ensure_running`
/// to decide whether to default to prod port 9001 is safe — if the env
/// var isn't set we refuse to auto-spawn anything from `~/.folddb`.
fn folddb_port_is_set() -> bool {
    std::env::var("FOLDDB_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .is_some()
}

/// Check if dev mode is persisted in the config file (via `config set env dev`).
pub fn is_dev_in_config() -> bool {
    let config_path = folddb_home().join("config").join("node_config.json");
    std::fs::read_to_string(config_path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("env").and_then(|e| e.as_str()).map(|s| s == "dev"))
        .unwrap_or(false)
}

/// Check if a port is already in use by trying to bind to it
fn is_port_in_use(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_err()
}

/// Start the daemon
pub async fn start(port: u16, dev: bool, open: bool) -> Result<String, CliError> {
    if let Some(pid) = read_running_pid() {
        if check_daemon_health(port).await {
            let url = format!("http://localhost:{}", port);
            if open {
                let _ = open_in_browser(&url);
            }
            return Ok(format!(
                "FoldDB is already running at {}\n(PID {} — stop with `folddb daemon stop`)",
                url, pid,
            ));
        }
        stop_process(pid);
        let _ = fs::remove_file(pid_file());
    }

    // Check port availability before starting
    if is_port_in_use(port) {
        return Err(CliError::new(format!("Port {} is already in use", port))
            .with_hint("Use --port to pick another port, or stop the process using this port"));
    }

    let home = folddb_home();
    fs::create_dir_all(&home)
        .map_err(|e| CliError::new(format!("Failed to create FOLDDB_HOME: {}", e)))?;

    // Find the folddb_server binary (same directory as this binary)
    let current_exe = std::env::current_exe()
        .map_err(|e| CliError::new(format!("Cannot determine executable path: {}", e)))?;
    let bin_dir = current_exe
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let server_bin = bin_dir.join("folddb_server");

    if !server_bin.exists() {
        return Err(CliError::new(format!(
            "folddb_server binary not found at {}",
            server_bin.display()
        ))
        .with_hint("Build with: cargo build --bin folddb_server"));
    }

    let log_path = log_file();
    let log = fs::File::create(&log_path)
        .map_err(|e| CliError::new(format!("Failed to create log file: {}", e)))?;
    let log_err = log
        .try_clone()
        .map_err(|e| CliError::new(format!("Failed to clone log handle: {}", e)))?;

    let mut cmd = Command::new(&server_bin);
    cmd.arg("--port").arg(port.to_string());

    if dev {
        cmd.arg("--schema-service-url")
            .arg("https://y0q3m6vk75.execute-api.us-west-2.amazonaws.com");
        cmd.env("EXEMEM_ENV", "dev");
    }

    cmd.stdout(log).stderr(log_err);

    let child = cmd
        .spawn()
        .map_err(|e| CliError::new(format!("Failed to start daemon: {}", e)))?;

    let pid = child.id();

    fs::write(pid_file(), pid.to_string())
        .map_err(|e| CliError::new(format!("Failed to write PID file: {}", e)))?;

    eprintln!("Starting FoldDB daemon…");

    // Poll for health (up to 30 seconds). Show a progress dot each second after
    // t=2s so the user sees that something is happening during the fastembed /
    // ONNX init that dominates first-boot latency.
    use std::io::Write;
    let timeout = 30;
    let mut printed_dots = false;
    for i in 0..timeout {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        if !is_process_alive(pid) {
            if printed_dots {
                eprintln!();
            }
            let log_tail = read_log_tail(&log_path);
            let _ = fs::remove_file(pid_file());
            return Err(CliError::new("Daemon process died during startup")
                .with_hint(format!("Last log output:\n{}", log_tail)));
        }

        if check_daemon_health(port).await {
            if printed_dots {
                eprintln!();
            }
            let env_suffix = if dev { " (dev)" } else { "" };
            let url = format!("http://localhost:{}", port);
            if open {
                let _ = open_in_browser(&url);
            }
            return Ok(format!(
                "FoldDB is running at {}{}\n(PID {} — stop with `folddb daemon stop`)",
                url, env_suffix, pid,
            ));
        }

        if i >= 2 {
            eprint!(".");
            let _ = std::io::stderr().flush();
            printed_dots = true;
        }
    }

    // Timeout
    if printed_dots {
        eprintln!();
    }
    stop_process(pid);
    let _ = fs::remove_file(pid_file());
    let log_tail = read_log_tail(&log_path);
    Err(
        CliError::new(format!("Daemon failed to start within {}s", timeout))
            .with_hint(format!("Last log output:\n{}", log_tail)),
    )
}

/// Try to open `url` in the user's default browser. Best-effort — if the open
/// command fails we still return success from `daemon start` since the URL is
/// in the output and the user can click/paste it themselves.
fn open_in_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let cmd = "xdg-open";
    #[cfg(not(unix))]
    let cmd: &str = {
        // No brew tap for Windows — but keep the code path compilable.
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "browser open not implemented for this platform",
        ));
    };
    let _child = Command::new(cmd).arg(url).spawn()?;
    Ok(())
}

/// Stop the daemon
pub fn stop() -> Result<String, CliError> {
    let pid = read_running_pid().ok_or_else(|| CliError::new("Daemon not running"))?;

    stop_process(pid);

    for _ in 0..5 {
        if !is_process_alive(pid) {
            let _ = fs::remove_file(pid_file());
            return Ok(format!("Daemon stopped (was PID {})", pid));
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    // Force kill
    unsafe {
        libc::kill(pid as i32, libc::SIGKILL);
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    let _ = fs::remove_file(pid_file());
    Ok(format!("Daemon force-killed (was PID {})", pid))
}

fn stop_process(pid: u32) {
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
}

/// Get daemon status
pub async fn status() -> Result<String, CliError> {
    match read_running_pid() {
        Some(pid) => {
            let port = default_port();
            let healthy = check_daemon_health(port).await;
            let health_str = if healthy { "healthy" } else { "not responding" };
            Ok(format!(
                "Daemon running (PID {}, port {}, {})",
                pid, port, health_str
            ))
        }
        None => Ok("Daemon not running".to_string()),
    }
}

/// Ensure the daemon is reachable. Returns the port it's listening on.
///
/// The CLI talks to an **already-running** daemon. It never spawns a daemon
/// implicitly: silently starting `folddb_server` against `~/.folddb` on the
/// prod port (9001) can corrupt real user state when the CLI was run from a
/// worktree / CI / scratch session that never meant to touch prod. Use
/// `folddb daemon start` to start one deliberately.
pub async fn ensure_running() -> Result<u16, CliError> {
    let port = default_port();
    let port_was_explicit = folddb_port_is_set();

    // Retry the health check before deciding the daemon is down. Under CI
    // load (or on a laptop with a busy CPU), a single 2s health check can
    // time out even when the daemon is healthy — three tries with a short
    // backoff covers the transient case without adding noticeable latency
    // to the happy path.
    for attempt in 0..3 {
        if check_daemon_health(port).await {
            return Ok(port);
        }
        if attempt < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }

    // If FOLDDB_PORT is unset, the CLI is about to talk to (or spawn against)
    // the prod port 9001. Refuse so we never silently reach into `~/.folddb`
    // from a context that didn't opt in to the prod instance.
    if !port_was_explicit {
        return Err(CliError::new(
            "No folddb daemon running and FOLDDB_PORT is not set",
        )
        .with_hint(
            "Set FOLDDB_PORT to an existing daemon's port, or run `folddb daemon start --port <N>` first. \
             The CLI does not auto-spawn a daemon — it would pick prod port 9001 and write to ~/.folddb.",
        ));
    }

    // A port was explicitly requested but no daemon is answering on it.
    // Warn if a PID file exists but on a different port.
    if let Some(pid) = read_running_pid() {
        eprintln!(
            "Warning: daemon PID {} exists but port {} is not responding.",
            pid, port
        );
        eprintln!("The daemon may be running on a different port.");
        eprintln!("Run `folddb daemon stop` first, or set FOLDDB_PORT to match.");
    }

    if is_port_in_use(port) {
        return Err(CliError::new(format!(
            "Port {} is in use but the daemon's health endpoint isn't responding",
            port
        ))
        .with_hint(
            "another process is holding this port. Run `folddb daemon stop` to clean up, or set FOLDDB_PORT to a free port.",
        ));
    }

    Err(CliError::new(format!(
        "No folddb daemon running on port {}",
        port
    ))
    .with_hint(format!(
        "Start one explicitly: `folddb daemon start --port {}`. The CLI does not auto-spawn a daemon.",
        port
    )))
}

// ---------------------------------------------------------------------------
// Service install/uninstall
// ---------------------------------------------------------------------------

const LAUNCHD_LABEL: &str = "com.folddb.daemon";

fn launchd_plist_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library/LaunchAgents")
        .join(format!("{}.plist", LAUNCHD_LABEL))
}

/// Install a launchd LaunchAgent so the daemon auto-starts on login.
pub fn install() -> Result<String, CliError> {
    if cfg!(not(target_os = "macos")) {
        return Err(CliError::new(
            "Service install is only supported on macOS (launchd)",
        ));
    }

    let server_bin = find_server_binary()?;
    let home = folddb_home();
    let log_path = log_file();
    let port = default_port();
    let dev = is_dev_in_config();

    // Build program arguments — include --schema-service-url if dev mode persisted
    let mut args = format!(
        r#"    <array>
        <string>{binary}</string>
        <string>--port</string>
        <string>{port}</string>"#,
        binary = server_bin.display(),
        port = port,
    );
    if dev {
        args.push_str(
            r#"
        <string>--schema-service-url</string>
        <string>https://y0q3m6vk75.execute-api.us-west-2.amazonaws.com</string>"#,
        );
    }
    args.push_str("\n    </array>");

    // Include EXEMEM_ENV if dev mode
    let mut env_vars = format!(
        r#"        <key>FOLDDB_HOME</key>
        <string>{home}</string>"#,
        home = home.display(),
    );
    if dev {
        env_vars.push_str(
            r#"
        <key>EXEMEM_ENV</key>
        <string>dev</string>"#,
        );
    }

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
{args}
    <key>EnvironmentVariables</key>
    <dict>
{env_vars}
    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
</dict>
</plist>"#,
        label = LAUNCHD_LABEL,
        args = args,
        env_vars = env_vars,
        log = log_path.display(),
    );

    let plist_path = launchd_plist_path();
    if let Some(parent) = plist_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| CliError::new(format!("Failed to create LaunchAgents dir: {}", e)))?;
    }

    // Unload first if already installed
    if plist_path.exists() {
        let _ = std::process::Command::new("launchctl")
            .arg("unload")
            .arg(&plist_path)
            .output();
    }

    fs::write(&plist_path, plist)
        .map_err(|e| CliError::new(format!("Failed to write plist: {}", e)))?;

    let output = std::process::Command::new("launchctl")
        .arg("load")
        .arg(&plist_path)
        .output()
        .map_err(|e| CliError::new(format!("Failed to run launchctl load: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::new(format!("launchctl load failed: {}", stderr)));
    }

    Ok(format!(
        "Service installed at {}\nDaemon will auto-start on login (port {})",
        plist_path.display(),
        port
    ))
}

/// Uninstall the launchd LaunchAgent.
pub fn uninstall() -> Result<String, CliError> {
    let plist_path = launchd_plist_path();

    if !plist_path.exists() {
        return Ok("Service not installed.".to_string());
    }

    let output = std::process::Command::new("launchctl")
        .arg("unload")
        .arg(&plist_path)
        .output()
        .map_err(|e| CliError::new(format!("Failed to run launchctl unload: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::new(format!(
            "launchctl unload failed: {}",
            stderr
        )));
    }

    fs::remove_file(&plist_path)
        .map_err(|e| CliError::new(format!("Failed to remove plist: {}", e)))?;

    Ok("Service uninstalled. Daemon will no longer auto-start.".to_string())
}

fn find_server_binary() -> Result<PathBuf, CliError> {
    let current_exe = std::env::current_exe()
        .map_err(|e| CliError::new(format!("Cannot determine executable path: {}", e)))?;
    let bin_dir = current_exe
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let server_bin = bin_dir.join("folddb_server");

    if !server_bin.exists() {
        return Err(CliError::new(format!(
            "folddb_server binary not found at {}",
            server_bin.display()
        ))
        .with_hint("Build with: cargo build --bin folddb_server"));
    }

    Ok(server_bin)
}

fn read_log_tail(path: &std::path::Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .rev()
        .take(20)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n")
}
