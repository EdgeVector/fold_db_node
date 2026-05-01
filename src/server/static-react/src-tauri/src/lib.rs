use fold_db::security::Ed25519KeyPair;
use fold_db_node::fold_node::config::{load_node_config, DatabaseConfig};
use fold_db_node::server::node_manager::NodeManagerConfig;
use fold_db_node::server::{start_embedded_server_lazy, EmbeddedServerHandle};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::{Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt;
use tokio::sync::Mutex;

/// Shared state for the Tauri application
pub struct AppState {
    pub server_handle: Arc<Mutex<Option<EmbeddedServerHandle>>>,
    pub server_port: u16,
}

/// Server status response
#[derive(Debug, Serialize, Deserialize)]
pub struct ServerStatus {
    pub running: bool,
    pub port: u16,
    pub url: String,
}

/// Get the current server status
#[tauri::command]
async fn get_server_status(state: State<'_, AppState>) -> Result<ServerStatus, String> {
    let handle = state.server_handle.lock().await;
    let running = handle.as_ref().map(|h| h.is_running()).unwrap_or(false);

    Ok(ServerStatus {
        running,
        port: state.server_port,
        url: format!("http://localhost:{}", state.server_port),
    })
}

/// Open the data directory in Finder/Explorer
#[tauri::command]
async fn open_data_directory() -> Result<(), String> {
    let data_dir = dirs::home_dir()
        .ok_or("Could not determine home directory")?
        .join(".folddb")
        .join("data");

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&data_dir)
            .spawn()
            .map_err(|e| format!("Failed to open directory: {}", e))?;
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&data_dir)
            .spawn()
            .map_err(|e| format!("Failed to open directory: {}", e))?;
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&data_dir)
            .spawn()
            .map_err(|e| format!("Failed to open directory: {}", e))?;
    }

    Ok(())
}

/// Get the app version
#[tauri::command]
fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Update availability info sent to the frontend
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub version: String,
    pub current_version: String,
    pub body: Option<String>,
}

/// Check for updates and return info if available
#[tauri::command]
async fn check_for_update(app: tauri::AppHandle) -> Result<Option<UpdateInfo>, String> {
    let updater = app
        .updater()
        .map_err(|e| format!("Updater not available: {}", e))?;
    match updater.check().await {
        Ok(Some(update)) => Ok(Some(UpdateInfo {
            version: update.version.clone(),
            current_version: update.current_version.clone(),
            body: update.body.clone(),
        })),
        Ok(None) => Ok(None),
        Err(e) => {
            eprintln!("[FoldDB] Update check failed: {}", e);
            Err(format!("Update check failed: {}", e))
        }
    }
}

/// Download and install an available update, then restart
#[tauri::command]
async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    let updater = app
        .updater()
        .map_err(|e| format!("Updater not available: {}", e))?;
    let update = updater
        .check()
        .await
        .map_err(|e| format!("Update check failed: {}", e))?
        .ok_or_else(|| "No update available".to_string())?;

    let mut started = false;
    update
        .download_and_install(
            |chunk_size, content_length| {
                if !started {
                    eprintln!(
                        "[FoldDB] Downloading update ({} bytes)",
                        content_length.unwrap_or(0)
                    );
                    started = true;
                } else {
                    eprintln!("[FoldDB] Downloaded {} bytes", chunk_size);
                }
            },
            || {
                eprintln!("[FoldDB] Download finished, installing...");
            },
        )
        .await
        .map_err(|e| format!("Install failed: {}", e))?;

    // Restart the app to apply the update
    app.restart();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Pick a port. Prefer 9001 (stable for external tools / docs), but fall
    // back through 9002..=9010 so launching the app doesn't fail just because
    // something else on the machine happens to hold 9001. On error we route
    // through the existing startup_error dialog below.
    let (server_port, port_error) = match pick_port(9001, 10) {
        Ok(p) => {
            eprintln!("[FoldDB] Selected port {p}");
            // Publish the chosen port so external tools (curl, CLI scripts) can
            // find the running instance without hard-coding 9001.
            if let Some(home) = dirs::home_dir() {
                let folddb_dir = home.join(".folddb");
                let _ = std::fs::create_dir_all(&folddb_dir);
                let _ = std::fs::write(folddb_dir.join("port"), p.to_string());
            }
            (p, None)
        }
        // Keep server_port = 9001 as a placeholder so the rest of this function
        // compiles; the error path below short-circuits before we'd actually
        // connect to it.
        Err(e) => (9001u16, Some(e)),
    };

    // If port selection failed, skip spawning the server and pipe the error
    // straight into the existing startup_error dialog flow below.
    let (server_handle, startup_error) = if let Some(e) = port_error {
        (None, Some(e))
    } else {
        let (tx, rx) = std::sync::mpsc::channel::<Result<EmbeddedServerHandle, String>>();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
            let result = rt.block_on(start_fold_server(server_port));
            match result {
                Ok(handle) => {
                    eprintln!("[FoldDB] Server started on port {server_port}");
                    let _ = tx.send(Ok(handle));
                    // Keep runtime alive so the server keeps running
                    rt.block_on(std::future::pending::<()>());
                }
                Err(e) => {
                    eprintln!("[FoldDB] Failed to start server: {e}");
                    let _ = tx.send(Err(e));
                }
            }
        });

        // Wait for the server to start (with timeout)
        match rx.recv_timeout(std::time::Duration::from_secs(30)) {
            Ok(Ok(handle)) => (Some(handle), None),
            Ok(Err(e)) => (None, Some(e)),
            Err(_) => (
                None,
                Some("Server failed to start within 30 seconds.".to_string()),
            ),
        }
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            get_server_status,
            open_data_directory,
            get_app_version,
            check_for_update,
            install_update
        ])
        .setup(move |app| {
            // Set up logging via tauri-plugin-log. Default targets are
            // Stdout (only useful when launched from a terminal),
            // LogDir (file at ~/Library/Logs/com.folddb.app/FoldDB.log
            // on macOS — what users should `tail -f` when something
            // misbehaves), and Webview (DevTools console). The
            // `let _` is load-bearing: if the embedded server already
            // initialized a global logger (which it can, via
            // observability::init_node_with_web), we just no-op rather
            // than crashing — better to lose Tauri-side logs to that
            // race than to refuse to start.
            //
            // All `log::info!` / `log::warn!` / `log::error!` calls
            // BELOW this line route through the plugin. eprintln!
            // calls earlier in `pub fn run()` (port pick, server-thread
            // result) write to stderr only — invisible when launched
            // via `open -a FoldDB`. If you need diagnosis pre-setup,
            // launch the binary directly:
            //   /Applications/FoldDB.app/Contents/MacOS/fold-app
            let _ = app.handle().plugin(
                tauri_plugin_log::Builder::default()
                    .level(log::LevelFilter::Info)
                    .build(),
            );

            // Initialize app state
            app.manage(AppState {
                server_handle: Arc::new(Mutex::new(server_handle)),
                server_port,
            });

            // If the server failed to start, show an error dialog and exit.
            // Note: sled lock conflicts no longer happen at startup (lazy init),
            // but other failures (config load, identity generation) can still occur.
            if let Some(error) = startup_error {
                let message = format!("FoldDB server failed to start:\n\n{}", error);

                app.dialog()
                    .message(message)
                    .kind(MessageDialogKind::Error)
                    .title("FoldDB - Startup Error")
                    .blocking_show();

                std::process::exit(1);
            }

            // Block until the embedded server is actually accepting TCP
            // connections. `start_embedded_server_lazy` returns Ok the
            // instant `tokio::spawn` queues the runner; `actix_web::HttpServer::bind`
            // hasn't run yet at that point. If the webview navigates before
            // bind(), it gets ECONNREFUSED on `GET /` and WebKit pins the
            // error page — the window stays blank forever, even after the
            // listener comes up. Symptom users have hit on every macOS
            // build before v0.3.15.
            //
            // Poll TCP connect (not /api/health) so we don't depend on
            // routes being wired or the node's first request being
            // processed; we just need the kernel-level listener accepting.
            // A successful TCP handshake is enough — by the time the
            // webview opens its connection, actix's accept loop is also
            // running.
            {
                use std::net::{SocketAddr, TcpStream};
                use std::time::{Duration, Instant};
                let addr: SocketAddr = format!("127.0.0.1:{server_port}")
                    .parse()
                    .expect("loopback addr always parses");
                let deadline = Instant::now() + Duration::from_secs(15);
                let mut ready = false;
                while Instant::now() < deadline {
                    if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
                        ready = true;
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                if !ready {
                    log::warn!(
                        "[FoldDB] Embedded server did not start accepting on {addr} within 15s; opening window anyway, expect a blank page."
                    );
                } else {
                    log::info!("[FoldDB] Embedded server accepting on {addr}; opening window.");
                }
            }

            // Create the main window — server is now listening.
            // When the app had to fall back off 9001, surface the chosen port in
            // the window title so the user (and any docs telling them to visit
            // localhost:9001) can tell at a glance.
            let url = format!("http://localhost:{}", server_port);
            let window_title = if server_port == 9001 {
                "FoldDB - Personal Database".to_string()
            } else {
                format!("FoldDB - Personal Database (port {server_port})")
            };
            let _window =
                WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url.parse().unwrap()))
                    .title(&window_title)
                    .inner_size(1400.0, 900.0)
                    .min_inner_size(1000.0, 700.0)
                    .center()
                    .build()
                    .map_err(|e| format!("Failed to create window: {}", e))?;

            // Background update check — non-blocking, fires event to frontend if update found
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                // Delay so the UI has time to load before we hit the network
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                if let Ok(updater) = handle.updater() {
                    match updater.check().await {
                        Ok(Some(update)) => {
                            let info = UpdateInfo {
                                version: update.version.clone(),
                                current_version: update.current_version.clone(),
                                body: update.body.clone(),
                            };
                            log::info!(
                                "[FoldDB] Update available: {} -> {}",
                                info.current_version, info.version
                            );
                            let _ = handle.emit("update-available", info);
                        }
                        Ok(None) => {
                            log::info!("[FoldDB] App is up to date");
                        }
                        Err(e) => {
                            log::warn!("[FoldDB] Background update check failed: {}", e);
                        }
                    }
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Start the Fold embedded server with lazy database initialization.
///
/// No database is opened at startup — the node is created lazily on the first
/// API request. This means the UI window appears immediately without waiting
/// for sled locks or other DB initialization.
async fn start_fold_server(port: u16) -> Result<EmbeddedServerHandle, String> {
    let data_dir = dirs::home_dir()
        .ok_or_else(|| "Could not determine home directory".to_string())?
        .join(".folddb")
        .join("data");

    std::fs::create_dir_all(&data_dir)
        .map_err(|e| format!("Failed to create data directory: {}", e))?;

    eprintln!("[FoldDB] Using data directory: {:?}", data_dir);

    // Fail fast with a clear message if the DB is locked — the port was
    // already selected by `pick_port` before we got here. Lazy DB init
    // defers sled open to the first API request, so without this probe a
    // stale folddb_server or second app instance produces silent 500s in
    // the UI instead of a proper startup dialog.
    probe_db_unlocked(&data_dir)?;

    // Node identity lives in the Sled `node_identity` tree (see
    // `fold_db_node::identity::IdentityStore`), not in a JSON file on
    // disk. `FoldNode::new` reads or auto-generates the keypair on boot
    // — no pre-load needed here.

    // Set NODE_CONFIG to a writable path so persist_node_config() can save.
    // The bundled .app runs inside a read-only code-signed directory,
    // so the default relative "config/node_config.json" would fail.
    let config_path = dirs::home_dir()
        .ok_or_else(|| "Could not determine home directory".to_string())?
        .join(".folddb")
        .join("node_config.json");
    std::env::set_var("NODE_CONFIG", &config_path);
    eprintln!("[FoldDB] Config path: {:?}", config_path);

    // Set FOLD_UPLOAD_PATH so upload storage uses an absolute writable path.
    // Without this, UploadStorageConfig defaults to the relative "data/uploads"
    // which resolves inside the read-only .app bundle on macOS.
    let upload_path = dirs::home_dir()
        .ok_or_else(|| "Could not determine home directory".to_string())?
        .join(".folddb")
        .join("uploads");
    std::env::set_var("FOLD_UPLOAD_PATH", &upload_path);
    eprintln!("[FoldDB] Upload path: {:?}", upload_path);

    // Set FOLD_CONFIG_DIR so ingestion_config.json is saved/loaded from ~/.folddb/
    // rather than ./config/ which resolves into the read-only .app bundle.
    let config_dir = dirs::home_dir()
        .ok_or_else(|| "Could not determine home directory".to_string())?
        .join(".folddb");
    std::env::set_var("FOLD_CONFIG_DIR", &config_dir);

    // Set FOLDDB_HOME so all path resolution uses ~/.folddb/ instead of
    // relative paths that resolve into the read-only .app bundle.
    std::env::set_var("FOLDDB_HOME", &config_dir);
    eprintln!("[FoldDB] FOLDDB_HOME: {:?}", config_dir);

    // Load node configuration (no DB access — just reads config file)
    let mut config =
        load_node_config(None, None).map_err(|e| format!("Failed to load config: {}", e))?;

    // Always override the storage path to an absolute location, preserving
    // any cloud_sync config. The legacy `{"type": "exemem", ...}` JSON format
    // defaults `path` to relative "data" (fold_db/storage/config.rs:174),
    // which resolves into the read-only .app bundle when the app is launched
    // from /Applications — producing "Read-only file system (os error 30)"
    // when sled tries to open the database.
    let cloud_sync = config.database.cloud_sync.take();
    config.database = match cloud_sync {
        Some(cs) => DatabaseConfig::with_cloud_sync(data_dir, cs),
        None => DatabaseConfig::local(data_dir),
    };

    config.schema_service_url = Some(fold_db_node::endpoints::schema_service_url());

    // Build NodeManagerConfig — no FoldNode created yet
    let node_manager_config = NodeManagerConfig {
        base_config: config,
    };

    eprintln!("[FoldDB] Starting server with lazy database initialization...");

    let handle = start_embedded_server_lazy(node_manager_config, port)
        .await
        .map_err(|e| format!("Failed to start server: {}", e))?;

    // Wait for the HTTP server to actually be listening before returning.
    // With lazy init the handle returns before actix binds the port.
    for i in 0..50 {
        if std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).is_ok() {
            eprintln!(
                "[FoldDB] Server is listening on port {} (took {}ms)",
                port,
                i * 50
            );
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    Ok(handle)
}

/// Pick the first free TCP port starting at `preferred`, trying at most
/// `max_attempts` consecutive ports.
///
/// We bind-and-release rather than just asking the OS for an ephemeral
/// port so that we stay predictable (9001 the vast majority of the time,
/// 9002–9010 on the rare conflict). A tiny race exists between our
/// release and the eventual actix bind — acceptable because the worst
/// case is a retry via the existing startup_error dialog.
fn pick_port(preferred: u16, max_attempts: u16) -> Result<u16, String> {
    for offset in 0..max_attempts {
        let port = preferred.saturating_add(offset);
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return Ok(port);
        }
    }
    let last = preferred.saturating_add(max_attempts.saturating_sub(1));
    Err(format!(
        "No free TCP port found in range {preferred}..={last}.\n\nOther services on this machine are occupying every port FoldDB would try. Quit anything listening on 127.0.0.1:{preferred} (and the nine ports above it) and relaunch."
    ))
}

/// Verify no other process holds the sled database lock.
///
/// Sled grabs an fcntl lock on `<data_dir>/db` when it opens. With lazy DB
/// init, that doesn't happen until the first API request — so a stale
/// `folddb_server` or second app instance produces silent 500s in the UI.
/// We replicate sled's lock probe here (fs2 uses the same fcntl locks) so
/// the startup dialog fires with an actionable message.
fn probe_db_unlocked(data_dir: &std::path::Path) -> Result<(), String> {
    use fs2::FileExt;
    let lock_file = data_dir.join("db");
    if !lock_file.exists() {
        // First run — nothing to check.
        return Ok(());
    }
    let f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_file)
        .map_err(|e| format!("Failed to open database lock file at {lock_file:?}: {e}"))?;
    match f.try_lock_exclusive() {
        Ok(()) => {
            // Release so sled can take the lock when lazy init runs.
            // Use fs2's unlock explicitly — std::fs::File::unlock was only
            // stabilized in 1.89 but this crate's MSRV is 1.77.2.
            let _ = FileExt::unlock(&f);
            Ok(())
        }
        Err(_) => Err(format!(
            "Another FoldDB process is already using the database at {data_dir:?}.\n\nPlease quit any running FoldDB app or dev server (e.g. `./run.sh --local`, `folddb_server`) and try again."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_tmp_dir(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "fold-app-test-{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn pick_port_returns_preferred_when_free() {
        // Ask the OS for a free port, release, confirm pick_port picks it.
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        assert_eq!(pick_port(port, 10).unwrap(), port);
    }

    #[test]
    fn pick_port_falls_back_when_preferred_occupied() {
        // Occupy a port, confirm pick_port returns a higher one within range.
        let occupied = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = occupied.local_addr().unwrap().port();
        let picked = pick_port(port, 20).unwrap();
        assert_ne!(picked, port, "should not pick the occupied port");
        assert!(
            picked > port && picked < port + 20,
            "picked port {picked} outside range"
        );
        drop(occupied);
    }

    #[test]
    fn pick_port_errors_when_all_attempts_occupied() {
        // Occupy three consecutive ports; pick_port with max_attempts=3 should fail.
        let l1 = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let base = l1.local_addr().unwrap().port();
        // Grabbing sequential ports isn't guaranteed, so bind them explicitly.
        let l2 = match std::net::TcpListener::bind(("127.0.0.1", base + 1)) {
            Ok(l) => l,
            Err(_) => return, // someone else has it — skip rather than flake
        };
        let l3 = match std::net::TcpListener::bind(("127.0.0.1", base + 2)) {
            Ok(l) => l,
            Err(_) => return,
        };
        let err = pick_port(base, 3).expect_err("all three ports are occupied");
        assert!(err.contains("No free TCP port"), "unexpected error: {err}");
        drop((l1, l2, l3));
    }

    #[test]
    fn probe_db_unlocked_ok_when_no_lock_file() {
        let tmp = unique_tmp_dir("no-lock");
        assert!(probe_db_unlocked(&tmp).is_ok());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn probe_db_unlocked_ok_when_lock_file_is_free() {
        let tmp = unique_tmp_dir("free-lock");
        std::fs::File::create(tmp.join("db")).unwrap();
        assert!(probe_db_unlocked(&tmp).is_ok());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // NOTE: the locked-by-another-process case is covered by manual testing
    // (launch `./run.sh --local`, then start the Tauri app — the startup
    // dialog should fire). It's not an automated test because fcntl locks
    // don't conflict within the same process, so a unit test would need a
    // real subprocess holding the lock, which adds significant flakiness
    // risk for marginal coverage gain.
}
