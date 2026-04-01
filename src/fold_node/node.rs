use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use fold_db::storage::traits::TypedStore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
use fold_db::sync::org_sync::{member_id_from_public_key, SyncPartitioner};

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

    /// Load or generate E2E encryption keys from the standard path.
    async fn load_e2e_keys() -> FoldDbResult<fold_db::crypto::E2eKeys> {
        let home = std::env::var("HOME")
            .map(std::path::PathBuf::from)
            .map_err(|_| FoldDbError::Config("HOME environment variable not set".to_string()))?;
        let e2e_key_path = home.join(".fold_db/e2e.key");
        fold_db::crypto::E2eKeys::load_or_generate(&e2e_key_path)
            .await
            .map_err(|e| FoldDbError::Config(format!("Failed to load E2E keys: {}", e)))
    }

    /// Resolve identity and load E2E keys — shared init for both constructors.
    async fn resolve_identity_and_keys(
        config: &NodeConfig,
    ) -> FoldDbResult<(String, String, fold_db::crypto::E2eKeys)> {
        let (private_key, public_key) = Self::resolve_identity(config)?;
        let e2e_keys = Self::load_e2e_keys().await?;
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

        // Configure org sync if the sync engine is enabled and orgs exist
        node.configure_org_sync_if_needed().await;

        Ok(node)
    }

    /// Creates a new FoldNode with the specified configuration.
    pub async fn new(#[allow(unused_mut)] mut config: NodeConfig) -> FoldDbResult<Self> {
        let (private_key, public_key, e2e_keys) = Self::resolve_identity_and_keys(&config).await?;

        // Update config with public key as user_id if not set (for DynamoDB)
        #[cfg(feature = "aws-backend")]
        if let crate::fold_node::config::DatabaseConfig::Cloud(ref mut d) = config.database {
            if d.user_id.is_none() {
                d.user_id = Some(public_key.clone());
            }
        }

        let db =
            fold_db::fold_db_core::factory::create_fold_db(&config.database, &e2e_keys).await?;
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
        let db_guard = match self.db.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                // DB is locked (e.g., during init); skip org sync config.
                // It will be retried when an org is created/joined.
                return;
            }
        };

        // Check if sync is enabled
        let sync_engine = match db_guard.sync_engine() {
            Some(engine) => Arc::clone(engine),
            None => return, // Sync not configured (local mode)
        };

        // Load org memberships from Sled
        let sled_db = match db_guard.sled_db() {
            Some(db) => db.clone(),
            None => return,
        };

        // Drop the db guard before async work
        drop(db_guard);

        let memberships = match org_ops::list_orgs(&sled_db) {
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

        // Derive member_id from the node's public key
        let pub_key_bytes = match BASE64.decode(&self.public_key) {
            Ok(bytes) => bytes,
            Err(e) => {
                log_feature!(
                    LogFeature::Database,
                    error,
                    "Failed to decode node public key for member_id: {}",
                    e
                );
                return;
            }
        };
        let member_id = member_id_from_public_key(&pub_key_bytes);

        // Build per-org crypto providers from each org's E2E secret
        let mut org_crypto: HashMap<String, Arc<dyn CryptoProvider>> = HashMap::new();
        for membership in &memberships {
            match Self::crypto_provider_for_org(membership) {
                Ok(provider) => {
                    org_crypto.insert(membership.org_hash.clone(), provider);
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
            "Configuring org sync: {} org(s), member_id={}",
            memberships.len(),
            member_id
        );

        sync_engine
            .configure_org_sync(partitioner, member_id, org_crypto)
            .await;
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

        let sled_db = match db_guard.sled_db() {
            Some(db) => db.clone(),
            None => return Ok(None),
        };

        drop(db_guard);

        // Get the org membership
        let membership = org_ops::get_org(&sled_db, org_hash)?;
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

#[derive(serde::Deserialize)]
struct NodeIdentity {
    private_key: String,
    public_key: String,
}

fn load_persisted_identity() -> FoldDbResult<Option<(String, String)>> {
    let config_path = std::path::Path::new("config/node_identity.json");
    if config_path.exists() {
        let content = std::fs::read_to_string(config_path).map_err(|e| {
            FoldDbError::Config(format!("Failed to read node_identity.json: {}", e))
        })?;

        match serde_json::from_str::<NodeIdentity>(&content) {
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
