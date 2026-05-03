use clap::Parser;
use fold_db_node::{
    fold_node::config::{load_node_config, NodeConfig},
    observability_setup::init_node_with_web,
    server::{
        http_server::FoldHttpServer,
        node_manager::{NodeManager, NodeManagerConfig},
        startup::StartupCtx,
    },
};
use std::path::PathBuf;
use tokio::task::JoinSet;

/// Dev CLI default HTTP port. Distinct from the bundled Tauri app's 9001
/// so running `./run.sh` while FoldDB.app is open doesn't collide. The
/// prod Tauri binary still uses 9001 (with 9002..=9010 fallback); dev
/// gets 9101..=9199 via run.sh's auto-slot logic.
const DEFAULT_DEV_HTTP_PORT: u16 = 9101;

/// Command line options for the HTTP server binary.
///
/// The HTTP server is now stateless - it accepts any user_hash from the
/// X-User-Hash header on each request, matching the Lambda implementation.
#[derive(Parser, Debug)]
#[command(
    author,
    version = env!("FOLDDB_BUILD_VERSION"),
    about = "FoldDB Server — run locally, open the UI at http://localhost:9101"
)]
struct Cli {
    /// Port for the HTTP server
    #[arg(long, default_value_t = DEFAULT_DEV_HTTP_PORT)]
    port: u16,

    /// Data directory (default: ~/.folddb/data)
    #[arg(long)]
    data_dir: Option<PathBuf>,

    /// Schema service URL (default: production schema service)
    #[arg(long)]
    schema_service_url: Option<String>,

    /// Run in demo mode with isolated data/config directories
    #[arg(long)]
    demo: bool,
}

/// Resolve the default data directory: $FOLDDB_HOME/data (or $FOLDDB_HOME/demo-data in demo mode)
fn default_data_dir(demo: bool) -> PathBuf {
    let subdir = if demo { "demo-data" } else { "data" };
    fold_db_node::utils::paths::folddb_home()
        .unwrap_or_else(|_| PathBuf::from(".folddb"))
        .join(subdir)
}

/// Resolve the default config directory: $FOLDDB_HOME/config (or $FOLDDB_HOME/demo-config in demo mode)
fn default_config_dir(demo: bool) -> PathBuf {
    let subdir = if demo { "demo-config" } else { "config" };
    fold_db_node::utils::paths::folddb_home()
        .unwrap_or_else(|_| PathBuf::from(".folddb"))
        .join(subdir)
}

/// Check if a user-provided or env-var config file exists.
fn config_file_exists() -> bool {
    let path = std::env::var("NODE_CONFIG").unwrap_or_else(|_| {
        fold_db_node::utils::paths::folddb_home()
            .map(|h| {
                h.join("config")
                    .join("node_config.json")
                    .to_string_lossy()
                    .to_string()
            })
            .unwrap_or_else(|_| "config/node_config.json".to_string())
    });
    std::path::Path::new(&path).exists()
}

/// Resolved startup info for the user-facing banner.
struct StartupInfo {
    label: &'static str,
    data_path: PathBuf,
    config_path: PathBuf,
    schema_service_url: Option<String>,
    had_config_file: bool,
}

/// Apply CLI/default overrides to the loaded config and set the env vars
/// downstream code relies on. Runs in both the no-config-file and
/// config-file paths so they share dir-creation, `FOLD_CONFIG_DIR`, and
/// `FOLD_STORAGE_PATH` handling — the asymmetry between the two arms was
/// the spot most likely to grow a bug.
///
/// `FOLD_STORAGE_PATH` is set in both arms intentionally:
/// `fold_node::operation_processor::admin_ops` reads it directly with a
/// fallback to the relative `"data"` string, which collides between
/// multi-node setups. The lazy `NodeManager` path also sets it on first
/// per-user node creation, but it must be live before any operation that
/// bypasses NodeManager runs.
fn setup_config_environment(
    config: &mut NodeConfig,
    data_dir: Option<PathBuf>,
    schema_service_url_override: Option<String>,
    demo: bool,
    has_config_file: bool,
) -> std::io::Result<StartupInfo> {
    let config_path = default_config_dir(demo);

    if !has_config_file {
        // Zero-config: derive both the local Sled path and schema URL from
        // CLI flags or the FoldDB defaults. Anything on the freshly-loaded
        // NodeConfig is uninteresting because no file backed it.
        let data_path = data_dir.unwrap_or_else(|| default_data_dir(demo));
        std::fs::create_dir_all(&data_path)?;
        config.database = fold_db::storage::config::DatabaseConfig::local(data_path.clone());
        config.storage_path = Some(data_path);
        config.schema_service_url = Some(
            schema_service_url_override.unwrap_or_else(fold_db_node::endpoints::schema_service_url),
        );
    } else {
        // Config file exists — honour explicit CLI overrides only.
        //
        // `--data-dir` overrides only the local Sled path. If the config
        // file declares cloud sync, that configuration is preserved — we're
        // pointing the local backing store somewhere else, not turning
        // cloud sync off. Both `database.path` and `storage_path` are
        // updated so `get_storage_path()` and the FOLD_STORAGE_PATH env
        // propagation below see the same value.
        if let Some(dir) = data_dir {
            config.database.path = dir.clone();
            config.storage_path = Some(dir);
        }
        if let Some(url) = schema_service_url_override {
            config.schema_service_url = Some(url);
        }
    }

    std::fs::create_dir_all(&config_path)?;
    std::env::set_var("FOLD_CONFIG_DIR", &config_path);
    std::env::set_var("FOLD_STORAGE_PATH", config.get_storage_path());

    let label = match (has_config_file, demo) {
        (true, _) => "FoldDB Server (config file detected)",
        (false, true) => "FoldDB Server [DEMO]",
        (false, false) => "FoldDB Server",
    };

    Ok(StartupInfo {
        label,
        data_path: config.get_storage_path(),
        config_path,
        schema_service_url: config.schema_service_url.clone(),
        had_config_file: has_config_file,
    })
}

/// Main entry point for the FoldDB HTTP server.
///
/// This is a STATELESS HTTP server - user identity comes from the X-User-Hash
/// header on each incoming request, just like the Lambda implementation.
///
/// # Architecture
///
/// The server uses lazy per-user node initialization:
/// - On startup: Only configuration is loaded; no per-user state is touched.
/// - On first request for a user: Node is created with user context
/// - Subsequent requests: Node is cached and reused
///
/// # Command-Line Arguments
///
/// * `--port <PORT>` - Port for the HTTP server (default: 9101)
/// * `--data-dir <PATH>` - Data directory (default: ~/.folddb/data)
/// * `--schema-service-url <URL>` - URL of the schema service
///
/// # Environment Variables
///
/// * `NODE_CONFIG` - Path to the node configuration file (default: config/node_config.json)
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Cli {
        port: http_port,
        data_dir,
        schema_service_url,
        demo,
    } = Cli::parse();

    // Load node configuration
    let mut config = load_node_config(None, None)?;
    let has_config_file = config_file_exists();

    let info = setup_config_environment(
        &mut config,
        data_dir,
        schema_service_url,
        demo,
        has_config_file,
    )?;

    println!("{}", info.label);
    println!("  Data:   {}", info.data_path.display());
    if !info.had_config_file {
        println!("  Config: {}", info.config_path.display());
    }
    if let Some(ref url) = info.schema_service_url {
        println!("  Schema: {}", url);
    }
    println!("  UI:     http://localhost:{}", http_port);
    println!();

    if fold_db_node::handlers::admin::test_admin_enabled() {
        println!("⚠️  TEST-ADMIN MODE ENABLED (FOLDDB_ENABLE_TEST_ADMIN=1)");
        println!("   /api/test-admin/* endpoints are unlocked. DO NOT USE IN PRODUCTION.");
        println!();
    }

    // Initialize observability stack: FMT (file) + RELOAD + RING + WEB
    // + OTel. The guard MUST be held for the lifetime of the process —
    // dropping it stops the FMT worker mid-flush and may lose
    // buffered log lines.
    //
    // Phase 3 / T5 wires WEB here so `/api/logs/stream` can subscribe
    // to a tracing-native broadcast. RING handles `/api/logs`,
    // RELOAD handles `PUT /api/logs/level`. Legacy
    // `LoggingSystem::init_with_fallback` still runs inside
    // `FoldHttpServer::new` so call sites that haven't migrated to
    // `tracing::*` keep emitting through the bridge.
    let obs_guard =
        init_node_with_web("fold_db_node").map_err(|e| -> Box<dyn std::error::Error> {
            format!("Failed to initialize observability stack: {}", e).into()
        })?;
    let obs_handles = obs_guard.handles();
    // The guard owns the FMT worker; leak it for the process lifetime
    // so the daemon — which only exits via SIGTERM — keeps the writer
    // draining instead of stopping it mid-flush.
    let _obs_guard: &'static fold_db_node::observability_setup::NodeObsGuard =
        Box::leak(Box::new(obs_guard));

    // Create NodeManager — nodes are created lazily per-user on first request.
    let node_manager_config = NodeManagerConfig {
        base_config: config,
    };
    let node_manager = NodeManager::new(node_manager_config);

    // Phase 1: deterministic, awaited resource initialization. Every
    // subsystem a background worker could need is fully initialized before
    // `boot` returns. See `server::startup` for the rationale.
    let ctx = StartupCtx::boot(node_manager, Some(obs_handles)).await?;

    // Phase 2: tracked spawns. Workers take `Arc<StartupCtx>` so they cannot
    // observe uninitialized state. The JoinSet is held until `run` returns
    // so workers stay alive for the server's lifetime.
    let mut tasks = JoinSet::new();
    ctx.spawn_workers(&mut tasks);

    // Phase 3: bind and serve.
    let bind_address = format!("127.0.0.1:{}", http_port);
    let http_server = FoldHttpServer::new(ctx, &bind_address);
    http_server
        .run()
        .await
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

    drop(tasks);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Cli, DEFAULT_DEV_HTTP_PORT};
    use clap::Parser;

    #[test]
    fn defaults() {
        let cli = Cli::parse_from(["test"]);
        assert_eq!(cli.port, DEFAULT_DEV_HTTP_PORT);
        assert!(cli.data_dir.is_none());
        assert!(cli.schema_service_url.is_none());
    }

    #[test]
    fn custom_port() {
        let cli = Cli::parse_from(["test", "--port", "8000"]);
        assert_eq!(cli.port, 8000);
    }

    #[test]
    fn with_data_dir() {
        let cli = Cli::parse_from(["test", "--data-dir", "/tmp/folddb"]);
        assert_eq!(cli.data_dir, Some(std::path::PathBuf::from("/tmp/folddb")));
    }

    #[test]
    fn with_schema_service() {
        let cli = Cli::parse_from(["test", "--schema-service-url", "http://localhost:9002"]);
        assert_eq!(
            cli.schema_service_url,
            Some("http://localhost:9002".to_string())
        );
    }

    #[test]
    fn demo_flag() {
        let cli = Cli::parse_from(["test", "--demo"]);
        assert!(cli.demo);
    }

    #[test]
    fn demo_flag_default_false() {
        let cli = Cli::parse_from(["test"]);
        assert!(!cli.demo);
    }

    #[test]
    fn demo_data_dir() {
        let normal = super::default_data_dir(false);
        let demo = super::default_data_dir(true);
        assert!(normal.ends_with("data"));
        assert!(demo.ends_with("demo-data"));
    }

    #[test]
    fn demo_config_dir() {
        let normal = super::default_config_dir(false);
        let demo = super::default_config_dir(true);
        assert!(normal.ends_with("config"));
        assert!(demo.ends_with("demo-config"));
    }

    /// Both arms of `setup_config_environment` must leave `FOLD_CONFIG_DIR`
    /// and `FOLD_STORAGE_PATH` set to paths that match the resolved config
    /// — that's the asymmetry the refactor was meant to eliminate. We run
    /// both scenarios in one test and serialize against the other env-var
    /// tests via a process-wide mutex (env mutation is global).
    #[test]
    fn setup_config_environment_keeps_env_vars_consistent() {
        use fold_db_node::fold_node::config::NodeConfig;
        use std::sync::Mutex;

        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let tmp = tempfile::tempdir().expect("tempdir");
        let prev_folddb_home = std::env::var("FOLDDB_HOME").ok();
        let prev_config_dir = std::env::var("FOLD_CONFIG_DIR").ok();
        let prev_storage_path = std::env::var("FOLD_STORAGE_PATH").ok();
        std::env::set_var("FOLDDB_HOME", tmp.path());

        // Arm 1: no config file. Helper picks default_data_dir + default_config_dir,
        // creates them, and propagates the storage path.
        let mut config = NodeConfig::default();
        let info = super::setup_config_environment(
            &mut config,
            /* data_dir */ None,
            /* schema_url */ None,
            /* demo */ false,
            /* has_config_file */ false,
        )
        .expect("zero-config setup");

        let expected_data = tmp.path().join("data");
        let expected_config = tmp.path().join("config");
        assert_eq!(info.data_path, expected_data);
        assert_eq!(info.config_path, expected_config);
        assert!(expected_data.is_dir(), "data dir must be created");
        assert!(expected_config.is_dir(), "config dir must be created");
        assert_eq!(
            std::path::PathBuf::from(std::env::var("FOLD_CONFIG_DIR").unwrap()),
            expected_config,
        );
        assert_eq!(
            std::path::PathBuf::from(std::env::var("FOLD_STORAGE_PATH").unwrap()),
            expected_data,
        );
        assert_eq!(config.get_storage_path(), expected_data);
        assert!(
            config.schema_service_url.is_some(),
            "zero-config must default the schema service URL",
        );

        // Arm 2: config file already exists. CLI passes a fresh data dir;
        // helper must keep `database.path`/`storage_path` and the env vars
        // in lockstep, and FOLD_STORAGE_PATH must reflect the override.
        let cli_data = tmp.path().join("override-data");
        let mut config = NodeConfig::default();
        config.schema_service_url = Some("https://from-config-file.example".to_string());
        let info = super::setup_config_environment(
            &mut config,
            Some(cli_data.clone()),
            /* schema_url */ None,
            /* demo */ false,
            /* has_config_file */ true,
        )
        .expect("config-file setup");

        assert_eq!(info.data_path, cli_data);
        assert_eq!(info.config_path, expected_config);
        assert_eq!(config.database.path, cli_data);
        assert_eq!(config.storage_path.as_deref(), Some(cli_data.as_path()));
        assert_eq!(
            std::path::PathBuf::from(std::env::var("FOLD_CONFIG_DIR").unwrap()),
            expected_config,
        );
        assert_eq!(
            std::path::PathBuf::from(std::env::var("FOLD_STORAGE_PATH").unwrap()),
            cli_data,
        );
        // CLI didn't pass --schema-service-url, so the file's URL is preserved.
        assert_eq!(
            config.schema_service_url.as_deref(),
            Some("https://from-config-file.example"),
        );

        // Restore env vars to keep the rest of the test process clean.
        match prev_folddb_home {
            Some(v) => std::env::set_var("FOLDDB_HOME", v),
            None => std::env::remove_var("FOLDDB_HOME"),
        }
        match prev_config_dir {
            Some(v) => std::env::set_var("FOLD_CONFIG_DIR", v),
            None => std::env::remove_var("FOLD_CONFIG_DIR"),
        }
        match prev_storage_path {
            Some(v) => std::env::set_var("FOLD_STORAGE_PATH", v),
            None => std::env::remove_var("FOLD_STORAGE_PATH"),
        }
    }
}
