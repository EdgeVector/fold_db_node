use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use fold_db::storage::traits::TypedStore;
use serde::{Deserialize, Serialize};

use std::sync::Arc;
use tokio::sync::Mutex;

use crate::fold_node::config::NodeConfig;
use fold_db::constants::SINGLE_PUBLIC_KEY_ID;
use fold_db::crypto::{CryptoProvider, LocalCryptoProvider};
use fold_db::error::{FoldDbError, FoldDbResult};
use fold_db::fold_db_core::FoldDB;
use fold_db::org::operations as org_ops;
use fold_db::org::OrgMembership;
use fold_db::security::{PublicKeyInfo, SecurityConfig, SecurityManager};
use fold_db::sync::org_sync::SyncPartitioner;
use fold_db::{AiConfig, CloudCredentials, NodeConfigStore, NodeIdentity};

/// Result of loading a view (and its dependencies) from the schema service.
#[derive(Debug, Default, Serialize)]
pub struct ViewLoadResult {
    /// Views that were fetched and registered locally.
    pub loaded_views: Vec<String>,
    /// Schemas that were fetched and loaded locally.
    pub loaded_schemas: Vec<String>,
    /// Names that were already loaded locally (skipped).
    pub already_loaded: Vec<String>,
    /// TransformViews to register (internal, leaf-first order).
    #[serde(skip)]
    pub views_to_register: Vec<fold_db::view::types::TransformView>,
    /// Schema JSON strings to load (internal, dependency order).
    #[serde(skip)]
    pub schemas_to_load: Vec<String>,
}

/// A node in the Fold distributed database system.
///
/// FoldNode combines database storage, schema management, and networking
/// capabilities into a complete node implementation. It can operate independently
/// or as part of a network of nodes, with trust relationships defining data access.
///
/// # Features
///
/// * Schema loading and management
/// * Query and mutation execution
/// * Network communication with other nodes
/// * Permission management for schemas
/// * Request forwarding to trusted nodes
///
#[derive(Clone)]
pub struct FoldNode {
    /// The underlying database instance for data storage and operations
    pub(super) db: Arc<Mutex<FoldDB>>,
    /// Configuration settings for this node
    pub config: NodeConfig,
    /// Unique identifier for this node
    pub(super) node_id: String,
    /// Security manager for authentication and encryption
    pub(super) security_manager: Arc<SecurityManager>,
    /// The node's private key for signing operations
    pub(super) private_key: String,
    /// The node's public key for verification
    pub(super) public_key: String,
    /// E2E encryption keys (content encryption + index blinding).
    /// Stored for future passkey integration where the key may need to be refreshed.
    pub(super) e2e_keys: fold_db::crypto::E2eKeys,
}

impl FoldNode {
    /// Resolve node identity from config or persisted file.
    fn resolve_identity(config: &NodeConfig) -> FoldDbResult<(String, String)> {
        if let (Some(priv_k), Some(pub_k)) = (&config.private_key, &config.public_key) {
            Ok((priv_k.clone(), pub_k.clone()))
        } else {
            match load_persisted_identity() {
                Ok(Some((priv_k, pub_k))) => Ok((priv_k, pub_k)),
                _ => Err(FoldDbError::SecurityError(
                    "Node identity (keys) not configured and no persisted identity found. \
                    Auto-generation is disabled. Please provide identity."
                        .to_string(),
                )),
            }
        }
    }

    /// Load E2E encryption keys.
    ///
    /// If a legacy `e2e.key` file exists, uses it (backward compatibility).
    /// Otherwise, derives encryption keys from the Ed25519 identity seed —
    /// one key for everything (identity + encryption).
    async fn load_e2e_keys(
        config: &NodeConfig,
        private_key_b64: &str,
    ) -> FoldDbResult<fold_db::crypto::E2eKeys> {
        let base = if let Some(config_dir) = &config.config_dir {
            config_dir
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| config_dir.clone())
        } else {
            crate::utils::paths::folddb_home()
                .map_err(|e| FoldDbError::Config(format!("Cannot resolve FOLDDB_HOME: {e}")))?
        };
        let e2e_key_path = base.join("e2e.key");

        if e2e_key_path.exists() {
            // Legacy: read existing e2e.key file
            let bytes = crate::sensitive_io::read_sensitive(&e2e_key_path)
                .map_err(|e| FoldDbError::Config(format!("Failed to read E2E key: {}", e)))?;
            if bytes.len() != 32 {
                return Err(FoldDbError::Config(format!(
                    "E2E key file has invalid length: {} (expected 32)",
                    bytes.len()
                )));
            }
            let mut secret = [0u8; 32];
            secret.copy_from_slice(&bytes);
            log::info!("Loaded legacy e2e.key file (will be migrated in future)");
            Ok(fold_db::crypto::E2eKeys::from_secret(&secret))
        } else {
            // Derive from Ed25519 identity — one key for everything
            let seed = Self::extract_ed25519_seed(private_key_b64)?;
            let keys = fold_db::crypto::E2eKeys::from_ed25519_seed(&seed)
                .map_err(|e| FoldDbError::Config(format!("Failed to derive E2E keys: {}", e)))?;
            log::info!("E2E keys derived from node identity (no separate e2e.key)");
            Ok(keys)
        }
    }

    /// Extract the 32-byte Ed25519 seed from a base64-encoded private key.
    pub fn extract_ed25519_seed(private_key_b64: &str) -> FoldDbResult<[u8; 32]> {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(private_key_b64)
            .map_err(|e| FoldDbError::Config(format!("Invalid private key base64: {}", e)))?;
        // Ed25519 secret keys are either 32 bytes (seed) or 64 bytes (seed + public)
        if bytes.len() != 32 && bytes.len() != 64 {
            return Err(FoldDbError::Config(format!(
                "Ed25519 private key has unexpected length: {} (expected 32 or 64)",
                bytes.len()
            )));
        }
        let seed_bytes = &bytes[..32];
        let mut seed = [0u8; 32];
        seed.copy_from_slice(seed_bytes);
        Ok(seed)
    }

    /// Resolve identity and load E2E keys — shared init for both constructors.
    async fn resolve_identity_and_keys(
        config: &NodeConfig,
    ) -> FoldDbResult<(String, String, fold_db::crypto::E2eKeys)> {
        let (private_key, public_key) = Self::resolve_identity(config)?;
        let e2e_keys = Self::load_e2e_keys(config, &private_key).await?;
        Ok((private_key, public_key, e2e_keys))
    }

    /// Log schema service configuration status.
    fn log_schema_service(config: &NodeConfig) {
        if let Some(schema_service_url) = &config.schema_service_url {
            if Self::is_test_schema_service(schema_service_url) {
                log_feature!(
                    LogFeature::Database,
                    info,
                    "Mock schema service configured: {}. Schemas must be loaded manually.",
                    schema_service_url
                );
            } else {
                log_feature!(
                    LogFeature::Database,
                    info,
                    "Schema service URL configured: {}. Schemas will be loaded asynchronously after node startup.",
                    schema_service_url
                );
            }
        } else {
            log::info!("No schema service URL configured - using local schema management only");
        }
    }

    /// Assemble a FoldNode from resolved components.
    async fn assemble(
        config: NodeConfig,
        db: Arc<Mutex<FoldDB>>,
        private_key: String,
        public_key: String,
        e2e_keys: fold_db::crypto::E2eKeys,
    ) -> FoldDbResult<Self> {
        let (node_id, security_manager, security_config) =
            Self::init_internals(&config, &db).await?;

        // Register the node's public key with the verifier for signature verification
        Self::register_node_public_key(&security_manager, &public_key).await?;

        let node = Self {
            db,
            config: NodeConfig {
                security_config,
                ..config.clone()
            },
            node_id,
            security_manager,
            private_key,
            public_key,
            e2e_keys,
        };

        Self::log_schema_service(&config);

        // Migrate config files to Sled config store (one-time, idempotent)
        migrate_config_files_to_sled(&node).await;

        // Configure org sync if the sync engine is enabled and orgs exist
        node.configure_org_sync_if_needed().await;

        Ok(node)
    }

    /// Creates a new FoldNode with the specified configuration.
    pub async fn new(config: NodeConfig) -> FoldDbResult<Self> {
        let (private_key, public_key, e2e_keys) = Self::resolve_identity_and_keys(&config).await?;

        // Legacy DynamoDB Cloud backend has been removed — no user_id patching needed.

        // Build auth-refresh callback for Exemem mode so the sync engine can
        // automatically recover from expired tokens (401) by re-registering.
        let auth_refresh = if config.database.has_cloud_sync() {
            Some(crate::server::routes::auth::build_auth_refresh_callback())
        } else {
            None
        };

        let db = fold_db::fold_db_core::factory::create_fold_db_with_auth_refresh(
            &config.database,
            &e2e_keys,
            auth_refresh,
        )
        .await?;
        let node = Self::assemble(config, db, private_key, public_key, e2e_keys).await?;
        log_feature!(
            LogFeature::Database,
            info,
            "FoldNode created successfully with schema system initialized"
        );
        Ok(node)
    }

    /// Creates a new FoldNode with a pre-created FoldDB instance.
    pub async fn new_with_db(config: NodeConfig, db: Arc<Mutex<FoldDB>>) -> FoldDbResult<Self> {
        let (private_key, public_key, e2e_keys) = Self::resolve_identity_and_keys(&config).await?;
        let node = Self::assemble(config, db, private_key, public_key, e2e_keys).await?;
        log_feature!(
            LogFeature::Database,
            info,
            "FoldNode created successfully with pre-created database"
        );
        Ok(node)
    }

    /// Creates a new FoldNode with a pre-created FoldDB and explicit E2E keys.
    ///
    /// Get a reference to the underlying FoldDB instance
    pub async fn get_fold_db(&self) -> FoldDbResult<tokio::sync::OwnedMutexGuard<FoldDB>> {
        Ok(self.db.clone().lock_owned().await)
    }

    /// Gets the unique identifier for this node.
    pub fn get_node_id(&self) -> &str {
        &self.node_id
    }

    /// Gets the configured schema service URL, if present.
    pub fn schema_service_url(&self) -> Option<String> {
        self.config.schema_service_url.clone()
    }

    /// Whether a schema service URL is a test/mock URL (not a real service).
    pub fn is_test_schema_service(url: &str) -> bool {
        url.starts_with("test://") || url.starts_with("mock://")
    }

    /// Get the real (non-test) schema service URL, or error.
    fn require_real_schema_service(&self) -> FoldDbResult<String> {
        let url = self.schema_service_url().ok_or_else(|| {
            FoldDbError::Config("Schema service URL is not configured".to_string())
        })?;
        if Self::is_test_schema_service(&url) {
            return Err(FoldDbError::Config(
                "Cannot use test/mock schema service for this operation".to_string(),
            ));
        }
        Ok(url)
    }

    /// Fetch available schemas from the schema service.
    pub async fn fetch_available_schemas(
        &self,
    ) -> FoldDbResult<Vec<fold_db::schema::types::Schema>> {
        let url = self.require_real_schema_service()?;
        crate::fold_node::SchemaServiceClient::new(&url)
            .get_available_schemas()
            .await
    }

    /// Add a new schema to the schema service.
    pub async fn add_schema_to_service(
        &self,
        schema: &fold_db::schema::types::Schema,
    ) -> FoldDbResult<crate::fold_node::schema_client::AddSchemaResponse> {
        let url = self.require_real_schema_service()?;
        crate::fold_node::SchemaServiceClient::new(&url)
            .add_schema(schema, std::collections::HashMap::new())
            .await
    }

    /// Batch check whether proposed schemas can reuse existing ones.
    /// Returns empty matches for test/mock schema service URLs.
    pub async fn batch_check_schema_reuse(
        &self,
        entries: &[crate::schema_service::types::SchemaLookupEntry],
    ) -> FoldDbResult<crate::schema_service::types::BatchSchemaReuseResponse> {
        let schema_service_url = self.schema_service_url().ok_or_else(|| {
            FoldDbError::Config("Schema service URL is not configured".to_string())
        })?;

        if Self::is_test_schema_service(&schema_service_url) {
            return Ok(crate::schema_service::types::BatchSchemaReuseResponse {
                matches: std::collections::HashMap::new(),
            });
        }

        crate::fold_node::SchemaServiceClient::new(&schema_service_url)
            .batch_check_schema_reuse(entries)
            .await
    }

    /// Register a view with the global schema service.
    pub async fn add_view_to_service(
        &self,
        request: &crate::schema_service::types::AddViewRequest,
    ) -> FoldDbResult<crate::schema_service::types::AddViewResponse> {
        let url = self.require_real_schema_service()?;
        crate::fold_node::SchemaServiceClient::new(&url)
            .add_view(request)
            .await
    }

    /// Load a view from the global schema service, including all transitive
    /// dependencies (source schemas and source views).
    ///
    /// ```text
    /// load_view_from_service("ViewC")
    ///   ├─ fetch StoredView "ViewC" + its output schema
    ///   ├─ for each input_query source:
    ///   │    ├─ already local? → skip
    ///   │    ├─ schema on service? → fetch + load
    ///   │    └─ view on service? → recurse
    ///   ├─ convert StoredView → TransformView
    ///   └─ register locally
    /// ```
    ///
    /// All-or-nothing: if any dependency fails, nothing is registered.
    pub async fn load_view_from_service(&self, name: &str) -> FoldDbResult<ViewLoadResult> {
        let url = self.require_real_schema_service()?;
        let client = crate::fold_node::SchemaServiceClient::new(&url);
        let mut loading = std::collections::HashSet::new();
        let mut result = ViewLoadResult::default();

        self.load_view_recursive(&client, name, &mut loading, &mut result, 0)
            .await?;

        // All fetches succeeded — now register everything in dependency order
        // (collected_views is in leaf-first order from recursion)
        let db = self.db.lock().await;
        for schema_json in &result.schemas_to_load {
            db.schema_manager
                .load_schema_from_json(schema_json)
                .await
                .map_err(|e| {
                    FoldDbError::Config(format!("Failed to load dependency schema locally: {}", e))
                })?;
        }
        for view in &result.views_to_register {
            db.schema_manager
                .register_view(view.clone())
                .await
                .map_err(|e| {
                    FoldDbError::Config(format!(
                        "Failed to register view '{}' locally: {}",
                        view.name, e
                    ))
                })?;
        }

        Ok(result)
    }

    /// Maximum depth for recursive view loading to prevent runaway chains.
    const MAX_VIEW_LOAD_DEPTH: usize = 16;

    /// Recursively fetch a view and its dependencies from the schema service.
    /// Collects schemas and views to register (leaf-first order).
    /// Does NOT register anything — caller handles registration after all fetches succeed.
    fn load_view_recursive<'a>(
        &'a self,
        client: &'a crate::fold_node::SchemaServiceClient,
        name: &'a str,
        loading: &'a mut std::collections::HashSet<String>,
        result: &'a mut ViewLoadResult,
        depth: usize,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = FoldDbResult<()>> + Send + 'a>> {
        Box::pin(async move {
            // Depth limit
            if depth > Self::MAX_VIEW_LOAD_DEPTH {
                return Err(FoldDbError::Config(format!(
                    "View chain depth exceeds limit of {} while loading '{}'",
                    Self::MAX_VIEW_LOAD_DEPTH,
                    name
                )));
            }

            // Already being loaded in this call chain → cycle
            if !loading.insert(name.to_string()) {
                return Err(FoldDbError::Config(format!(
                    "Circular view dependency detected: '{}' is already being loaded",
                    name
                )));
            }

            // Already loaded locally → skip
            {
                let db = self.db.lock().await;
                if db.schema_manager.get_view(name)?.is_some() {
                    result.already_loaded.push(name.to_string());
                    return Ok(());
                }
            }

            // Already queued for registration in this batch → skip
            if result.views_to_register.iter().any(|v| v.name == name) {
                return Ok(());
            }

            // Fetch view from service
            let stored_view = client.get_view(name).await.map_err(|e| {
                FoldDbError::Config(format!(
                    "View '{}' not found on schema service: {}",
                    name, e
                ))
            })?;

            // Fetch output schema (needed for key_config + typed output_fields)
            let output_schema = client
                .get_schema(&stored_view.output_schema_name)
                .await
                .map_err(|e| {
                    FoldDbError::Config(format!(
                        "Output schema '{}' for view '{}' not found on service: {}",
                        stored_view.output_schema_name, name, e
                    ))
                })?;

            // Ensure output schema is loaded locally
            {
                let db = self.db.lock().await;
                if db
                    .schema_manager
                    .get_schema(&stored_view.output_schema_name)
                    .await?
                    .is_none()
                {
                    let schema_json = serde_json::to_string(&output_schema).map_err(|e| {
                        FoldDbError::Config(format!("Failed to serialize output schema: {}", e))
                    })?;
                    result.schemas_to_load.push(schema_json);
                    result
                        .loaded_schemas
                        .push(stored_view.output_schema_name.clone());
                }
            }

            // Resolve input dependencies
            for query in &stored_view.input_queries {
                let source = &query.schema_name;

                // Already loaded locally as schema or view?
                let is_local = {
                    let db = self.db.lock().await;
                    db.schema_manager.get_schema(source).await?.is_some()
                        || db.schema_manager.get_view(source)?.is_some()
                };
                if is_local {
                    result.already_loaded.push(source.clone());
                    continue;
                }

                // Already queued in this batch?
                if result.views_to_register.iter().any(|v| v.name == *source)
                    || result.loaded_schemas.contains(source)
                {
                    continue;
                }

                // Try as schema on service
                if let Ok(schema) = client.get_schema(source).await {
                    let schema_json = serde_json::to_string(&schema).map_err(|e| {
                        FoldDbError::Config(format!(
                            "Failed to serialize schema '{}': {}",
                            source, e
                        ))
                    })?;
                    result.schemas_to_load.push(schema_json);
                    result.loaded_schemas.push(source.clone());
                    continue;
                }

                // Try as view on service (recurse)
                self.load_view_recursive(client, source, loading, result, depth + 1)
                    .await?;
            }

            // Convert StoredView → TransformView
            let transform_view = Self::stored_view_to_transform_view(&stored_view, &output_schema)?;
            result.views_to_register.push(transform_view);
            result.loaded_views.push(name.to_string());

            Ok(())
        })
    }

    /// Convert a StoredView (from schema service) to a TransformView (local DB)
    /// by extracting key_config and typed output_fields from the output schema.
    fn stored_view_to_transform_view(
        stored: &crate::schema_service::types::StoredView,
        output_schema: &fold_db::schema::types::Schema,
    ) -> FoldDbResult<fold_db::view::types::TransformView> {
        use fold_db::schema::types::field_value_type::FieldValueType;

        let fields = output_schema.fields.as_deref().unwrap_or(&[]);
        let mut output_fields = std::collections::HashMap::new();
        for field_name in fields {
            let field_type = output_schema
                .field_types
                .get(field_name)
                .cloned()
                .unwrap_or(FieldValueType::Any);
            output_fields.insert(field_name.clone(), field_type);
        }

        Ok(fold_db::view::types::TransformView::new(
            &stored.name,
            stored.schema_type.clone(),
            output_schema.key.clone(),
            stored.input_queries.clone(),
            stored.wasm_bytes.clone(),
            output_fields,
        ))
    }

    /// Execute a batch of mutations.
    pub async fn mutate_batch(
        &self,
        mutations: Vec<fold_db::schema::types::operations::Mutation>,
    ) -> FoldDbResult<Vec<String>> {
        let mut db = self.db.lock().await;
        Ok(db
            .mutation_manager
            .write_mutations_batch_async(mutations)
            .await?)
    }

    async fn init_internals(
        config: &NodeConfig,
        db: &Arc<Mutex<FoldDB>>,
    ) -> FoldDbResult<(String, Arc<SecurityManager>, SecurityConfig)> {
        // Retrieve or generate the persistent node_id from fold_db
        let node_id = {
            let guard = db.lock().await;
            guard
                .get_node_id()
                .await
                .map_err(|e| FoldDbError::Config(format!("Failed to get node_id: {}", e)))?
        };

        // Initialize security manager with node configuration
        let security_config = config.security_config.clone();

        let security_manager = {
            let guard = db.lock().await;

            let db_ops = guard.db_ops.clone();

            Arc::new(
                SecurityManager::new_with_persistence(
                    config.security_config.clone(),
                    Arc::clone(&db_ops),
                )
                .await
                .map_err(|e| FoldDbError::SecurityError(e.to_string()))?,
            )
        };

        Ok((node_id, security_manager, security_config))
    }

    /// Register the node's public key with the security manager's verifier
    /// so that incoming signed messages can be verified.
    async fn register_node_public_key(
        security_manager: &Arc<SecurityManager>,
        public_key_base64: &str,
    ) -> FoldDbResult<()> {
        let key_info = PublicKeyInfo::new(
            SINGLE_PUBLIC_KEY_ID.to_string(),
            public_key_base64.to_string(),
            "system".to_string(),
            vec!["read".to_string(), "write".to_string()],
        );
        security_manager
            .verifier
            .register_system_public_key(key_info)
            .await
            .map_err(|e| FoldDbError::SecurityError(e.to_string()))?;
        log_feature!(
            LogFeature::Permissions,
            info,
            "Registered node public key for signature verification"
        );
        Ok(())
    }
}

impl FoldNode {
    /// Gets the node's private key.
    pub fn get_node_private_key(&self) -> &str {
        &self.private_key
    }

    /// Gets the node's public key.
    pub fn get_node_public_key(&self) -> &str {
        &self.public_key
    }

    /// Resolve the config directory from the node's configuration.
    pub fn get_config_dir(&self) -> Result<std::path::PathBuf, String> {
        self.config.get_config_dir()
    }

    /// Get the E2E encryption key for encrypting files before storage.
    pub fn get_encryption_key(&self) -> [u8; 32] {
        self.e2e_keys.encryption_key()
    }

    /// Gets a reference to the security manager.
    pub fn get_security_manager(&self) -> &Arc<SecurityManager> {
        &self.security_manager
    }

    /// Get the unified progress tracker
    /// This is the single source of truth for all job progress (ingestion, indexing, reset, etc.)
    /// Local deployments use Sled storage, cloud deployments use DynamoDB
    pub async fn get_progress_tracker(&self) -> fold_db::progress::ProgressTracker {
        let db = self.db.lock().await;
        db.get_progress_tracker()
    }

    /// Get the current indexing status
    pub async fn get_indexing_status(
        &self,
    ) -> fold_db::fold_db_core::orchestration::IndexingStatus {
        let db = self.db.lock().await;
        db.get_indexing_status().await
    }

    /// Check if indexing is currently in progress
    pub async fn is_indexing(&self) -> bool {
        let db = self.db.lock().await;
        db.is_indexing().await
    }

    /// Wait for all pending background tasks to complete
    pub async fn wait_for_background_tasks(&self, timeout: std::time::Duration) -> bool {
        let db = self.db.lock().await;
        db.wait_for_background_tasks(timeout).await
    }

    /// Increment pending task count manually
    pub async fn increment_pending_tasks(&self) {
        let db = self.db.lock().await;
        db.increment_pending_tasks();
    }

    /// Decrement pending task count manually
    pub async fn decrement_pending_tasks(&self) {
        let db = self.db.lock().await;
        db.decrement_pending_tasks();
    }

    /// Check if a file has already been ingested by a specific user.
    ///
    /// Returns the ingestion record if found, `None` otherwise.
    pub async fn is_file_ingested(
        &self,
        pub_key: &str,
        file_hash: &str,
    ) -> Option<FileIngestionRecord> {
        let key = format!("file:{}:{}", pub_key, file_hash);
        let db = self.db.lock().await;
        db.db_ops
            .idempotency_store()
            .get_item::<FileIngestionRecord>(&key)
            .await
            .ok()
            .flatten()
    }

    /// Record that a file has been successfully ingested by a user.
    pub async fn mark_file_ingested(
        &self,
        pub_key: &str,
        file_hash: &str,
        record: FileIngestionRecord,
    ) -> FoldDbResult<()> {
        let key = format!("file:{}:{}", pub_key, file_hash);
        let db = self.db.lock().await;
        db.db_ops
            .idempotency_store()
            .put_item(&key, &record)
            .await
            .map_err(|e| FoldDbError::Database(e.to_string()))
    }

    /// Query process results for a given progress_id.
    ///
    /// Returns actual mutation outcomes (schema_name + key_value as stored)
    /// written by the ProcessResultsSubscriber during ingestion.
    pub async fn get_process_results(
        &self,
        progress_id: &str,
    ) -> FoldDbResult<Vec<MutationOutcome>> {
        let db = self.db.lock().await;
        let prefix = format!("{}:mut:", progress_id);
        let items: Vec<(
            String,
            fold_db::fold_db_core::infrastructure::process_results_subscriber::ProcessMutationResult,
        )> = db
            .db_ops
            .process_results_store()
            .scan_items_with_prefix(&prefix)
            .await
            .map_err(|e| FoldDbError::Database(e.to_string()))?;

        Ok(items
            .into_iter()
            .map(|(key, result)| {
                let mutation_id = key.rsplit(":mut:").next().unwrap_or("").to_string();
                MutationOutcome {
                    mutation_id,
                    schema_name: result.schema_name,
                    key_value: result.key_value,
                }
            })
            .collect())
    }
}

// =========================================================================
// Org sync wiring
// =========================================================================

impl FoldNode {
    /// Configure org sync on the sync engine if sync is enabled and the node
    /// is a member of any organizations.
    ///
    /// Called automatically at startup and should be called again when the
    /// node creates, joins, or leaves an org.
    pub async fn configure_org_sync_if_needed(&self) {
        let db_guard = self.db.lock().await;

        // Check if sync is enabled
        let sync_engine = match db_guard.sync_engine() {
            Some(engine) => Arc::clone(engine),
            None => return, // Sync not configured (local mode)
        };

        // Load org memberships from Sled
        let pool = match db_guard.sled_pool() {
            Some(p) => p.clone(),
            None => return,
        };

        // Drop the db guard before async work
        drop(db_guard);

        let memberships = match org_ops::list_orgs(&pool) {
            Ok(orgs) => orgs,
            Err(e) => {
                log_feature!(
                    LogFeature::Database,
                    warn,
                    "Failed to load org memberships for sync config: {}",
                    e
                );
                return;
            }
        };

        if memberships.is_empty() {
            log_feature!(
                LogFeature::Database,
                debug,
                "No org memberships found, skipping org sync configuration"
            );
            return;
        }

        // Build sync targets and crypto providers for each org
        let mut org_targets = Vec::new();
        let mut org_crypto_pairs: Vec<(String, Arc<dyn CryptoProvider>)> = Vec::new();
        for membership in &memberships {
            match Self::crypto_provider_for_org(membership) {
                Ok(provider) => {
                    org_targets.push(fold_db::sync::org_sync::SyncTarget {
                        label: membership.org_name.clone(),
                        prefix: membership.org_hash.clone(),
                        crypto: provider.clone(),
                    });
                    org_crypto_pairs.push((membership.org_hash.clone(), provider));
                }
                Err(e) => {
                    log_feature!(
                        LogFeature::Database,
                        error,
                        "Failed to create crypto provider for org '{}': {}",
                        membership.org_name,
                        e
                    );
                }
            }
        }

        let partitioner = SyncPartitioner::new(&memberships);

        log_feature!(
            LogFeature::Database,
            info,
            "Configuring org sync: {} org(s)",
            memberships.len()
        );

        sync_engine
            .configure_org_sync(partitioner, org_targets)
            .await;

        // Register org crypto providers on the encrypting store so org-scoped
        // keys are encrypted/decrypted with the org's shared E2E key.
        let db_guard = self.db.lock().await;
        for (org_hash, crypto) in &org_crypto_pairs {
            db_guard
                .register_org_crypto(org_hash, Arc::clone(crypto))
                .await;
        }
        drop(db_guard);

        // Load persisted cursors so incremental downloads resume
        sync_engine.load_download_cursors().await;
    }

    /// Create a CryptoProvider from an org's E2E secret (base64-encoded 32-byte key).
    fn crypto_provider_for_org(
        membership: &OrgMembership,
    ) -> Result<Arc<dyn CryptoProvider>, FoldDbError> {
        let key_bytes = BASE64.decode(&membership.org_e2e_secret).map_err(|e| {
            FoldDbError::SecurityError(format!(
                "Invalid base64 org E2E secret for org '{}': {}",
                membership.org_name, e
            ))
        })?;

        if key_bytes.len() != 32 {
            return Err(FoldDbError::SecurityError(format!(
                "Org E2E secret for '{}' is {} bytes, expected 32",
                membership.org_name,
                key_bytes.len()
            )));
        }

        let mut key = [0u8; 32];
        key.copy_from_slice(&key_bytes);
        Ok(Arc::new(LocalCryptoProvider::from_key(key)))
    }

    /// Trigger an immediate sync cycle.
    ///
    /// Used after operations like org join where waiting for the timer-based
    /// sync would leave the user seeing stale/empty data.
    pub async fn trigger_immediate_sync(&self) {
        let db_guard = self.db.lock().await;
        let sync_engine = match db_guard.sync_engine() {
            Some(engine) => Arc::clone(engine),
            None => return,
        };
        drop(db_guard);

        if let Err(e) = sync_engine.sync().await {
            log::warn!("Immediate sync trigger failed: {}", e);
        }
    }

    /// Get the org sync status for a specific organization.
    ///
    /// Returns sync state, pending count, member list, and last sync time
    /// for the specified org, or None if sync is not enabled.
    pub async fn get_org_sync_status(&self, org_hash: &str) -> FoldDbResult<Option<OrgSyncStatus>> {
        let db_guard = self.db.lock().await;

        let sync_engine = match db_guard.sync_engine() {
            Some(engine) => Arc::clone(engine),
            None => return Ok(None),
        };

        let pool = match db_guard.sled_pool() {
            Some(p) => p.clone(),
            None => return Ok(None),
        };

        drop(db_guard);

        // Get the org membership
        let membership = org_ops::get_org(&pool, org_hash)?;
        let membership = match membership {
            Some(m) => m,
            None => {
                return Err(FoldDbError::Database(format!(
                    "Organization '{}' not found",
                    org_hash
                )));
            }
        };

        let has_org = sync_engine.has_org_sync().await;
        let status = sync_engine.status().await;

        let members: Vec<String> = membership
            .members
            .iter()
            .map(|m| m.display_name.clone())
            .collect();

        Ok(Some(OrgSyncStatus {
            org_hash: org_hash.to_string(),
            org_name: membership.org_name,
            sync_enabled: has_org,
            state: format!("{:?}", status.state).to_lowercase(),
            pending_count: status.pending_count,
            last_sync_at: status.last_sync_at,
            last_error: status.last_error,
            members,
        }))
    }
}

/// Sync status for a specific organization.
#[derive(Debug, Clone, Serialize)]
pub struct OrgSyncStatus {
    pub org_hash: String,
    pub org_name: String,
    pub sync_enabled: bool,
    pub state: String,
    pub pending_count: usize,
    pub last_sync_at: Option<u64>,
    pub last_error: Option<String>,
    pub members: Vec<String>,
}

/// A single mutation outcome from the process_results store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationOutcome {
    pub mutation_id: String,
    pub schema_name: String,
    pub key_value: fold_db::schema::types::KeyValue,
}

/// Metadata stored when a file is successfully ingested by a user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileIngestionRecord {
    pub ingested_at: String,
    pub source_folder: Option<String>,
    pub source_file_name: Option<String>,
    pub progress_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose;
    use fold_db::security::Ed25519KeyPair;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_node_private_key_generation() {
        let temp_dir = tempdir().unwrap();

        // Generate identity for the test
        let keypair = Ed25519KeyPair::generate().unwrap();
        let pub_key = keypair.public_key_base64();
        let priv_key = keypair.secret_key_base64();

        let config = NodeConfig::new(temp_dir.path().to_path_buf())
            .with_schema_service_url("test://mock")
            .with_identity(&pub_key, &priv_key);

        let node = FoldNode::new(config).await.unwrap();

        // Verify that private and public keys were generated (or rather, loaded correctly)
        let private_key = node.get_node_private_key();
        let public_key = node.get_node_public_key();

        assert!(!private_key.is_empty());
        assert!(!public_key.is_empty());
        assert_ne!(private_key, public_key);

        assert_eq!(private_key, priv_key);
        assert_eq!(public_key, pub_key);

        // Verify that the keys are valid base64
        assert!(general_purpose::STANDARD.decode(private_key).is_ok());
        assert!(general_purpose::STANDARD.decode(public_key).is_ok());
    }
}

// =========================================================================
// Config file → Sled migration (Phase 4)
// =========================================================================

/// Migrate config files to the Sled-backed NodeConfigStore.
///
/// This is a one-time, idempotent migration. If the Sled config store already
/// has data, we skip entirely. Otherwise we read the legacy JSON config files
/// and write their contents into Sled.
async fn migrate_config_files_to_sled(node: &FoldNode) {
    let db_guard = match node.get_fold_db().await {
        Ok(g) => g,
        Err(_) => return,
    };

    // TODO: Once the fold_db PR merges, replace this with:
    //   let store = match db_guard.config_store() { Some(s) => s, None => return };
    // For now, open the Sled tree directly via the stub.
    let pool = match db_guard.sled_pool() {
        Some(p) => p.clone(),
        None => return,
    };
    let store = match NodeConfigStore::new(pool) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("Failed to open node_config tree for migration: {}", e);
            return;
        }
    };

    if !store.is_empty() {
        return; // Already migrated
    }

    // Drop the db guard before doing file I/O
    drop(db_guard);

    let folddb_home = match crate::utils::paths::folddb_home() {
        Ok(h) => h,
        Err(_) => return,
    };

    let mut migrated_any = false;

    // 1. Migrate node identity
    let identity_path = folddb_home.join("config").join("node_identity.json");
    if identity_path.exists() {
        if let Ok(bytes) = crate::sensitive_io::read_sensitive(&identity_path) {
            if let Ok(content) = String::from_utf8(bytes) {
                #[derive(serde::Deserialize)]
                struct IdFile {
                    private_key: String,
                    public_key: String,
                }
                if let Ok(id) = serde_json::from_str::<IdFile>(&content) {
                    let identity = NodeIdentity {
                        private_key: id.private_key,
                        public_key: id.public_key,
                    };
                    if let Err(e) = store.set_identity(&identity) {
                        log::warn!("Failed to migrate node identity to Sled: {}", e);
                    } else {
                        migrated_any = true;
                    }
                }
            }
        }
    }

    // 2. Migrate cloud credentials
    if let Ok(Some(creds)) = crate::keychain::load_credentials() {
        let cloud = CloudCredentials {
            api_url: crate::endpoints::exemem_api_url(),
            api_key: creds.api_key,
            session_token: Some(creds.session_token),
            user_hash: Some(creds.user_hash),
        };
        if let Err(e) = store.set_cloud_config(&cloud) {
            log::warn!("Failed to migrate cloud credentials to Sled: {}", e);
        } else {
            migrated_any = true;
        }
    }

    // 3. Migrate AI/ingestion config
    let ingestion_path = folddb_home.join("config").join("ingestion_config.json");
    if ingestion_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&ingestion_path) {
            if let Ok(saved) =
                serde_json::from_str::<crate::ingestion::config::SavedConfig>(&content)
            {
                let provider_str = match saved.provider {
                    crate::ingestion::config::AIProvider::Anthropic => "anthropic",
                    crate::ingestion::config::AIProvider::Ollama => "ollama",
                };
                let ai = AiConfig {
                    provider: provider_str.to_string(),
                    anthropic_key: if saved.anthropic.api_key.is_empty() {
                        None
                    } else {
                        Some(saved.anthropic.api_key)
                    },
                    anthropic_model: Some(saved.anthropic.model),
                    anthropic_base_url: Some(saved.anthropic.base_url),
                    ollama_model: Some(saved.ollama.model),
                    ollama_url: Some(saved.ollama.base_url),
                    ollama_vision_model: Some(saved.ollama.vision_model),
                };
                if let Err(e) = store.set_ai_config(&ai) {
                    log::warn!("Failed to migrate AI config to Sled: {}", e);
                } else {
                    migrated_any = true;
                }
            }
        }
    }

    // 4. Migrate identity card
    if let Ok(Some(card)) = crate::trust::identity_card::IdentityCard::load() {
        if let Err(e) = store.set_display_name(&card.display_name) {
            log::warn!("Failed to migrate display_name to Sled: {}", e);
        } else {
            migrated_any = true;
        }
        if let Some(ref hint) = card.contact_hint {
            if let Err(e) = store.set_contact_hint(hint) {
                log::warn!("Failed to migrate contact_hint to Sled: {}", e);
            }
        }
    }

    if migrated_any {
        log::info!("Migrated config files to Sled config store");
    }
}

#[derive(serde::Deserialize)]
struct PersistedNodeIdentity {
    private_key: String,
    public_key: String,
}

fn load_persisted_identity() -> FoldDbResult<Option<(String, String)>> {
    // Check FOLDDB_HOME/config/node_identity.json first, fall back to relative path
    let config_path = crate::utils::paths::folddb_home()
        .map(|h| h.join("config").join("node_identity.json"))
        .unwrap_or_else(|_| std::path::PathBuf::from("config/node_identity.json"));
    if config_path.exists() {
        let bytes = crate::sensitive_io::read_sensitive(&config_path).map_err(|e| {
            FoldDbError::Config(format!("Failed to read node_identity.json: {}", e))
        })?;
        let content = String::from_utf8(bytes).map_err(|e| {
            FoldDbError::Config(format!("node_identity.json is not valid UTF-8: {}", e))
        })?;

        match serde_json::from_str::<PersistedNodeIdentity>(&content) {
            Ok(identity) => Ok(Some((identity.private_key, identity.public_key))),
            Err(e) => {
                log::warn!(
                    "Failed to parse node_identity.json: {}. Generating new identity.",
                    e
                );
                Ok(None)
            }
        }
    } else {
        Ok(None)
    }
}
