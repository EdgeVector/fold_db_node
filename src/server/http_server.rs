use super::middleware::auth::UserContextMiddleware;
use super::node_manager::NodeManager;
use super::routes::log as log_routes;
use super::routes::{
    admin as admin_routes, auth as auth_routes, config as config_routes,
    discovery as discovery_routes, feed as feed_routes, filesystem as filesystem_routes,
    query as query_routes, schema as schema_routes, security as security_routes,
    system as system_routes,
};
use super::static_assets::Asset;
use crate::fold_node::llm_query;
use crate::server::routes::apple_import as apple_import_routes;
use crate::server::routes::file_upload as file_upload_routes;
use crate::server::routes::ingestion as ingestion_routes;
use crate::server::routes::smart_folder as smart_folder_routes;
use crate::utils::http_errors;
use fold_db::error::{FoldDbError, FoldDbResult};

use actix_cors::Cors;
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;

use actix_web::{web, App, HttpRequest, HttpResponse, HttpServer as ActixHttpServer};
use std::sync::Arc;

/// HTTP server for the Fold node.
///
/// FoldHttpServer provides a web-based interface for external clients to interact
/// with a Fold node. It handles HTTP requests and can serve the built React
/// UI, and provides REST API endpoints for schemas, queries, and mutations.
///
/// # Architecture
///
/// The server uses lazy node initialization:
/// - On startup: Only configuration is loaded
/// - On first request: Node is created and cached
/// - Subsequent requests: Cached node is reused
pub struct FoldHttpServer {
    /// The node manager for lazy per-user node creation
    node_manager: Arc<NodeManager>,
    /// The HTTP server bind address
    bind_address: String,
}

/// Shared application state for the HTTP server.
pub struct AppState {
    /// The node manager for getting per-user nodes
    pub(crate) node_manager: Arc<NodeManager>,
}

impl AppState {
    /// Resolve the current discovery configuration without mutating any
    /// process-wide state. Returns `None` when the node is not registered
    /// with Exemem.
    pub async fn discovery_config(&self) -> Option<super::discovery_config::DiscoveryConfig> {
        super::discovery_config::DiscoveryConfig::resolve(&self.node_manager).await
    }
}

impl FoldHttpServer {
    /// Create a new HTTP server.
    ///
    /// This method creates a new HTTP server that listens on the specified address.
    /// It uses the provided NodeManager to create per-user nodes lazily.
    ///
    /// # Arguments
    ///
    /// * `node_manager` - The NodeManager instance for creating per-user nodes
    /// * `bind_address` - The address to bind to (e.g., "127.0.0.1:9001")
    ///
    /// # Returns
    ///
    /// A `FoldDbResult` containing the new FoldHttpServer instance.
    ///
    /// # Errors
    ///
    /// Returns a `FoldDbError` if:
    /// * There is an error starting the HTTP server
    pub async fn new(node_manager: NodeManager, bind_address: &str) -> FoldDbResult<Self> {
        fold_db::logging::LoggingSystem::init_with_fallback(None).await;

        Ok(Self {
            node_manager: Arc::new(node_manager),
            bind_address: bind_address.to_string(),
        })
    }

    /// Run the HTTP server.
    ///
    /// This method starts the HTTP server and begins accepting client connections.
    /// It can serve the compiled React UI and provides REST API endpoints for
    /// schemas, queries, and mutations.
    ///
    /// # Returns
    ///
    /// A `FoldDbResult` indicating success or failure.
    ///
    /// # Errors
    ///
    /// Returns a `FoldDbError` if:
    /// * There is an error binding to the specified address
    /// * There is an error starting the server
    pub async fn run(&self) -> FoldDbResult<()> {
        // Check for interrupted bootstrap and resume if needed
        if let Some((api_url, api_key)) = crate::server::routes::auth::check_bootstrap_pending() {
            log_feature!(
                LogFeature::HttpServer,
                info,
                "Found interrupted bootstrap — resuming cloud data download"
            );
            let node_manager = self.node_manager.clone();
            tokio::spawn(async move {
                // Get Sled pool from the node manager
                if let Some(pool) = node_manager.get_sled_pool().await {
                    if let Err(e) = crate::server::routes::auth::resume_bootstrap(
                        &api_url,
                        &api_key,
                        &node_manager,
                        pool,
                    )
                    .await
                    {
                        log::error!("Bootstrap resume failed: {}", e);
                    }
                } else {
                    log::warn!("Cannot resume bootstrap: no Sled pool available yet");
                }
            });
        }

        // Load schemas from schema service if configured
        self.load_schemas_if_configured().await?;

        // Initialize upload storage — use FOLDDB_UPLOAD_PATH env or default to data/uploads
        let upload_path = std::env::var("FOLDDB_UPLOAD_PATH")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                crate::utils::paths::folddb_home()
                    .map(|h| h.join("data").join("uploads"))
                    .unwrap_or_else(|_| std::path::PathBuf::from("data/uploads"))
            });
        let upload_storage = fold_db::storage::UploadStorage::local(upload_path);

        log_feature!(
            LogFeature::HttpServer,
            info,
            "Upload storage initialized: {}",
            if upload_storage.is_local() {
                "Local"
            } else {
                "S3"
            }
        );

        // Discovery configuration is resolved on-demand via
        // `AppState::discovery_config()` — no process-wide env mutation.
        // Log whether it is currently available so operators can debug.
        match super::discovery_config::DiscoveryConfig::resolve(&self.node_manager).await {
            Some(cfg) => log::info!("Discovery configuration resolved: url={}", cfg.url),
            None => {
                log::info!("Discovery configuration not yet available (no identity registered)")
            }
        }

        // Create shared application state
        let app_state = web::Data::new(AppState {
            node_manager: self.node_manager.clone(),
        });

        // Auto-refresh Exemem session token on startup — but ONLY if the stored
        // token is actually near expiry. Unconditionally re-registering on every
        // boot rotates the API key (Exemem deactivates the old one), which leaves
        // the in-memory `SyncEngine` holding a stale key and breaks cloud sync.
        //
        // Non-fatal: if load/parse/refresh fails (no network, no credentials),
        // we log and continue. Timeout guards against macOS Keychain blocking.
        {
            let app_state_clone = app_state.clone();
            tokio::spawn(async move {
                // Load stored credentials so we can inspect the session token
                // and decide whether a refresh is actually needed.
                let creds = match crate::keychain::load_credentials() {
                    Ok(Some(c)) => c,
                    Ok(None) => {
                        log::info!("No Exemem credentials stored; skipping startup refresh");
                        return;
                    }
                    Err(e) => {
                        log::warn!("Failed to load Exemem credentials (non-fatal): {}", e);
                        return;
                    }
                };

                // Only refresh when the token is near expiry. Threshold of 12h
                // of remaining lifetime means each boot can refresh at most once
                // per half-day instead of on every launch.
                const MIN_REMAINING_SECS: i64 = 12 * 60 * 60;
                let now = chrono::Utc::now().timestamp();
                match session_token_needs_refresh(&creds.session_token, now, MIN_REMAINING_SECS) {
                    Ok(false) => {
                        log::info!(
                            "Exemem session token still valid (>12h remaining); skipping startup refresh"
                        );
                        return;
                    }
                    Ok(true) => {
                        log::info!("Exemem session token near expiry; refreshing...");
                    }
                    Err(e) => {
                        // Malformed token — treat as "needs refresh" rather than
                        // a silent skip, so we recover from a corrupted token.
                        log::warn!("Unable to parse stored session token ({}); refreshing", e);
                    }
                }

                match tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    crate::server::routes::auth::refresh_session_token(&app_state_clone),
                )
                .await
                {
                    Ok(Ok(_)) => log::info!("Exemem session token refreshed successfully"),
                    Ok(Err(e)) => {
                        log::warn!("Exemem session token refresh failed (non-fatal): {}", e)
                    }
                    Err(_) => {
                        log::warn!("Exemem session token refresh timed out after 10s (non-fatal)")
                    }
                }
            });
        }

        // Create upload storage data
        let upload_storage_data = web::Data::new(upload_storage.clone());

        // Create LLM query state (gracefully handles missing configuration)
        let llm_query_state = web::Data::new(llm_query::LlmQueryState::new());

        // Create IngestionService wrapped in RwLock so config saves can reload it
        let ingestion_service: Option<Arc<crate::ingestion::ingestion_service::IngestionService>> =
            crate::ingestion::ingestion_service::IngestionService::from_env()
                .ok()
                .map(Arc::new);
        let ingestion_service_data = web::Data::new(tokio::sync::RwLock::new(ingestion_service));

        // Create BatchControllerMap for spend-limit batch tracking
        let batch_controller_map_data =
            web::Data::new(crate::ingestion::batch_controller::create_batch_controller_map());

        // Load Apple auto-sync config
        let sync_config_state =
            crate::ingestion::apple_import::sync_scheduler::create_sync_config_state();
        let sync_config_data = web::Data::new(sync_config_state);

        let progress_tracker = fold_db::progress::create_tracker().await;
        let progress_tracker_data = web::Data::new(progress_tracker);

        // Spawn Apple auto-sync background scheduler
        crate::server::routes::apple_import::spawn_sync_scheduler(
            sync_config_data.get_ref().clone(),
            app_state.clone(),
            ingestion_service_data.clone(),
            progress_tracker_data.clone(),
        );

        // Start the HTTP server
        let server = ActixHttpServer::new(move || {
            // CORS — restrict to localhost origins only.
            // This is the primary CSRF protection: prevents external webpages
            // from making requests to the local FoldDB server.
            // Allow the server's own port plus common Vite dev server ports.
            let cors = Cors::default()
                .allowed_origin_fn(|origin, _req_head| {
                    let origin = origin.as_bytes();
                    // Allow any localhost/127.0.0.1 origin (safe for local dev)
                    origin.starts_with(b"http://localhost:")
                        || origin.starts_with(b"http://127.0.0.1:")
                        || origin == b"tauri://localhost"
                })
                .allow_any_method()
                .allow_any_header()
                .max_age(3600);

            // Configure custom JSON error handler
            let json_config =
                web::JsonConfig::default().error_handler(http_errors::json_error_handler);

            App::new()
                .wrap(cors)
                .wrap(UserContextMiddleware)
                .app_data(app_state.clone())
                .app_data(llm_query_state.clone())
                .app_data(upload_storage_data.clone())
                .app_data(progress_tracker_data.clone())
                .app_data(ingestion_service_data.clone())
                .app_data(batch_controller_map_data.clone())
                .app_data(sync_config_data.clone())
                .app_data(json_config)
                .configure(Self::configure_api)
                // Serve embedded static assets (React build)
                // This must be last to allow API routes to take precedence
                .default_service(web::route().to(serve_embedded_asset))
        })
        .bind(&self.bind_address)
        .map_err(|e| FoldDbError::Config(format!("Failed to bind HTTP server: {}", e)))?
        .run();

        // Run the server
        server
            .await
            .map_err(|e| FoldDbError::Config(format!("HTTP server error: {}", e)))?;

        Ok(())
    }

    async fn load_schemas_if_configured(&self) -> FoldDbResult<()> {
        // Load schemas from schema service if configured
        let base_config = self.node_manager.get_base_config().await;
        let schema_service_url = base_config.schema_service_url.clone();

        if let Some(url) = schema_service_url {
            // Skip loading for mock/test schema services
            if crate::fold_node::node::FoldNode::is_test_schema_service(&url) {
                log_feature!(
                    LogFeature::Database,
                    info,
                    "Mock schema service detected ({}). Skipping automatic schema loading. Schemas must be loaded manually in tests.",
                    url
                );
            } else {
                log_feature!(
                    LogFeature::Database,
                    info,
                    "Loading schemas from schema service at {}...",
                    url
                );

                // For schema loading, we need a temporary node
                // Schemas are global, so we use a system context
                let client = crate::fold_node::SchemaServiceClient::new(&url);

                match client.list_schemas().await {
                    Ok(schemas) => {
                        log_feature!(
                            LogFeature::Database,
                            info,
                            "Loaded {} schemas from schema service",
                            schemas.len()
                        );
                    }
                    Err(e) => {
                        log_feature!(
                            LogFeature::Database,
                            error,
                            "Failed to load schemas from schema service: {}. Server will start but no schemas will be available.",
                            e
                        );
                    }
                }
            }
        }
        Ok(())
    }

    fn configure_api(cfg: &mut web::ServiceConfig) {
        cfg.service(
            web::scope("/api")
                .configure(Self::configure_openapi_routes)
                .configure(Self::configure_schema_routes)
                .configure(Self::configure_view_routes)
                .configure(Self::configure_query_routes)
                .configure(Self::configure_ingestion_routes)
                .configure(Self::configure_log_routes)
                .configure(Self::configure_system_routes)
                .configure(Self::configure_llm_query_routes)
                .configure(Self::configure_security_routes)
                .configure(Self::configure_discovery_routes)
                .configure(Self::configure_fingerprints_routes)
                .configure(Self::configure_trust_routes)
                .configure(Self::configure_identity_routes)
                .configure(Self::configure_sharing_routes)
                // Capability tokens and payment gates are not yet implemented — routes removed
                .configure(Self::configure_feed_routes)
                .configure(Self::configure_remote_routes)
                .configure(Self::configure_auth_routes)
                .configure(Self::configure_sync_routes)
                .configure(Self::configure_org_routes)
                .configure(Self::configure_test_admin_routes),
        );
    }

    /// Register test-admin routes. Registration is unconditional; the handlers
    /// themselves reject requests unless `FOLDDB_ENABLE_TEST_ADMIN=1`.
    fn configure_test_admin_routes(cfg: &mut web::ServiceConfig) {
        use crate::server::routes::test_admin as test_admin_routes;
        cfg.service(
            web::scope("/test-admin")
                .route(
                    "/contacts",
                    web::post().to(test_admin_routes::upsert_contact),
                )
                .route(
                    "/my-messaging-keys",
                    web::get().to(test_admin_routes::my_messaging_keys),
                ),
        );
    }

    fn configure_openapi_routes(cfg: &mut web::ServiceConfig) {
        cfg.route(
            "/openapi.json",
            web::get().to(|| async move {
                let doc = crate::server::openapi::build_openapi();
                HttpResponse::Ok()
                    .content_type("application/json")
                    .body(doc)
            }),
        );
    }

    fn configure_schema_routes(cfg: &mut web::ServiceConfig) {
        cfg.route("/schemas", web::get().to(schema_routes::list_schemas))
            .route("/schemas/load", web::post().to(schema_routes::load_schemas))
            .route("/schema/{name}", web::get().to(schema_routes::get_schema))
            .route(
                "/schema/{name}/keys",
                web::get().to(schema_routes::list_schema_keys),
            )
            .route(
                "/schema/{name}/approve",
                web::post().to(schema_routes::approve_schema),
            )
            .route(
                "/schema/{name}/block",
                web::post().to(schema_routes::block_schema),
            );
    }

    fn configure_view_routes(cfg: &mut web::ServiceConfig) {
        use crate::server::routes::views as view_routes;

        cfg.route("/views", web::get().to(view_routes::list_views))
            .route("/view", web::post().to(view_routes::create_view))
            .route("/view/{name}", web::get().to(view_routes::get_view))
            .route("/view/{name}", web::delete().to(view_routes::delete_view))
            .route(
                "/view/{name}/approve",
                web::post().to(view_routes::approve_view),
            )
            .route(
                "/view/{name}/block",
                web::post().to(view_routes::block_view),
            )
            .route("/views/load/{name}", web::post().to(view_routes::load_view));
    }

    fn configure_query_routes(cfg: &mut web::ServiceConfig) {
        cfg.route("/query", web::post().to(query_routes::execute_query))
            .route("/mutation", web::post().to(query_routes::execute_mutation))
            .route(
                "/native-index/search",
                web::get().to(query_routes::native_index_search),
            )
            .route(
                "/indexing/status",
                web::get().to(query_routes::get_indexing_status),
            )
            .route(
                "/history/{molecule_uuid}",
                web::get().to(query_routes::get_molecule_history),
            )
            .route(
                "/atom/{atom_uuid}",
                web::get().to(query_routes::get_atom_content),
            )
            .route(
                "/process-results/{progress_id}",
                web::get().to(query_routes::get_process_results),
            )
            .route("/conflicts", web::get().to(query_routes::get_conflicts))
            .route(
                "/conflicts/{conflict_id}/resolve",
                web::post().to(query_routes::resolve_conflict),
            );
    }

    fn configure_ingestion_routes(cfg: &mut web::ServiceConfig) {
        cfg.route(
            "/ingestion/process",
            web::post().to(ingestion_routes::process_json),
        )
        .route(
            "/ingestion/upload",
            web::post().to(file_upload_routes::upload_file),
        )
        .route(
            "/ingestion/status",
            web::get().to(ingestion_routes::get_status),
        )
        .route(
            "/ingestion/config",
            web::get().to(ingestion_routes::get_ingestion_config),
        )
        .route(
            "/ingestion/config",
            web::post().to(ingestion_routes::save_ingestion_config),
        )
        .route(
            "/ingestion/progress",
            web::get().to(ingestion_routes::get_all_progress),
        )
        .route(
            "/ingestion/progress/summary",
            web::get().to(ingestion_routes::get_progress_summary),
        )
        .route(
            "/ingestion/progress/{id}",
            web::get().to(ingestion_routes::get_progress),
        )
        .route(
            "/ingestion/batch-folder",
            web::post().to(ingestion_routes::batch_folder_ingest),
        )
        .route(
            "/ingestion/smart-folder/scan",
            web::post().to(smart_folder_routes::smart_folder_scan),
        )
        .route(
            "/ingestion/smart-folder/scan/{id}",
            web::get().to(smart_folder_routes::get_scan_result),
        )
        .route(
            "/ingestion/smart-folder/ingest",
            web::post().to(smart_folder_routes::smart_folder_ingest),
        )
        .route(
            "/ingestion/batch/{batch_id}",
            web::get().to(ingestion_routes::get_batch_status),
        )
        .route(
            "/ingestion/smart-folder/resume",
            web::post().to(ingestion_routes::resume_batch),
        )
        .route(
            "/ingestion/smart-folder/cancel",
            web::post().to(ingestion_routes::cancel_batch),
        )
        .route(
            "/ingestion/smart-folder/adjust",
            web::post().to(smart_folder_routes::adjust_scan_results),
        )
        .route(
            "/file/{hash}",
            web::get().to(file_upload_routes::serve_file),
        )
        .route(
            "/ingestion/ollama/models",
            web::get().to(ingestion_routes::list_ollama_models),
        )
        .route(
            "/ingestion/apple-import/status",
            web::get().to(apple_import_routes::apple_import_status),
        )
        .route(
            "/ingestion/apple-import/notes",
            web::post().to(apple_import_routes::apple_import_notes),
        )
        .route(
            "/ingestion/apple-import/reminders",
            web::post().to(apple_import_routes::apple_import_reminders),
        )
        .route(
            "/ingestion/apple-import/photos",
            web::post().to(apple_import_routes::apple_import_photos),
        )
        .route(
            "/ingestion/apple-import/calendar",
            web::post().to(apple_import_routes::apple_import_calendar),
        )
        .route(
            "/ingestion/apple-import/sync-config",
            web::get().to(apple_import_routes::get_sync_config),
        )
        .route(
            "/ingestion/apple-import/sync-config",
            web::post().to(apple_import_routes::update_sync_config),
        )
        .route(
            "/ingestion/apple-import/next-sync",
            web::get().to(apple_import_routes::get_next_sync),
        );
    }

    fn configure_log_routes(cfg: &mut web::ServiceConfig) {
        cfg.route("/logs", web::get().to(log_routes::list_logs))
            .route("/logs/stream", web::get().to(log_routes::stream_logs))
            .route("/logs/config", web::get().to(log_routes::get_config))
            .route(
                "/logs/config/reload",
                web::post().to(log_routes::reload_config),
            )
            .route(
                "/logs/level",
                web::put().to(log_routes::update_feature_level),
            )
            .route("/logs/features", web::get().to(log_routes::get_features));
    }

    fn configure_sync_routes(cfg: &mut web::ServiceConfig) {
        use super::routes::sync as sync_routes;

        cfg.route("/sync/status", web::get().to(sync_routes::get_sync_status))
            .route("/sync/trigger", web::post().to(sync_routes::trigger_sync))
            .route(
                "/sync/org/{org_hash}/status",
                web::get().to(sync_routes::get_org_sync_status),
            );
    }

    fn configure_system_routes(cfg: &mut web::ServiceConfig) {
        use super::routes::sync as sync_routes;

        cfg.route(
            "/system/status",
            web::get().to(system_routes::get_system_status),
        )
        .route(
            "/system/public-key",
            web::get().to(system_routes::get_node_public_key),
        )
        .route(
            "/system/sync-status",
            web::get().to(sync_routes::get_sync_status),
        )
        .route(
            "/system/reset-database",
            web::post().to(admin_routes::reset_database),
        )
        .route(
            "/system/auto-identity",
            web::get().to(config_routes::auto_identity),
        )
        .route(
            "/system/database-config",
            web::get().to(config_routes::get_database_config),
        )
        .route(
            "/system/database-config",
            web::post().to(config_routes::update_database_config),
        )
        .route("/system/setup", web::post().to(config_routes::apply_setup))
        .route(
            "/system/migrate-to-cloud",
            web::post().to(admin_routes::migrate_to_cloud),
        )
        .route(
            "/system/database-status",
            web::get().to(config_routes::get_database_status),
        )
        .route(
            "/system/onboarding-complete",
            web::post().to(config_routes::mark_onboarding_complete),
        )
        .route(
            "/system/complete-path",
            web::post().to(filesystem_routes::complete_path),
        )
        .route(
            "/system/list-directory",
            web::post().to(filesystem_routes::list_directory),
        );
    }

    fn configure_llm_query_routes(cfg: &mut web::ServiceConfig) {
        cfg.route(
            "/llm-query/native-index",
            web::post().to(llm_query::ai_native_index_query),
        )
        .route("/llm-query/chat", web::post().to(llm_query::chat))
        .route(
            "/llm-query/analyze-followup",
            web::post().to(llm_query::analyze_followup),
        )
        .route("/llm-query/agent", web::post().to(llm_query::agent_query));
    }

    fn configure_security_routes(cfg: &mut web::ServiceConfig) {
        cfg.service(
            web::scope("/security").service(
                web::resource("/system-key")
                    .route(web::get().to(security_routes::get_system_public_key)),
            ),
        );
    }

    fn configure_fingerprints_routes(cfg: &mut web::ServiceConfig) {
        use crate::server::routes::fingerprints as fp_routes;
        cfg.service(
            web::scope("/fingerprints")
                .route(
                    "/ingest-photo-faces",
                    web::post().to(fp_routes::ingest_photo_faces),
                )
                .route(
                    "/my-identity-card",
                    web::get().to(fp_routes::get_my_identity_card),
                )
                .route(
                    "/identity-cards/import",
                    web::post().to(fp_routes::import_identity_card),
                )
                .route("/identities", web::get().to(fp_routes::list_identities))
                .route(
                    "/ingest-text-signals",
                    web::post().to(fp_routes::ingest_text_signals),
                )
                .route(
                    "/import-contacts",
                    web::post().to(fp_routes::import_contacts),
                )
                .service(
                    web::scope("/personas")
                        .route("", web::get().to(fp_routes::list_personas))
                        .route("/{id}", web::get().to(fp_routes::get_persona))
                        .route("/{id}", web::patch().to(fp_routes::update_persona))
                        .route("/{id}", web::delete().to(fp_routes::delete_persona)),
                )
                .service(
                    web::scope("/ingestion-errors")
                        .route("", web::get().to(fp_routes::list_ingestion_errors))
                        .route("/{id}", web::patch().to(fp_routes::resolve_ingestion_error)),
                )
                .service(
                    web::scope("/suggestions")
                        .route("", web::get().to(fp_routes::list_suggested_personas))
                        .route(
                            "/accept",
                            web::post().to(fp_routes::accept_suggested_persona),
                        ),
                ),
        );
    }

    fn configure_discovery_routes(cfg: &mut web::ServiceConfig) {
        cfg.service(
            web::scope("/discovery")
                .route("/opt-ins", web::get().to(discovery_routes::list_opt_ins))
                .route("/opt-in", web::post().to(discovery_routes::opt_in))
                .route("/opt-out", web::post().to(discovery_routes::opt_out))
                .route(
                    "/my-pseudonyms",
                    web::get().to(discovery_routes::my_pseudonyms),
                )
                .route(
                    "/opt-out-all",
                    web::post().to(discovery_routes::opt_out_all),
                )
                .route("/publish", web::post().to(discovery_routes::publish))
                .route("/search", web::post().to(discovery_routes::search))
                .route("/connect", web::post().to(discovery_routes::connect))
                .route(
                    "/connection-requests",
                    web::get().to(discovery_routes::connection_requests),
                )
                .route(
                    "/connection-requests/respond",
                    web::post().to(discovery_routes::respond_to_request),
                )
                .route(
                    "/connection-requests/check-network",
                    web::post().to(discovery_routes::check_network),
                )
                .route(
                    "/sent-requests",
                    web::get().to(discovery_routes::sent_requests),
                )
                .route("/requests", web::get().to(discovery_routes::poll_requests))
                .route(
                    "/browse/categories",
                    web::get().to(discovery_routes::browse_categories),
                )
                .route("/interests", web::get().to(discovery_routes::get_interests))
                .route(
                    "/interests/toggle",
                    web::post().to(discovery_routes::toggle_interest),
                )
                .route(
                    "/interests/detect",
                    web::post().to(discovery_routes::detect_interests),
                )
                .route(
                    "/similar-profiles",
                    web::get().to(discovery_routes::similar_profiles),
                )
                .route(
                    "/calendar-sharing/status",
                    web::get().to(discovery_routes::calendar_sharing_status),
                )
                .route(
                    "/calendar-sharing/opt-in",
                    web::post().to(discovery_routes::calendar_sharing_opt_in),
                )
                .route(
                    "/calendar-sharing/opt-out",
                    web::post().to(discovery_routes::calendar_sharing_opt_out),
                )
                .route(
                    "/calendar-sharing/sync",
                    web::post().to(discovery_routes::sync_calendar_events),
                )
                .route(
                    "/calendar-sharing/peer-events",
                    web::post().to(discovery_routes::store_peer_events),
                )
                .route(
                    "/shared-events",
                    web::get().to(discovery_routes::get_shared_events),
                )
                // Photo moment detection routes
                .route("/moments", web::get().to(discovery_routes::moment_list))
                .route(
                    "/moments/opt-ins",
                    web::get().to(discovery_routes::moment_opt_in_list),
                )
                .route(
                    "/moments/opt-in",
                    web::post().to(discovery_routes::moment_opt_in),
                )
                .route(
                    "/moments/opt-out",
                    web::post().to(discovery_routes::moment_opt_out),
                )
                .route(
                    "/moments/scan",
                    web::post().to(discovery_routes::moment_scan),
                )
                .route(
                    "/moments/receive",
                    web::post().to(discovery_routes::moment_receive_hashes),
                )
                .route(
                    "/moments/detect",
                    web::post().to(discovery_routes::moment_detect),
                )
                // Face discovery routes
                .route(
                    "/face-search",
                    web::post().to(discovery_routes::face_search),
                )
                .route(
                    "/faces/{schema}/{key}",
                    web::get().to(discovery_routes::list_faces),
                )
                // Data sharing
                .route("/share", web::post().to(discovery_routes::share_data)),
        );

        // Notification routes (top-level, not under /discovery)
        cfg.service(
            web::scope("/notifications")
                .route("", web::get().to(discovery_routes::list_notifications))
                .route(
                    "/count",
                    web::get().to(discovery_routes::notification_count),
                )
                .route(
                    "/{id}",
                    web::delete().to(discovery_routes::dismiss_notification),
                ),
        );
    }

    fn configure_trust_routes(cfg: &mut web::ServiceConfig) {
        use crate::server::routes::trust as trust_routes;

        cfg.service(
            web::scope("/trust")
                .route("/grant", web::post().to(trust_routes::grant_trust))
                .route(
                    "/revoke/{key}",
                    web::delete().to(trust_routes::revoke_trust),
                )
                .route("/grants", web::get().to(trust_routes::list_trust_grants))
                .route("/resolve/{key}", web::get().to(trust_routes::resolve_trust))
                .route("/audit", web::get().to(trust_routes::get_audit_log))
                .route("/invite", web::post().to(trust_routes::create_trust_invite))
                .route(
                    "/invite/accept",
                    web::post().to(trust_routes::accept_trust_invite),
                )
                .route(
                    "/invite/preview",
                    web::post().to(trust_routes::preview_trust_invite),
                )
                .route(
                    "/invite/share",
                    web::post().to(trust_routes::share_trust_invite),
                )
                .route(
                    "/invite/fetch",
                    web::get().to(trust_routes::fetch_shared_invite),
                )
                .route(
                    "/invite/send-verified",
                    web::post().to(trust_routes::send_verified_invite),
                )
                .route(
                    "/invite/verify",
                    web::post().to(trust_routes::verify_invite_code),
                )
                .route(
                    "/invite/decline",
                    web::post().to(trust_routes::decline_trust_invite),
                )
                .route(
                    "/invite/declined",
                    web::get().to(trust_routes::list_declined_invites),
                )
                .route(
                    "/invite/declined/{nonce}",
                    web::delete().to(trust_routes::undecline_invite),
                )
                .route(
                    "/invite/sent",
                    web::get().to(trust_routes::list_sent_invites),
                ),
        )
        .route(
            "/schema/{name}/field/{field}/policy",
            web::put().to(trust_routes::set_field_policy),
        )
        .route(
            "/schema/{name}/field/{field}/policy",
            web::get().to(trust_routes::get_field_policy),
        )
        .route(
            "/schema/{name}/policies",
            web::get().to(trust_routes::get_all_field_policies),
        );
    }

    fn configure_identity_routes(cfg: &mut web::ServiceConfig) {
        use crate::server::routes::trust as trust_routes;

        cfg.service(
            web::scope("/identity")
                .route("/card", web::get().to(trust_routes::get_identity_card))
                .route("/card", web::put().to(trust_routes::set_identity_card)),
        )
        .service(
            web::scope("/contacts")
                .route("", web::get().to(trust_routes::list_contacts))
                .route("/{key}", web::get().to(trust_routes::get_contact))
                .route("/{key}", web::delete().to(trust_routes::revoke_contact)),
        );
    }

    fn configure_sharing_routes(cfg: &mut web::ServiceConfig) {
        use crate::server::routes::trust as trust_routes;

        cfg.service(
            web::scope("/sharing")
                .route("/roles", web::get().to(trust_routes::list_sharing_roles))
                .route("/audit/{key}", web::get().to(trust_routes::sharing_audit))
                .route(
                    "/assign/{key}",
                    web::post().to(trust_routes::assign_contact_role),
                )
                .route(
                    "/remove/{key}/{domain}",
                    web::delete().to(trust_routes::remove_contact_role),
                )
                .route("/posture", web::get().to(trust_routes::sharing_posture))
                .route(
                    "/apply-defaults",
                    web::post().to(trust_routes::apply_defaults_all),
                )
                .route(
                    "/policy/{schema}/{field}",
                    web::put().to(trust_routes::set_field_policy),
                )
                .route(
                    "/policies/{schema}",
                    web::get().to(trust_routes::get_all_field_policies),
                )
                .route("/exemem-status", web::get().to(trust_routes::exemem_status))
                .route(
                    "/rules",
                    web::post().to(crate::server::routes::sharing::create_rule),
                )
                .route(
                    "/rules",
                    web::get().to(crate::server::routes::sharing::list_rules),
                )
                .route(
                    "/rules/{id}",
                    web::delete().to(crate::server::routes::sharing::deactivate_rule),
                )
                .route(
                    "/invite",
                    web::post().to(crate::server::routes::sharing::generate_invite),
                )
                .route(
                    "/accept",
                    web::post().to(crate::server::routes::sharing::accept_invite),
                )
                .route(
                    "/pending-invites",
                    web::get().to(crate::server::routes::sharing::list_pending_invites),
                ),
        );
    }

    fn configure_remote_routes(cfg: &mut web::ServiceConfig) {
        use crate::server::routes::remote as remote_routes;

        cfg.service(
            web::scope("/remote")
                .route("/node-info", web::get().to(remote_routes::node_info))
                // Outbound: async queries via messaging service
                .route("/async-query", web::post().to(remote_routes::async_query))
                .route("/async-browse", web::post().to(remote_routes::async_browse))
                .route(
                    "/async-queries",
                    web::get().to(remote_routes::list_async_queries),
                )
                .route(
                    "/async-query/{id}",
                    web::get().to(remote_routes::get_async_query),
                )
                .route(
                    "/async-query/{id}",
                    web::delete().to(remote_routes::delete_async_query),
                ),
        );
    }

    fn configure_feed_routes(cfg: &mut web::ServiceConfig) {
        cfg.route("/feed", web::post().to(feed_routes::get_feed));
    }

    fn configure_org_routes(cfg: &mut web::ServiceConfig) {
        use super::routes::org as org_routes;

        cfg.service(
            web::scope("/org")
                .route("", web::post().to(org_routes::create_org))
                .route("", web::get().to(org_routes::list_orgs))
                .route("/join", web::post().to(org_routes::join_org))
                .route(
                    "/invites/pending",
                    web::get().to(org_routes::get_pending_invites),
                )
                .route("/{org_hash}", web::get().to(org_routes::get_org))
                .route("/{org_hash}", web::delete().to(org_routes::delete_org))
                .route("/{org_hash}/leave", web::post().to(org_routes::leave_org))
                .route(
                    "/{org_hash}/members",
                    web::post().to(org_routes::add_member),
                )
                .route(
                    "/{org_hash}/members/{node_public_key}",
                    web::delete().to(org_routes::remove_member),
                )
                .route(
                    "/{org_hash}/cloud-members",
                    web::get().to(org_routes::get_cloud_members),
                )
                .route(
                    "/{org_hash}/invite",
                    web::post().to(org_routes::generate_invite),
                )
                .route(
                    "/invites/{org_hash}/decline",
                    web::post().to(org_routes::decline_invite),
                ),
        );
    }

    fn configure_auth_routes(cfg: &mut web::ServiceConfig) {
        cfg.service(
            web::scope("/auth")
                .route("/credentials", web::get().to(auth_routes::get_credentials))
                .route(
                    "/credentials",
                    web::post().to(auth_routes::store_credentials),
                )
                .route(
                    "/credentials",
                    web::delete().to(auth_routes::delete_credentials),
                )
                .route(
                    "/exemem-config",
                    web::get().to(auth_routes::get_exemem_config),
                )
                .route(
                    "/register",
                    web::post().to(auth_routes::register_with_exemem),
                )
                .route(
                    "/recovery-phrase",
                    web::get().to(auth_routes::get_recovery_phrase),
                )
                .route("/restore", web::post().to(auth_routes::restore_from_phrase))
                .route(
                    "/restore/status",
                    web::get().to(auth_routes::restore_status),
                ),
        );
    }
}

/// Determine whether the stored Exemem session token needs refreshing.
///
/// Session tokens have the format `user_hash.timestamp.expiry.signature` where
/// `expiry` is a Unix timestamp in seconds. The token needs refreshing when the
/// remaining lifetime `(expiry - now)` is less than `min_remaining_secs`.
///
/// Returns `Err` on any parse failure — callers should treat that as "refresh
/// anyway" rather than silently skipping, so a corrupted token is recoverable.
pub(crate) fn session_token_needs_refresh(
    token: &str,
    now: i64,
    min_remaining_secs: i64,
) -> Result<bool, String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 4 {
        return Err(format!(
            "session token must have 4 dot-separated parts, got {}",
            parts.len()
        ));
    }
    let expiry: i64 = parts[2]
        .parse()
        .map_err(|e| format!("session token expiry field is not a valid integer: {e}"))?;
    Ok((expiry - now) < min_remaining_secs)
}

/// Serve embedded static assets from the React build.
/// Falls back to index.html for SPA client-side routing.
async fn serve_embedded_asset(req: HttpRequest) -> HttpResponse {
    let path = req.path();
    // Try the exact path (with leading /)
    let asset_path = if path == "/" { "/index.html" } else { path };

    if let Some(content) = Asset::get(asset_path) {
        let mime = mime_guess::from_path(asset_path).first_or_octet_stream();
        HttpResponse::Ok()
            .content_type(mime.as_ref())
            .body(content.data.into_owned())
    } else if path.starts_with("/api/") {
        // API routes that don't match any registered handler should return 404,
        // not the SPA index.html (which makes debugging confusing).
        HttpResponse::NotFound().json(serde_json::json!({
            "ok": false,
            "error": format!("No route matches {}", path)
        }))
    } else {
        // SPA fallback: return index.html for unmatched routes
        match Asset::get("/index.html") {
            Some(content) => HttpResponse::Ok()
                .content_type("text/html")
                .body(content.data.into_owned()),
            None => HttpResponse::NotFound().body("UI not available"),
        }
    }
}

#[cfg(test)]
mod session_token_tests {
    use super::session_token_needs_refresh;

    #[test]
    fn token_far_from_expiry_does_not_need_refresh() {
        // now = 1000, expiry = 1000 + 24h, threshold = 12h → plenty of time left
        let now = 1000;
        let expiry = now + 24 * 3600;
        let token = format!("userhash.{}.{}.sigsig", now, expiry);
        assert!(!session_token_needs_refresh(&token, now, 12 * 3600).unwrap());
    }

    #[test]
    fn token_near_expiry_needs_refresh() {
        // 1h remaining, threshold = 12h → needs refresh
        let now = 1000;
        let expiry = now + 3600;
        let token = format!("userhash.{}.{}.sigsig", now, expiry);
        assert!(session_token_needs_refresh(&token, now, 12 * 3600).unwrap());
    }

    #[test]
    fn malformed_token_returns_err() {
        assert!(session_token_needs_refresh("not-a-token", 0, 1).is_err());
        assert!(session_token_needs_refresh("a.b.c", 0, 1).is_err());
        assert!(session_token_needs_refresh("a.b.notanum.d", 0, 1).is_err());
    }

    #[test]
    fn expired_token_needs_refresh() {
        let now = 10_000;
        let expiry = 9_000; // already expired
        let token = format!("userhash.{}.{}.sigsig", 1000, expiry);
        assert!(session_token_needs_refresh(&token, now, 0).unwrap());
    }
}
