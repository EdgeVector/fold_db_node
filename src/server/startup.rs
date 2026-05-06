//! Phased server boot.
//!
//! Phase 1 — [`StartupCtx::boot`] runs deterministically and awaits every
//! resource a background task could need (Sled pool, schemas, discovery,
//! upload storage, ingestion service, progress tracker, etc).
//!
//! Phase 2 — [`StartupCtx::spawn_workers`] hands an `Arc<StartupCtx>` to
//! each background worker. Workers can only be constructed *after* `boot`
//! returns, so they cannot observe uninitialized state. The borrow
//! checker enforces the ordering — moving the spawns earlier would not
//! compile because there is no ctx to clone yet.
//!
//! Phase 3 — the binary calls `FoldHttpServer::new(ctx).run()` which
//! binds Actix and starts serving requests.

use std::path::PathBuf;
use std::sync::Arc;

use actix_web::web;
use fold_db::error::FoldDbResult;
use fold_db::progress::ProgressTracker;
use fold_db::storage::{SledPool, UploadStorage};
use tokio::sync::RwLock;
use tokio::task::JoinSet;
use tracing::Instrument;

use crate::fold_node::llm_query::LlmQueryState;
use crate::ingestion::apple_import::sync_scheduler::{create_sync_config_state, SyncConfigState};
use crate::ingestion::batch_controller::{create_batch_controller_map, BatchControllerMap};
use crate::ingestion::ingestion_service::IngestionService;
use crate::ingestion::service_state::IngestionServiceState;
use crate::observability_setup::ObsHandles;
use crate::server::discovery_config::DiscoveryConfig;
use crate::server::http_server::{session_token_needs_refresh, AppState};
use crate::server::node_manager::NodeManager;

/// Newtype wrapper around the per-server config directory so Actix `web::Data`
/// extractors are unambiguous (a bare `web::Data<PathBuf>` would collide with
/// any other `PathBuf`-shaped state).
pub struct ConfigDir(pub PathBuf);

impl ConfigDir {
    pub fn as_path(&self) -> &std::path::Path {
        &self.0
    }
}

/// All resources required to run the server, with init guaranteed complete.
///
/// Construction goes through [`StartupCtx::boot`]; never build one by hand.
/// Workers, handlers, and the HTTP server take `&Arc<StartupCtx>` so they
/// cannot race a half-initialized state.
pub struct StartupCtx {
    /// Lazily creates per-user FoldNodes. Already wrapped in `Arc` so the
    /// boot path and Actix `web::Data<AppState>` share the same NodeManager.
    pub node_manager: Arc<NodeManager>,
    /// The single Sled file-lock holder for the lifetime of the process.
    /// Eagerly initialized in `boot()` so the bootstrap-resume worker can
    /// rely on it being `Some` instead of racing the lazy creation path.
    pub sled_pool: Arc<SledPool>,
    /// Resolved Sled storage directory. Captured once at boot from
    /// `NodeConfig::get_storage_path()` so server-side callers don't
    /// async-acquire the manager's RwLock just to learn the path.
    pub storage_path: PathBuf,
    /// `Some((api_url, api_key))` if a previous run wrote a bootstrap
    /// marker before crashing mid-download. Captured at boot so workers
    /// don't reread the marker file.
    pub bootstrap_pending: Option<(String, String)>,
    /// Observability handles, owned for the process lifetime by the
    /// binary's leaked `NodeObsGuard`. `None` for embedded callers that
    /// did not initialize the tracing pipeline.
    pub obs: Option<ObsHandles>,

    /// Pre-wrapped Actix state — workers also clone these via `Arc` semantics.
    pub app_state: web::Data<AppState>,
    pub upload_storage: web::Data<UploadStorage>,
    pub progress_tracker: web::Data<ProgressTracker>,
    pub ingestion: web::Data<IngestionServiceState>,
    pub apple_sync_config: web::Data<SyncConfigState>,
    pub batch_controllers: web::Data<BatchControllerMap>,
    pub llm_query: web::Data<LlmQueryState>,
    /// Per-server config directory. Threaded to ingestion routes via
    /// `web::Data` so two nodes in one test process don't share state.
    pub config_dir: web::Data<ConfigDir>,
}

impl StartupCtx {
    /// Phase 1. Run all I/O that must complete before the HTTP server binds.
    ///
    /// `get_or_init_sled_pool` MUST run before `bootstrap_pending` is captured
    /// so the resume worker has a guaranteed pool to use. Remaining steps are
    /// independent and ordered only for log readability.
    pub async fn boot(
        node_manager: NodeManager,
        obs: Option<ObsHandles>,
    ) -> FoldDbResult<Arc<Self>> {
        // Snapshot the path inputs the boot pipeline needs before wrapping
        // node_manager in Arc — `config_dir` and `upload_path` flow into the
        // upload-storage and ingestion-service builders below; `storage_path`
        // is the resolved Sled directory exposed on `StartupCtx`.
        let (config_dir_path, upload_path, storage_path) = {
            let cfg = node_manager.get_full_config().await;
            (
                cfg.config_dir.clone(),
                cfg.upload_path.clone(),
                cfg.base_config.get_storage_path(),
            )
        };

        let node_manager = Arc::new(node_manager);

        // Eager pool init — fixes the bootstrap-resume `None` pool race.
        let sled_pool = node_manager.get_or_init_sled_pool().await;

        // Idempotent migration for buggy installs from the pre-bootstrap-endpoint
        // CLI wizard: re-encrypt plaintext identities under the keychain master
        // key and synthesize a missing onboarding marker if an identity exists.
        // After one boot the conditions stop matching; safe to run every boot.
        if let Err(e) = migrate_legacy_identity_state(&sled_pool) {
            // Don't block boot — surface the error so it's visible in logs and
            // the admin can decide. Worst case the user re-runs onboarding.
            tracing::warn!(
                target: "fold_node::http_server",
                error = %e,
                "Legacy identity-state migration failed (non-fatal)"
            );
        }

        // Capture the bootstrap marker after the pool is live so the resume
        // worker can use the captured pool unconditionally.
        let bootstrap_pending = crate::handlers::auth::check_bootstrap_pending();

        // Schemas: best-effort, non-fatal.
        load_schemas_if_configured(&node_manager).await;

        // Discovery: just log resolution state for operators.
        match DiscoveryConfig::resolve(&node_manager).await {
            Some(cfg) => tracing::info!("Discovery configuration resolved: url={}", cfg.url),
            None => {
                tracing::info!("Discovery configuration not yet available (no identity registered)")
            }
        }

        let upload_storage = build_upload_storage(upload_path.clone());
        tracing::info!(
            target: "fold_node::http_server",
            "Upload storage initialized: {}",
            if upload_storage.is_local() { "Local" } else { "S3" }
        );

        let app_state = web::Data::new(AppState {
            node_manager: Arc::clone(&node_manager),
        });
        let upload_storage = web::Data::new(upload_storage);
        let progress_tracker = web::Data::new(fold_db::progress::create_tracker().await);
        let ingestion = web::Data::new(RwLock::new(
            IngestionService::from_config_dir(&config_dir_path)
                .ok()
                .map(Arc::new),
        ));
        let apple_sync_config = web::Data::new(create_sync_config_state());
        let batch_controllers = web::Data::new(create_batch_controller_map());
        let llm_query = web::Data::new(LlmQueryState::new(config_dir_path.clone()));
        let config_dir = web::Data::new(ConfigDir(config_dir_path));

        Ok(Arc::new(Self {
            node_manager,
            sled_pool,
            storage_path,
            bootstrap_pending,
            obs,
            app_state,
            upload_storage,
            progress_tracker,
            ingestion,
            apple_sync_config,
            batch_controllers,
            llm_query,
            config_dir,
        }))
    }

    /// Phase 2. Spawn every background worker onto `tasks`. Each worker
    /// takes `Arc<Self>` so the borrow checker forbids constructing them
    /// before `boot()` returns.
    pub fn spawn_workers(self: &Arc<Self>, tasks: &mut JoinSet<()>) {
        if let Some((api_url, api_key)) = self.bootstrap_pending.clone() {
            let ctx = Arc::clone(self);
            tasks.spawn(bootstrap_resume(ctx, api_url, api_key).in_current_span());
        }

        let ctx = Arc::clone(self);
        tasks.spawn(token_refresh(ctx).in_current_span());

        let ctx = Arc::clone(self);
        tasks.spawn(apple_sync(ctx).in_current_span());
    }
}

/// Default upper bound on `load_schemas_if_configured`. Independent of the
/// inner `schema_service_client` per-request budget (120s + up to 3 retries) —
/// boot must not pay that worst case.
const DEFAULT_SCHEMA_PRELOAD_TIMEOUT_SECS: u64 = 10;

/// Resolve the schema-preload timeout. Operators override via
/// `FOLDDB_SCHEMA_PRELOAD_TIMEOUT_SECS`; an unparseable value falls back to
/// the default with a warn so the typo is visible in logs.
fn schema_preload_timeout() -> std::time::Duration {
    let secs = match std::env::var("FOLDDB_SCHEMA_PRELOAD_TIMEOUT_SECS") {
        Ok(raw) => match raw.parse::<u64>() {
            Ok(s) => s,
            Err(_) => {
                tracing::warn!(
                    target: "fold_node::database",
                    raw = %raw,
                    default_secs = DEFAULT_SCHEMA_PRELOAD_TIMEOUT_SECS,
                    "FOLDDB_SCHEMA_PRELOAD_TIMEOUT_SECS unparseable; using default"
                );
                DEFAULT_SCHEMA_PRELOAD_TIMEOUT_SECS
            }
        },
        Err(_) => DEFAULT_SCHEMA_PRELOAD_TIMEOUT_SECS,
    };
    std::time::Duration::from_secs(secs)
}

/// Best-effort schema preload from the configured schema service. Failures
/// (including the outer timeout) are logged but not propagated — the server
/// still starts; schemas just aren't cached.
async fn load_schemas_if_configured(node_manager: &Arc<NodeManager>) {
    let base_config = node_manager.get_base_config().await;
    let Some(url) = base_config.schema_service_url.clone() else {
        return;
    };

    if crate::fold_node::node::FoldNode::is_test_schema_service(&url) {
        tracing::info!(
            target: "fold_node::database",
            "Mock schema service detected ({}). Skipping automatic schema loading. Schemas must be loaded manually in tests.",
            url
        );
        return;
    }

    let timeout = schema_preload_timeout();
    tracing::info!(
        target: "fold_node::database",
        url = %url,
        timeout_secs = timeout.as_secs(),
        "Loading schemas from schema service..."
    );

    let client = crate::fold_node::SchemaServiceClient::new(&url);
    match tokio::time::timeout(timeout, client.list_schemas()).await {
        Ok(Ok(schemas)) => tracing::info!(
            target: "fold_node::database",
            count = schemas.len(),
            "Loaded schemas from schema service"
        ),
        Ok(Err(e)) => tracing::error!(
            target: "fold_node::database",
            url = %url,
            error = %e,
            "Failed to load schemas from schema service. Server will start but no schemas will be available."
        ),
        Err(_) => tracing::warn!(
            target: "fold_node::database",
            url = %url,
            timeout_secs = timeout.as_secs(),
            "Schema preload timed out (non-fatal); continuing boot without preloaded schemas"
        ),
    }
}

fn build_upload_storage(upload_path: PathBuf) -> UploadStorage {
    UploadStorage::local(upload_path)
}

async fn bootstrap_resume(ctx: Arc<StartupCtx>, api_url: String, api_key: String) {
    tracing::info!(
        target: "fold_node::http_server",
        "Found interrupted bootstrap — resuming cloud data download"
    );
    if let Err(e) = crate::handlers::auth::resume_bootstrap(
        &api_url,
        &api_key,
        &ctx.node_manager,
        Arc::clone(&ctx.sled_pool),
    )
    .await
    {
        tracing::error!("Bootstrap resume failed: {}", e);
    }
    // Drop any cached node so the next request rebuilds against the now-
    // restored Sled state. Safe even on failure: the next request would
    // otherwise serve from a node cached against pre-restore data.
    ctx.node_manager.invalidate_all_nodes().await;
}

/// Load Exemem credentials with the keychain bounded by a 5s timeout and
/// a `spawn_blocking` hop, so a locked OS keychain can never pin a tokio
/// worker thread. The error class is encoded in the `Err` string so the
/// caller can preserve a single warn line while operators still grep the
/// distinct messages: timeout, blocking-task panic, or load failure.
async fn load_credentials_bounded() -> Result<Option<crate::keychain::ExememCredentials>, String> {
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::task::spawn_blocking(crate::keychain::load_credentials),
    )
    .await
    .map_err(|_| "load timed out after 5s".to_string())?
    .map_err(|e| format!("load task panicked: {}", e))?
}

async fn token_refresh(ctx: Arc<StartupCtx>) {
    let creds = match load_credentials_bounded().await {
        Ok(Some(c)) => c,
        Ok(None) => {
            tracing::info!("No Exemem credentials stored; skipping startup refresh");
            return;
        }
        Err(e) => {
            tracing::warn!("Keychain {} (non-fatal); skipping startup refresh", e);
            return;
        }
    };

    const MIN_REMAINING_SECS: i64 = 12 * 60 * 60;
    let now = chrono::Utc::now().timestamp();
    match session_token_needs_refresh(&creds.session_token, now, MIN_REMAINING_SECS) {
        Ok(false) => {
            tracing::info!(
                "Exemem session token still valid (>12h remaining); skipping startup refresh"
            );
            return;
        }
        Ok(true) => {
            tracing::info!("Exemem session token near expiry; refreshing...");
        }
        Err(e) => {
            tracing::warn!("Unable to parse stored session token ({}); refreshing", e);
        }
    }

    match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        crate::server::routes::auth::refresh_session_token(&ctx.app_state),
    )
    .await
    {
        Ok(Ok(_)) => tracing::info!("Exemem session token refreshed successfully"),
        Ok(Err(e)) => tracing::warn!("Exemem session token refresh failed (non-fatal): {}", e),
        Err(_) => tracing::warn!("Exemem session token refresh timed out after 10s (non-fatal)"),
    }
}

/// Idempotent boot-time migration for installs created by the pre-bootstrap
/// CLI wizard, which (a) stored the identity plaintext (the master key was
/// never minted because IdentityStore::open was never called from the
/// in-process pool) and (b) sometimes failed to write `.onboarding_complete`
/// on the cloud-disabled path.
///
/// Conservative by design: never rotates the identity, never grants
/// "setup complete" to a never-set-up node. Sub-step 1 only runs when the
/// `os-keychain` feature is on AND the stored value lacks the `ENC:` prefix;
/// sub-step 2 only runs when an identity exists AND the marker is absent.
/// After one boot, neither condition matches and the function no-ops.
fn migrate_legacy_identity_state(pool: &Arc<SledPool>) -> Result<(), String> {
    let raw = crate::identity::peek_raw_identity_value(pool)?;
    let Some(raw_value) = raw else {
        // No identity persisted yet — bootstrap will be the first writer.
        // Nothing to migrate; nothing to back-fill.
        return Ok(());
    };

    // ---- Sub-step 1: re-encrypt plaintext identity ----
    #[cfg(feature = "os-keychain")]
    {
        if !raw_value.starts_with("ENC:") {
            // Mint the master key explicitly first — upstream's
            // resolve_master_key returns Ok(None) on the no-encrypted-blob
            // path, so without this call set() would re-write plaintext.
            // initialize_master_key is idempotent.
            crate::secure_store::initialize_master_key()?;
            // identity::open() now resolves the master key (FOLDDB_MASTER_KEY
            // first, then the keychain we just minted into) and returns a
            // store that encrypts on `set`. The on-disk value flips from
            // plaintext to ENC:<base64> blob with the SAME identity bytes —
            // open() returns the plaintext as-is, set() re-writes encrypted.
            let store = crate::identity::open(Arc::clone(pool))?;
            if let Some(id) = store.get()? {
                store.set(&id)?;
                tracing::info!(
                    target: "fold_node::http_server",
                    "Migrated plaintext identity to encrypted (legacy CLI-wizard install)"
                );
            }
        }
    }
    #[cfg(not(feature = "os-keychain"))]
    {
        // Without the keychain feature there's nothing to encrypt under.
        // The plaintext form is the on-disk format for this build, so nothing
        // to do for sub-step 1.
        let _ = raw_value;
    }

    // ---- Sub-step 2: synthesize missing marker ----
    let marker_path = match crate::utils::paths::folddb_home() {
        Ok(home) => home.join("data").join(".onboarding_complete"),
        Err(e) => {
            // FOLDDB_HOME is unreachable — nothing we can do, but identity
            // re-encryption above already succeeded so we don't fail boot.
            return Err(format!("Cannot resolve FOLDDB_HOME for marker: {e}"));
        }
    };
    if !marker_path.exists() {
        if let Some(parent) = marker_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Cannot create data dir for marker: {e}"))?;
        }
        crate::sensitive_io::write_sensitive(&marker_path, b"1")
            .map_err(|e| format!("Cannot write synthesized marker: {e}"))?;
        tracing::info!(
            target: "fold_node::http_server",
            "Synthesized missing onboarding marker (identity exists)"
        );
    }

    Ok(())
}

async fn apple_sync(ctx: Arc<StartupCtx>) {
    crate::server::routes::apple_import::run_sync_scheduler(
        ctx.apple_sync_config.get_ref().clone(),
        ctx.app_state.clone(),
        ctx.ingestion.clone(),
        ctx.progress_tracker.clone(),
        ctx.upload_storage.clone(),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fold_node::config::NodeConfig;
    use crate::server::node_manager::{NodeManager, NodeManagerConfig};

    fn test_config(path: &std::path::Path) -> NodeManagerConfig {
        let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
        NodeManagerConfig {
            base_config: NodeConfig::new(path.to_path_buf())
                .with_schema_service_url("test://mock")
                .with_seed_identity(crate::identity::identity_from_keypair(&keypair)),
            config_dir: path.join("config"),
            upload_path: path.join("uploads"),
        }
    }

    /// Regression for the bootstrap-resume `None` pool race.
    ///
    /// Before phased boot, the resume task spawned at the top of
    /// `FoldHttpServer::run()` before any callsite had initialized the
    /// SledPool. The task then called `get_sled_pool()` (a non-initializing
    /// read), saw `None`, and silently exited. After interrupted bootstraps
    /// users could end up with an empty local DB and no surfaced error.
    ///
    /// `boot()` MUST initialize the pool before returning so background
    /// workers can take `Arc<SledPool>` from `ctx.sled_pool` rather than
    /// racing `get_or_init_sled_pool()` on the lazy path.
    #[tokio::test]
    async fn boot_initializes_sled_pool() {
        let tmp = tempfile::TempDir::new().unwrap();
        let manager = NodeManager::new(test_config(tmp.path()));
        let ctx = StartupCtx::boot(manager, None).await.expect("boot");

        // Pool is reachable via the manager — proves boot ran the eager
        // `get_or_init_sled_pool()` rather than leaving it for the first
        // request to lazily create.
        assert!(
            ctx.node_manager.get_sled_pool().await.is_some(),
            "boot() must eagerly initialize SledPool"
        );

        // The ctx field carries the same pool — workers consume it directly,
        // so the bootstrap-resume worker can't observe a `None` pool.
        let from_ctx = Arc::clone(&ctx.sled_pool);
        let from_manager = ctx
            .node_manager
            .get_sled_pool()
            .await
            .expect("pool must be Some");
        assert!(
            Arc::ptr_eq(&from_ctx, &from_manager),
            "ctx.sled_pool and node_manager's pool must be the same Arc — \
             a second Sled at the same path would race the OS file lock"
        );
    }

    // FOLDDB_HOME is a process-global env var; tests that read it must
    // serialize on this mutex to avoid racing each other. Aliased to the
    // crate-wide [`crate::secure_store::test_master_key::ENV_LOCK`] so
    // tests that touch FOLDDB_HOME don't race tests in `handlers::auth`,
    // `server::routes::auth`, etc. that also set FOLDDB_HOME.
    fn folddb_home_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::secure_store::test_master_key::lock()
    }

    /// Happy path: a valid plaintext credentials file at
    /// `$FOLDDB_HOME/credentials.json` is loaded and returned as `Ok(Some(_))`.
    /// Plaintext path only — `os-keychain` flips the on-disk format to
    /// encrypted and is exercised by integration tests on the release build.
    #[cfg(not(feature = "os-keychain"))]
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn load_credentials_bounded_returns_loaded_creds_when_present() {
        let _guard = folddb_home_lock();
        let tmp = tempfile::TempDir::new().expect("tempdir");
        std::env::set_var("FOLDDB_HOME", tmp.path());

        let creds = crate::keychain::ExememCredentials {
            user_hash: "user-hash".into(),
            session_token: "session-token".into(),
            api_key: "api-key".into(),
        };
        let json = serde_json::to_string(&creds).expect("serialize");
        std::fs::write(tmp.path().join("credentials.json"), json).expect("write creds");

        let loaded = load_credentials_bounded()
            .await
            .expect("load should succeed");
        let loaded = loaded.expect("creds file present, expected Some");
        assert_eq!(loaded.user_hash, "user-hash");
        assert_eq!(loaded.session_token, "session-token");
        assert_eq!(loaded.api_key, "api-key");
    }

    /// No credentials file present: returns `Ok(None)` so the caller emits
    /// the "no creds stored" info log and exits cleanly. This is the dominant
    /// path on a fresh install.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn load_credentials_bounded_returns_none_when_no_file() {
        let _guard = folddb_home_lock();
        let tmp = tempfile::TempDir::new().expect("tempdir");
        std::env::set_var("FOLDDB_HOME", tmp.path());

        let result = load_credentials_bounded().await;
        assert!(
            matches!(result, Ok(None)),
            "expected Ok(None), got {:?}",
            result
        );
    }

    /// Keychain-fail leg: a corrupt credentials file makes `load_credentials`
    /// return `Err`, which the helper propagates so the caller can warn
    /// non-fatally. The error string flows through unprefixed (no
    /// "load timed out" / "load task panicked" tag), preserving the
    /// per-class operator visibility the original `match` exposed.
    #[cfg(not(feature = "os-keychain"))]
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn load_credentials_bounded_returns_err_for_corrupt_file() {
        let _guard = folddb_home_lock();
        let tmp = tempfile::TempDir::new().expect("tempdir");
        std::env::set_var("FOLDDB_HOME", tmp.path());
        std::fs::write(tmp.path().join("credentials.json"), "not-json{").expect("write garbage");

        let err = load_credentials_bounded()
            .await
            .expect_err("corrupt file must produce Err");
        assert!(
            !err.starts_with("load timed out") && !err.starts_with("load task panicked"),
            "corrupt-file Err must come from load_credentials, not the timeout/join wrappers; got: {}",
            err
        );
    }

    // Dedicated mutex for `FOLDDB_SCHEMA_PRELOAD_TIMEOUT_SECS`. Kept separate
    // from `folddb_home_lock` because the schema-preload test does not touch
    // FOLDDB_HOME — sharing a lock would couple this test to the credentials
    // tests' panic-poisoning blast radius for no isolation gain.
    fn schema_preload_env_lock() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::{Mutex, OnceLock};
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("schema-preload env lock poisoned")
    }

    /// D5 sub-step 2: when an identity already lives in the Sled tree but
    /// the `.onboarding_complete` marker is missing (the bug where the CLI
    /// wizard skipped writing the marker on the cloud-disabled path), the
    /// boot-time migration must synthesize the marker so subsequent boots
    /// don't drop the user back into the onboarding wizard.
    ///
    /// This test does NOT require the os-keychain feature — sub-step 2
    /// runs unconditionally once an identity is detected.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn migration_synthesizes_missing_marker() {
        let _guard = folddb_home_lock();
        // A 32-zero-byte master key keeps the os-keychain build path off
        // the OS keychain in CI. No effect when the feature is disabled.
        std::env::set_var("FOLDDB_MASTER_KEY", "0".repeat(64));
        let tmp = tempfile::tempdir().expect("tempdir");
        std::env::set_var("FOLDDB_HOME", tmp.path());

        let data_dir = tmp.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();

        // Persist an identity directly into the Sled tree, mirroring what
        // the legacy CLI wizard wrote (modulo encryption — the tree opens
        // either form transparently).
        let pool = Arc::new(SledPool::new(data_dir.clone()));
        let id = crate::identity::generate_identity().expect("gen identity");
        crate::identity::save(Arc::clone(&pool), &id).expect("save identity");

        let marker = data_dir.join(".onboarding_complete");
        assert!(!marker.exists(), "precondition: marker absent");

        super::migrate_legacy_identity_state(&pool).expect("migration");

        assert!(
            marker.exists(),
            "migration must synthesize the marker when identity exists"
        );

        std::env::remove_var("FOLDDB_MASTER_KEY");
        std::env::remove_var("FOLDDB_HOME");
    }

    /// D5 idempotency: a second migration run must be a no-op. Running this
    /// every boot is safe only because both sub-steps' triggering conditions
    /// stop matching after the first successful migration.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn migration_is_idempotent() {
        let _guard = folddb_home_lock();
        std::env::set_var("FOLDDB_MASTER_KEY", "0".repeat(64));
        let tmp = tempfile::tempdir().expect("tempdir");
        std::env::set_var("FOLDDB_HOME", tmp.path());

        let data_dir = tmp.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        let pool = Arc::new(SledPool::new(data_dir.clone()));
        let id = crate::identity::generate_identity().expect("gen identity");
        crate::identity::save(Arc::clone(&pool), &id).expect("save");

        super::migrate_legacy_identity_state(&pool).expect("first run");
        let raw_after_first = crate::identity::peek_raw_identity_value(&pool)
            .expect("peek")
            .expect("identity present");
        let marker_mtime_after_first = std::fs::metadata(data_dir.join(".onboarding_complete"))
            .expect("marker present")
            .modified()
            .expect("mtime");

        super::migrate_legacy_identity_state(&pool).expect("second run");
        let raw_after_second = crate::identity::peek_raw_identity_value(&pool)
            .expect("peek")
            .expect("identity present");

        assert_eq!(
            raw_after_first, raw_after_second,
            "idempotent run must not rewrite the identity tree"
        );
        let marker_mtime_after_second = std::fs::metadata(data_dir.join(".onboarding_complete"))
            .expect("marker present")
            .modified()
            .expect("mtime");
        assert_eq!(
            marker_mtime_after_first, marker_mtime_after_second,
            "second run must not touch the marker file"
        );

        std::env::remove_var("FOLDDB_MASTER_KEY");
        std::env::remove_var("FOLDDB_HOME");
    }

    /// D5 conservatism: when no identity exists, the migration must NOT
    /// create a marker. Synthesizing one here would let a never-set-up
    /// node be treated as initialized.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn migration_no_op_on_empty_pool() {
        let _guard = folddb_home_lock();
        std::env::set_var("FOLDDB_MASTER_KEY", "0".repeat(64));
        let tmp = tempfile::tempdir().expect("tempdir");
        std::env::set_var("FOLDDB_HOME", tmp.path());

        let data_dir = tmp.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        let pool = Arc::new(SledPool::new(data_dir.clone()));

        super::migrate_legacy_identity_state(&pool).expect("migration");

        let marker = data_dir.join(".onboarding_complete");
        assert!(
            !marker.exists(),
            "migration must NOT create a marker for a never-set-up node"
        );

        std::env::remove_var("FOLDDB_MASTER_KEY");
        std::env::remove_var("FOLDDB_HOME");
    }

    /// D5 sub-step 1: a plaintext identity (legacy CLI wizard) must be
    /// re-encrypted under the keychain master key after one boot. Identity
    /// bytes must round-trip exactly — the migration is structural, not
    /// rotational.
    ///
    /// Gated on os-keychain because that's the only build that has the
    /// encrypted-vs-plaintext distinction. FOLDDB_MASTER_KEY supplies the
    /// key so CI can run without a real keychain.
    #[cfg(feature = "os-keychain")]
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn migration_reencrypts_plaintext_identity_under_master_key() {
        let _guard = folddb_home_lock();
        std::env::set_var("FOLDDB_MASTER_KEY", "0".repeat(64));
        let tmp = tempfile::tempdir().expect("tempdir");
        std::env::set_var("FOLDDB_HOME", tmp.path());

        let data_dir = tmp.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();

        // Write a plaintext identity directly to the tree. Mirrors what
        // the legacy CLI wizard produced (master key never minted because
        // the tree was opened standalone with no keychain access in the
        // wizard's process).
        let pool = Arc::new(SledPool::new(data_dir.clone()));
        let id = crate::identity::generate_identity().expect("gen identity");
        {
            let guard = pool.acquire_arc().expect("acquire pool");
            let tree = guard.db().open_tree("node_identity").expect("open tree");
            tree.insert(b"private_key", id.private_key.as_bytes())
                .expect("insert priv");
            tree.insert(b"public_key", id.public_key.as_bytes())
                .expect("insert pub");
        }

        // Precondition: stored value is plaintext (no ENC: prefix).
        let raw_before = crate::identity::peek_raw_identity_value(&pool)
            .expect("peek")
            .expect("identity present");
        assert!(
            !raw_before.starts_with("ENC:"),
            "precondition: stored identity must be plaintext"
        );

        super::migrate_legacy_identity_state(&pool).expect("migration");

        // Postcondition 1: stored value is now ENC:-prefixed.
        let raw_after = crate::identity::peek_raw_identity_value(&pool)
            .expect("peek")
            .expect("identity present");
        assert!(
            raw_after.starts_with("ENC:"),
            "post: stored identity must be encrypted; got: {}",
            raw_after.chars().take(20).collect::<String>()
        );

        // Postcondition 2: identity bytes round-trip — migration is
        // structural, not rotational.
        let loaded = crate::identity::load(Arc::clone(&pool))
            .expect("load")
            .expect("identity present");
        assert_eq!(
            loaded.private_key, id.private_key,
            "private key must round-trip"
        );
        assert_eq!(
            loaded.public_key, id.public_key,
            "public key must round-trip"
        );

        std::env::remove_var("FOLDDB_MASTER_KEY");
        std::env::remove_var("FOLDDB_HOME");
    }

    /// Regression for the boot-blocking schema preload. A slow schema service
    /// must NOT pin Phase 1 — the outer `tokio::time::timeout` has to fire
    /// well before the wiremock stub's response would land, so HTTP bind
    /// happens on schedule.
    #[tokio::test(flavor = "multi_thread")]
    #[allow(clippy::await_holding_lock)]
    async fn load_schemas_timeout_unblocks_boot_when_schema_service_is_slow() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let _guard = schema_preload_env_lock();
        std::env::set_var("FOLDDB_SCHEMA_PRELOAD_TIMEOUT_SECS", "1");

        // Wiremock stub that delays 5s — far past the 1s outer timeout.
        // If the timeout doesn't fire, the test will hang for 5s and fail
        // the elapsed-time assertion, not just print a slow result.
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "schemas": [] }))
                    .set_delay(std::time::Duration::from_secs(5)),
            )
            .mount(&mock)
            .await;
        let slow_url = mock.uri();

        let tmp = tempfile::TempDir::new().expect("tempdir");
        let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
        let cfg = NodeManagerConfig {
            base_config: NodeConfig::new(tmp.path().to_path_buf())
                .with_schema_service_url(&slow_url)
                .with_seed_identity(crate::identity::identity_from_keypair(&keypair)),
            config_dir: tmp.path().join("config"),
            upload_path: tmp.path().join("uploads"),
        };
        let manager = Arc::new(NodeManager::new(cfg));

        let started = std::time::Instant::now();
        load_schemas_if_configured(&manager).await;
        let elapsed = started.elapsed();

        std::env::remove_var("FOLDDB_SCHEMA_PRELOAD_TIMEOUT_SECS");

        // 1s timeout + slack. Anything close to 5s means the outer timeout
        // never fired and we're back to blocking on the stub.
        assert!(
            elapsed < std::time::Duration::from_secs(3),
            "load_schemas_if_configured must abort via timeout; took {:?}",
            elapsed
        );
    }
}
