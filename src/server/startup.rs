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
}

impl StartupCtx {
    /// Phase 1. Run all I/O that must complete before the HTTP server binds.
    ///
    /// Order matters for log readability but not correctness — every step is
    /// independent except for `get_or_init_sled_pool`, which must run before
    /// `bootstrap_pending` is captured so the resume worker has a guaranteed
    /// pool to use.
    pub async fn boot(
        node_manager: NodeManager,
        obs: Option<ObsHandles>,
    ) -> FoldDbResult<Arc<Self>> {
        let node_manager = Arc::new(node_manager);

        // Eager pool init — fixes the bootstrap-resume `None` pool race.
        let sled_pool = node_manager.get_or_init_sled_pool().await;

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

        let upload_storage = build_upload_storage();
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
        let ingestion =
            web::Data::new(RwLock::new(IngestionService::from_env().ok().map(Arc::new)));
        let apple_sync_config = web::Data::new(create_sync_config_state());
        let batch_controllers = web::Data::new(create_batch_controller_map());
        let llm_query = web::Data::new(LlmQueryState::new());

        Ok(Arc::new(Self {
            node_manager,
            sled_pool,
            bootstrap_pending,
            obs,
            app_state,
            upload_storage,
            progress_tracker,
            ingestion,
            apple_sync_config,
            batch_controllers,
            llm_query,
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

/// Best-effort schema preload from the configured schema service. Failures
/// are logged but not propagated — the server still starts, schemas just
/// aren't cached.
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

    tracing::info!(
        target: "fold_node::database",
        "Loading schemas from schema service at {}...",
        url
    );

    let client = crate::fold_node::SchemaServiceClient::new(&url);
    match client.list_schemas().await {
        Ok(schemas) => tracing::info!(
            target: "fold_node::database",
            "Loaded {} schemas from schema service",
            schemas.len()
        ),
        Err(e) => tracing::error!(
            target: "fold_node::database",
            "Failed to load schemas from schema service: {}. Server will start but no schemas will be available.",
            e
        ),
    }
}

fn build_upload_storage() -> UploadStorage {
    let upload_path = std::env::var("FOLDDB_UPLOAD_PATH")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            crate::utils::paths::folddb_home()
                .map(|h| h.join("data").join("uploads"))
                .unwrap_or_else(|_| std::path::PathBuf::from("data/uploads"))
        });
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

async fn token_refresh(ctx: Arc<StartupCtx>) {
    // `load_credentials` may block on the OS keychain (release builds with
    // `os-keychain`): if the user's macOS keychain is locked, the call sits
    // until they dismiss a prompt — and on a headless box, until forever.
    // Move it onto the blocking pool and bound it with a timeout so a stuck
    // keychain can't pin a tokio worker thread for the lifetime of the
    // process. `refresh_session_token` below has its own 10s timeout for
    // the network leg.
    let load_result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::task::spawn_blocking(crate::keychain::load_credentials),
    )
    .await;
    let creds = match load_result {
        Ok(Ok(Ok(Some(c)))) => c,
        Ok(Ok(Ok(None))) => {
            tracing::info!("No Exemem credentials stored; skipping startup refresh");
            return;
        }
        Ok(Ok(Err(e))) => {
            tracing::warn!("Failed to load Exemem credentials (non-fatal): {}", e);
            return;
        }
        Ok(Err(e)) => {
            tracing::warn!(
                "Keychain load task panicked (non-fatal); skipping startup refresh: {}",
                e
            );
            return;
        }
        Err(_) => {
            tracing::warn!(
                "Keychain load timed out after 5s (non-fatal); skipping startup refresh"
            );
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
}
