use super::middleware::auth::UserContextMiddleware;
use super::middleware::signature::SignatureVerificationMiddleware;
use super::node_manager::NodeManager;
use super::routes::log as log_routes;
use super::routes::{
    admin as admin_routes, config as config_routes, discovery as discovery_routes,
    filesystem as filesystem_routes, query as query_routes, schema as schema_routes,
    security as security_routes, system as system_routes,
};
use super::static_assets::Asset;
use crate::fold_node::llm_query;
use crate::fold_node::FoldNode;
use crate::ingestion::routes as ingestion_routes;
use crate::utils::http_errors;
use fold_db::error::{FoldDbError, FoldDbResult};

use actix_cors::Cors;
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;

use actix_web::{web, App, HttpRequest, HttpResponse, HttpServer as ActixHttpServer};
use std::sync::Arc;
use tokio::sync::RwLock;

/// HTTP server for the Fold node.
///
/// FoldHttpServer provides a web-based interface for external clients to interact
/// with a Fold node. It handles HTTP requests and can serve the built React
/// UI, and provides REST API endpoints for schemas, queries, and mutations.
///
/// # Architecture
///
/// The server now uses a lazy per-user node initialization pattern:
/// - On startup: Only configuration is loaded, no DynamoDB access
/// - On first request for a user: Node is created with user context
/// - Subsequent requests: Node is cached and reused
///
/// This aligns with Lambda's multi-tenant architecture.
///
/// # Features
///
/// * Static file serving for the UI
/// * REST API endpoints for schemas, queries, and mutations
/// * Sample data management
/// * One-click loading of sample data
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
        // Extract DynamoDB logs config from base config if using DynamoDB backend
        let base_config = node_manager.get_base_config().await;
        let logs_config = {
            match &base_config.database {
                #[cfg(feature = "aws-backend")]
                crate::fold_node::config::DatabaseConfig::Cloud(d) => {
                    // Note: user_id is NOT set here - it comes from per-request headers
                    Some((d.tables.logs.clone(), d.region.clone(), None))
                }
                _ => None,
            }
        };

        // Initialize the enhanced logging system with Cloud config if available
        fold_db::logging::LoggingSystem::init_with_fallback(logs_config).await;

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
        // Load schemas from schema service if configured
        self.load_schemas_if_configured().await?;

        // Initialize upload storage from environment config
        let upload_storage_config = fold_db::storage::config::UploadStorageConfig::from_env()
            .unwrap_or_else(|e| {
                log_feature!(
                    LogFeature::HttpServer,
                    warn,
                    "Failed to load upload storage config from env: {}. Using default.",
                    e
                );
                fold_db::storage::config::UploadStorageConfig::default()
            });

        let upload_storage = match upload_storage_config {
            fold_db::storage::config::UploadStorageConfig::Local { path } => {
                fold_db::storage::UploadStorage::local(path)
            }
        };

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

        // Create shared application state
        let app_state = web::Data::new(AppState {
            node_manager: self.node_manager.clone(),
        });

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

        // Create progress tracker based on database config
        let progress_tracker = {
            #[cfg(feature = "aws-backend")]
            {
                let run_base_config = self.node_manager.get_base_config().await;
                if let crate::fold_node::config::DatabaseConfig::Cloud(cloud_config) =
                    &run_base_config.database
                {
                    fold_db::progress::create_tracker(Some((
                        cloud_config.tables.process.clone(),
                        cloud_config.region.clone(),
                    )))
                    .await
                } else {
                    fold_db::progress::create_tracker(None).await
                }
            }
            #[cfg(not(feature = "aws-backend"))]
            {
                fold_db::progress::create_tracker(None).await
            }
        };
        let progress_tracker_data = web::Data::new(progress_tracker);

        // Start the HTTP server
        let server = ActixHttpServer::new(move || {
            // Create CORS middleware
            let cors = Cors::default()
                .allow_any_origin()
                .allow_any_method()
                .allow_any_header()
                .max_age(3600);

            // Configure custom JSON error handler
            let json_config =
                web::JsonConfig::default().error_handler(http_errors::json_error_handler);

            App::new()
                .wrap(SignatureVerificationMiddleware)
                .wrap(cors)
                .wrap(UserContextMiddleware)
                .app_data(app_state.clone())
                .app_data(llm_query_state.clone())
                .app_data(upload_storage_data.clone())
                .app_data(progress_tracker_data.clone())
                .app_data(ingestion_service_data.clone())
                .app_data(batch_controller_map_data.clone())
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
                .configure(Self::configure_trust_routes)
                .configure(Self::configure_capability_routes)
                .configure(Self::configure_remote_routes),
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
            );
    }

    fn configure_ingestion_routes(cfg: &mut web::ServiceConfig) {
        cfg.route(
            "/ingestion/process",
            web::post().to(ingestion_routes::process_json),
        )
        .route(
            "/ingestion/upload",
            web::post().to(crate::ingestion::file_handling::upload::upload_file),
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
            "/ingestion/progress/{id}",
            web::get().to(ingestion_routes::get_progress),
        )
        .route(
            "/ingestion/batch-folder",
            web::post().to(ingestion_routes::batch_folder_ingest),
        )
        .route(
            "/ingestion/smart-folder/scan",
            web::post().to(ingestion_routes::smart_folder_scan),
        )
        .route(
            "/ingestion/smart-folder/scan/{id}",
            web::get().to(ingestion_routes::get_scan_result),
        )
        .route(
            "/ingestion/smart-folder/ingest",
            web::post().to(ingestion_routes::smart_folder_ingest),
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
            web::post().to(ingestion_routes::adjust_scan_results),
        )
        .route(
            "/file/{hash}",
            web::get().to(crate::ingestion::file_handling::upload::serve_file),
        )
        .route(
            "/ingestion/ollama/models",
            web::get().to(ingestion_routes::list_ollama_models),
        )
        .route(
            "/ingestion/apple-import/status",
            web::get().to(ingestion_routes::apple_import_routes::apple_import_status),
        )
        .route(
            "/ingestion/apple-import/notes",
            web::post().to(ingestion_routes::apple_import_routes::apple_import_notes),
        )
        .route(
            "/ingestion/apple-import/reminders",
            web::post().to(ingestion_routes::apple_import_routes::apple_import_reminders),
        )
        .route(
            "/ingestion/apple-import/photos",
            web::post().to(ingestion_routes::apple_import_routes::apple_import_photos),
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

    fn configure_system_routes(cfg: &mut web::ServiceConfig) {
        cfg.route(
            "/system/status",
            web::get().to(system_routes::get_system_status),
        )
        .route(
            "/system/private-key",
            web::get().to(system_routes::get_node_private_key),
        )
        .route(
            "/system/public-key",
            web::get().to(system_routes::get_node_public_key),
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

    fn configure_discovery_routes(cfg: &mut web::ServiceConfig) {
        cfg.service(
            web::scope("/discovery")
                .route("/opt-ins", web::get().to(discovery_routes::list_opt_ins))
                .route("/opt-in", web::post().to(discovery_routes::opt_in))
                .route("/opt-out", web::post().to(discovery_routes::opt_out))
                .route("/publish", web::post().to(discovery_routes::publish))
                .route("/search", web::post().to(discovery_routes::search))
                .route("/connect", web::post().to(discovery_routes::connect))
                .route("/requests", web::get().to(discovery_routes::poll_requests)),
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
                .route("/override", web::put().to(trust_routes::set_trust_override))
                .route("/resolve/{key}", web::get().to(trust_routes::resolve_trust))
                .route("/audit", web::get().to(trust_routes::get_audit_log)),
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

    fn configure_capability_routes(cfg: &mut web::ServiceConfig) {
        use crate::server::routes::trust as trust_routes;

        cfg.service(
            web::scope("/capabilities")
                .route("/issue", web::post().to(trust_routes::issue_capability))
                .route("/revoke", web::delete().to(trust_routes::revoke_capability))
                .route(
                    "/list/{schema}/{field}",
                    web::get().to(trust_routes::list_capabilities),
                ),
        )
        .route(
            "/schema/{name}/payment-gate",
            web::put().to(trust_routes::set_payment_gate),
        )
        .route(
            "/schema/{name}/payment-gate",
            web::get().to(trust_routes::get_payment_gate),
        );
    }

    fn configure_remote_routes(cfg: &mut web::ServiceConfig) {
        use crate::server::routes::remote as remote_routes;

        cfg.service(
            web::scope("/remote")
                .route("/query", web::post().to(remote_routes::remote_query))
                .route("/node-info", web::get().to(remote_routes::node_info)),
        );
    }
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

// Helper function to get a node for a request
// This is used by route handlers
pub async fn get_node_for_request(
    app_state: &web::Data<AppState>,
    user_id: &str,
) -> Result<Arc<RwLock<FoldNode>>, FoldDbError> {
    app_state
        .node_manager
        .get_node(user_id)
        .await
        .map_err(|e| FoldDbError::Config(format!("Failed to get node for user: {}", e)))
}
