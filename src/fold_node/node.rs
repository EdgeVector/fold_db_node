use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use serde::{Deserialize, Serialize};

use std::sync::Arc;

use crate::fold_node::config::NodeConfig;
use crate::identity::{self, NodeIdentity};
use fold_db::constants::SINGLE_PUBLIC_KEY_ID;
use fold_db::crypto::{CryptoProvider, LocalCryptoProvider};
use fold_db::error::{FoldDbError, FoldDbResult};
use fold_db::fold_db_core::FoldDB;
use fold_db::org::operations as org_ops;
use fold_db::org::OrgMembership;
use fold_db::security::{PublicKeyInfo, SecurityConfig, SecurityManager};
use fold_db::storage::SledPool;
use fold_db::sync::org_sync::SyncPartitioner;

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
    pub(super) db: Arc<FoldDB>,
    /// Configuration settings for this node
    pub config: NodeConfig,
    /// Unique identifier for this node
    pub(super) node_id: String,
    /// Security manager for authentication and encryption
    pub(super) security_manager: Arc<SecurityManager>,
    /// Ed25519 keypair backing this node. One canonical source — lives in
    /// the `node_identity` Sled tree (see [`crate::identity::IdentityStore`])
    /// and is read once at boot. Every consumer holds an [`Arc<NodeIdentity>`]
    /// clone — no string copies, no parallel representations.
    pub(super) identity: Arc<NodeIdentity>,
    /// E2E encryption keys (content encryption + index blinding).
    /// Stored for future passkey integration where the key may need to be refreshed.
    pub(super) e2e_keys: fold_db::crypto::E2eKeys,
}

impl FoldNode {
    /// Resolve the node identity from the Sled `node_identity` tree.
    ///
    /// Policy:
    /// - If the tree already has an identity, use it verbatim.
    /// - Else, if `config.seed_identity` is set (setup / restore / test
    ///   flows), persist that to the tree and return it.
    /// - Else, generate a fresh Ed25519 keypair, persist it, and return it
    ///   (fresh-install path).
    ///
    /// The Sled pool passed in is threaded through to [`FoldDB`] as well so
    /// both share the same file-lock holder — no double-open race on the
    /// same data directory.
    fn resolve_identity(config: &NodeConfig, pool: Arc<SledPool>) -> FoldDbResult<NodeIdentity> {
        let store = identity::open(pool)
            .map_err(|e| FoldDbError::SecurityError(format!("identity store: {e}")))?;
        if let Some(existing) = store
            .get()
            .map_err(|e| FoldDbError::SecurityError(format!("identity read: {e}")))?
        {
            return Ok(existing);
        }
        if let Some(seed) = &config.seed_identity {
            store
                .set(seed)
                .map_err(|e| FoldDbError::SecurityError(format!("identity seed write: {e}")))?;
            log::info!("Seeded node identity into Sled from config.seed_identity");
            return Ok(seed.clone());
        }
        let fresh = identity::generate_identity()
            .map_err(|e| FoldDbError::SecurityError(format!("identity generate: {e}")))?;
        store
            .set(&fresh)
            .map_err(|e| FoldDbError::SecurityError(format!("identity write: {e}")))?;
        log::info!("Generated fresh node identity and persisted to Sled");
        Ok(fresh)
    }

    /// Derive E2E encryption keys from the Ed25519 identity seed —
    /// one key for everything (identity + encryption).
    fn load_e2e_keys(identity: &NodeIdentity) -> FoldDbResult<fold_db::crypto::E2eKeys> {
        let seed = Self::extract_ed25519_seed(&identity.private_key)?;
        let keys = fold_db::crypto::E2eKeys::from_ed25519_seed(&seed)
            .map_err(|e| FoldDbError::Config(format!("Failed to derive E2E keys: {}", e)))?;
        log::info!("E2E keys derived from node identity (no separate e2e.key)");
        Ok(keys)
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

    /// Resolve identity and load E2E keys from a given Sled pool.
    /// Shared init for both constructors; the pool returned is the same
    /// one callers should hand to [`FoldDB`] so the identity store and
    /// the main database share one file-lock holder.
    fn resolve_identity_and_keys(
        config: &NodeConfig,
        pool: Arc<SledPool>,
    ) -> FoldDbResult<(Arc<NodeIdentity>, fold_db::crypto::E2eKeys)> {
        let identity = Arc::new(Self::resolve_identity(config, pool)?);
        let e2e_keys = Self::load_e2e_keys(&identity)?;
        Ok((identity, e2e_keys))
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
            log::debug!("No schema service URL configured - using local schema management only");
        }
    }

    /// Assemble a FoldNode from resolved components.
    async fn assemble(
        config: NodeConfig,
        db: Arc<FoldDB>,
        identity: Arc<NodeIdentity>,
        e2e_keys: fold_db::crypto::E2eKeys,
    ) -> FoldDbResult<Self> {
        let (node_id, security_manager) = Self::init_internals(&config, &db).await?;

        // Register the node's public key with the verifier for signature verification
        Self::register_node_public_key(&security_manager, &identity.public_key).await?;

        let node = Self {
            db,
            config: NodeConfig {
                // The seed_identity (if any) has already been written to
                // the Sled identity tree during resolve_identity — drop
                // the plaintext copy from the config we hand to the node
                // so no code path can read it back off the struct.
                seed_identity: None,
                ..config.clone()
            },
            node_id,
            security_manager,
            identity,
            e2e_keys,
        };

        Self::log_schema_service(&config);

        // Configure org sync if the sync engine is enabled and orgs exist
        node.configure_org_sync_if_needed().await;

        // Register the twelve Phase 1 built-in fingerprint schemas so
        // the fingerprints subsystem (personas, face extraction,
        // ingest endpoint) can serve requests. Best-effort: an
        // unreachable schema service is logged and the node still
        // boots so non-fingerprints functionality stays available.
        //
        // Test/mock schema-service URLs are skipped — unit tests that
        // don't need fingerprints don't pay the registration cost,
        // and tests that DO need it call register_phase_1_schemas
        // directly against their own in-process schema service.
        if let Some(url) = node.schema_service_url() {
            if !Self::is_test_schema_service(&url) {
                match crate::fingerprints::registration::register_phase_1_schemas(&node).await {
                    Ok(outcome) => {
                        log::info!(
                            "fingerprints: Phase 1 registration complete ({} schemas)",
                            outcome.total()
                        );

                        // Me persona bootstrap — only fires when an
                        // IdentityCard already exists on disk AND no
                        // built-in persona is present yet. Nodes that
                        // have not completed the setup wizard skip
                        // this; nodes upgrading from pre-Phase-1
                        // pick up the Me persona on their first
                        // restart after the wizard has saved a card.
                        // Subsequent restarts are no-ops because the
                        // existing Me persona trips the idempotency
                        // guard in ensure_me_persona_if_absent.
                        let card_result = match node.get_fold_db() {
                            Ok(db) => crate::trust::identity_card::IdentityCard::load(&db).await,
                            Err(e) => Err(fold_db::schema::SchemaError::InvalidData(format!(
                                "FoldDB not ready: {e}"
                            ))),
                        };
                        match card_result {
                            Ok(Some(card)) => {
                                let node_arc = Arc::new(node.clone());
                                match crate::fingerprints::self_identity::ensure_me_persona_if_absent(
                                    node_arc,
                                    card.display_name.clone(),
                                )
                                .await
                                {
                                    Ok(Some(outcome)) => {
                                        log::info!(
                                            "fingerprints: Me persona bootstrapped at startup (ps_id={})",
                                            outcome.me_persona_id
                                        );
                                    }
                                    Ok(None) => {
                                        log::debug!(
                                            "fingerprints: Me persona already present — no bootstrap needed"
                                        );
                                    }
                                    Err(e) => {
                                        log::warn!(
                                            "fingerprints: Me persona bootstrap failed: {}",
                                            e
                                        );
                                    }
                                }
                            }
                            Ok(None) => {
                                log::info!(
                                    "fingerprints: no IdentityCard on disk yet — Me persona will be \
                                     bootstrapped when the setup wizard saves a card"
                                );
                            }
                            Err(e) => {
                                log::warn!(
                                    "fingerprints: failed to load IdentityCard for Me bootstrap: {}",
                                    e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!(
                            "fingerprints: Phase 1 registration failed — subsystem dormant \
                             until next restart. Error: {}",
                            e
                        );
                    }
                }
            }
        }

        Ok(node)
    }

    /// Creates a new FoldNode with the specified configuration.
    ///
    /// Opens the Sled pool once and threads it through both the identity
    /// store and [`FoldDB`]. That guarantees one file-lock holder per
    /// data directory — two separate `SledPool::new` calls on the same
    /// path would `WouldBlock` each other.
    pub async fn new(config: NodeConfig) -> FoldDbResult<Self> {
        let pool = Arc::new(SledPool::new(config.get_storage_path()));
        let (identity, e2e_keys) = Self::resolve_identity_and_keys(&config, Arc::clone(&pool))?;

        // Inject per-device credentials from credentials.json into the DatabaseConfig.
        // credentials.json is the single source of truth for api_key and session_token.
        let mut config = config;
        if config.database.has_cloud_sync() {
            if let Ok(Some(creds)) = crate::keychain::load_credentials() {
                if let Some(ref mut cloud) = config.database.cloud_sync {
                    cloud.api_key = creds.api_key;
                    if !creds.session_token.is_empty() {
                        cloud.session_token = Some(creds.session_token);
                    }
                }
            }
        }

        // Build auth-refresh callback for Exemem mode so the sync engine can
        // automatically recover from expired tokens (401) by re-registering.
        // Pass the identity in — the callback captures it so sync never has
        // to re-read the Sled `node_identity` tree (that pool is owned by
        // this FoldNode and a second opener would race the file lock).
        let auth_refresh =
            crate::handlers::auth::auth_refresh_for(&config.database, Arc::clone(&identity));

        let db = fold_db::fold_db_core::factory::create_fold_db_with_pool_and_auth_refresh(
            &config.database,
            &e2e_keys,
            auth_refresh,
            Some(pool),
        )
        .await?;
        let node = Self::assemble(config, db, identity, e2e_keys).await?;
        log_feature!(
            LogFeature::Database,
            info,
            "FoldNode created successfully with schema system initialized"
        );
        Ok(node)
    }

    /// Creates a new FoldNode with a pre-created FoldDB instance.
    ///
    /// Reuses the pool from the provided `FoldDB` for identity resolution
    /// so both handles share the file lock. If the db was built without a
    /// pool (tests with in-memory Sled or a different backend), falls
    /// back to opening a fresh pool on `config.get_storage_path()`.
    pub async fn new_with_db(config: NodeConfig, db: Arc<FoldDB>) -> FoldDbResult<Self> {
        let pool = db
            .sled_pool()
            .cloned()
            .unwrap_or_else(|| Arc::new(SledPool::new(config.get_storage_path())));
        let (identity, e2e_keys) = Self::resolve_identity_and_keys(&config, pool)?;
        let node = Self::assemble(config, db, identity, e2e_keys).await?;
        log_feature!(
            LogFeature::Database,
            info,
            "FoldNode created successfully with pre-created database"
        );
        Ok(node)
    }

    /// Get a reference to the underlying FoldDB instance.
    pub fn get_fold_db(&self) -> FoldDbResult<Arc<FoldDB>> {
        Ok(Arc::clone(&self.db))
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
    ///
    /// The service wraps each schema in a `SchemaEnvelope` (Schema +
    /// `system: bool`). Current callers only need the raw Schema, so we
    /// flatten here. If a caller later needs to distinguish system vs
    /// user schemas, add a sibling method that returns the envelopes.
    pub async fn fetch_available_schemas(
        &self,
    ) -> FoldDbResult<Vec<fold_db::schema::types::Schema>> {
        let url = self.require_real_schema_service()?;
        let envelopes = crate::fold_node::SchemaServiceClient::new(&url)
            .get_available_schemas()
            .await?;
        Ok(envelopes.into_iter().map(|e| e.schema).collect())
    }

    /// Add a new schema to the schema service.
    pub async fn add_schema_to_service(
        &self,
        schema: &fold_db::schema::types::Schema,
    ) -> FoldDbResult<schema_service_core::types::AddSchemaResponse> {
        let url = self.require_real_schema_service()?;
        crate::fold_node::SchemaServiceClient::new(&url)
            .add_schema(schema, std::collections::HashMap::new())
            .await
    }

    /// Batch check whether proposed schemas can reuse existing ones.
    /// Returns empty matches for test/mock schema service URLs.
    pub async fn batch_check_schema_reuse(
        &self,
        entries: &[schema_service_core::types::SchemaLookupEntry],
    ) -> FoldDbResult<schema_service_core::types::BatchSchemaReuseResponse> {
        let schema_service_url = self.schema_service_url().ok_or_else(|| {
            FoldDbError::Config("Schema service URL is not configured".to_string())
        })?;

        if Self::is_test_schema_service(&schema_service_url) {
            return Ok(schema_service_core::types::BatchSchemaReuseResponse {
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
        request: &schema_service_core::types::AddViewRequest,
    ) -> FoldDbResult<schema_service_core::types::AddViewResponse> {
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
        for schema_json in &result.schemas_to_load {
            self.db
                .schema_manager()
                .load_schema_from_json(schema_json)
                .await
                .map_err(|e| {
                    FoldDbError::Config(format!("Failed to load dependency schema locally: {}", e))
                })?;
        }
        for view in &result.views_to_register {
            self.db
                .schema_manager()
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
            if self.db.schema_manager().get_view(name)?.is_some() {
                result.already_loaded.push(name.to_string());
                return Ok(());
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

            // Fetch output schema (needed for key_config + typed output_fields).
            // The `system` flag on the envelope is not load-bearing here —
            // view output schemas can be either user or system-seeded; the
            // registration path treats them the same. Unwrap to the inner
            // Schema immediately.
            let output_schema = client
                .get_schema(&stored_view.output_schema_name)
                .await
                .map_err(|e| {
                    FoldDbError::Config(format!(
                        "Output schema '{}' for view '{}' not found on service: {}",
                        stored_view.output_schema_name, name, e
                    ))
                })?
                .schema;

            // Ensure output schema is loaded locally
            if self
                .db
                .schema_manager()
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

            // Resolve input dependencies
            for query in &stored_view.input_queries {
                let source = &query.schema_name;

                // Already loaded locally as schema or view?
                let is_local = self.db.schema_manager().get_schema(source).await?.is_some()
                    || self.db.schema_manager().get_view(source)?.is_some();
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

                // Try as schema on service. Serialize the inner Schema
                // rather than the envelope — `load_schema_from_json` below
                // parses into `Schema`, and the `system` flag is not needed
                // on the local side.
                if let Ok(envelope) = client.get_schema(source).await {
                    let schema_json = serde_json::to_string(&envelope.schema).map_err(|e| {
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
    /// Triggers pass through unchanged: `schema_service_core::types::Trigger`
    /// is a re-export of `fold_db::triggers::types::Trigger` after the
    /// trigger-type consolidation (fold_db PR #587, schema_service PR #19).
    fn stored_view_to_transform_view(
        stored: &schema_service_core::types::StoredView,
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

        // MDT-E: fold_db::view::types::TransformView expects
        // Option<WasmTransformSpec> (bytes + fuel ceiling) where it previously
        // took Option<Vec<u8>>. StoredView still carries raw `wasm_bytes` as a
        // dev/legacy fallback, so wrap with a default fuel ceiling when
        // converting. The registered-transform path (stored.transform_hash)
        // carries max_gas via the TransformRecord and will be preferred once
        // `TransformResolver` threads it through — tracked in
        // `projects/transform-worker-split` follow-ups.
        const STORED_VIEW_DEFAULT_MAX_GAS: u64 = 1_000_000_000;
        let wasm_transform_spec =
            stored
                .wasm_bytes
                .clone()
                .map(|bytes| fold_db::view::types::WasmTransformSpec {
                    bytes,
                    max_gas: STORED_VIEW_DEFAULT_MAX_GAS,
                    gas_model: None,
                });
        let mut transform_view = fold_db::view::types::TransformView::new(
            &stored.name,
            stored.schema_type.clone(),
            output_schema.key.clone(),
            stored.input_queries.clone(),
            wasm_transform_spec,
            output_fields,
        );

        transform_view.triggers = stored.triggers.clone();

        Ok(transform_view)
    }

    /// Execute a batch of mutations.
    pub async fn mutate_batch(
        &self,
        mutations: Vec<fold_db::schema::types::operations::Mutation>,
    ) -> FoldDbResult<Vec<String>> {
        Ok(self
            .db
            .mutation_manager()
            .write_mutations_batch_async(mutations)
            .await?)
    }

    async fn init_internals(
        _config: &NodeConfig,
        db: &Arc<FoldDB>,
    ) -> FoldDbResult<(String, Arc<SecurityManager>)> {
        // Retrieve or generate the persistent node_id from fold_db
        let node_id = db
            .get_node_id()
            .await
            .map_err(|e| FoldDbError::Config(format!("Failed to get node_id: {}", e)))?;

        // Build SecurityConfig from env at init time. It's only used here,
        // not stored back on the node — keeping a cached copy on NodeConfig
        // just gave the struct a dead field.
        let security_config = SecurityConfig::from_env();
        let db_ops = db.db_ops().clone();
        let security_manager = Arc::new(
            SecurityManager::new_with_persistence(security_config, Arc::clone(&db_ops))
                .await
                .map_err(|e| FoldDbError::SecurityError(e.to_string()))?,
        );

        Ok((node_id, security_manager))
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
    /// Access the node's identity (shared `Arc<NodeIdentity>`). Prefer
    /// this over the string getters when you need both keys — it's a
    /// single Arc clone and avoids copying either base64 string.
    pub fn identity(&self) -> &Arc<NodeIdentity> {
        &self.identity
    }

    /// Gets the node's private key.
    pub fn get_node_private_key(&self) -> &str {
        &self.identity.private_key
    }

    /// Gets the node's public key.
    pub fn get_node_public_key(&self) -> &str {
        &self.identity.public_key
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
    pub fn get_progress_tracker(&self) -> fold_db::progress::ProgressTracker {
        self.db.get_progress_tracker()
    }

    /// Get the current indexing status
    pub async fn get_indexing_status(
        &self,
    ) -> fold_db::fold_db_core::orchestration::IndexingStatus {
        self.db.get_indexing_status().await
    }

    /// Check if indexing is currently in progress
    pub async fn is_indexing(&self) -> bool {
        self.db.is_indexing().await
    }

    /// Wait for all pending background tasks to complete
    pub async fn wait_for_background_tasks(&self, timeout: std::time::Duration) -> bool {
        self.db.wait_for_background_tasks(timeout).await
    }

    /// Increment pending task count manually
    pub fn increment_pending_tasks(&self) {
        self.db.increment_pending_tasks();
    }

    /// Decrement pending task count manually
    pub fn decrement_pending_tasks(&self) {
        self.db.decrement_pending_tasks();
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
        self.db
            .db_ops()
            .get_idempotency_item::<FileIngestionRecord>(&key)
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
        self.db
            .db_ops()
            .put_idempotency_item(&key, &record)
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
        let prefix = format!("{}:mut:", progress_id);
        let items: Vec<(
            String,
            fold_db::fold_db_core::process_results_subscriber::ProcessMutationResult,
        )> = self
            .db
            .db_ops()
            .scan_process_results(&prefix)
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

/// Result of building per-org sync targets from the on-disk org memberships.
///
/// Shared between the normal startup path (`configure_org_sync_if_needed`) and
/// the restore path (`bootstrap_from_cloud`) so both install exactly the same
/// set of targets and crypto providers on the `SyncEngine`.
pub(crate) struct OrgSyncConfig {
    pub partitioner: SyncPartitioner,
    pub targets: Vec<fold_db::sync::org_sync::SyncTarget>,
    /// `(org_hash, crypto_provider)` pairs, one per successfully-built target.
    /// Used to register org crypto providers on the encrypting store.
    pub crypto_pairs: Vec<(String, Arc<dyn CryptoProvider>)>,
    pub membership_count: usize,
}

/// Build the org-sync configuration from the org memberships stored in Sled.
///
/// Returns `Ok(None)` if there are no org memberships (caller should skip org
/// sync setup). Returns `Err` if the Sled lookup itself fails — memberships
/// whose crypto provider cannot be built are logged loudly and skipped (they
/// do not fail the whole restore).
pub(crate) fn build_org_sync_config_from_sled(
    pool: &Arc<fold_db::storage::SledPool>,
) -> FoldDbResult<Option<OrgSyncConfig>> {
    let memberships = org_ops::list_orgs(pool)?;
    let share_rules = fold_db::sharing::store::list_share_rules(pool).unwrap_or_default();
    let share_subscriptions =
        fold_db::sharing::store::list_share_subscriptions(pool).unwrap_or_default();

    if memberships.is_empty() && share_rules.is_empty() && share_subscriptions.is_empty() {
        return Ok(None);
    }

    let mut targets = Vec::new();
    let mut crypto_pairs: Vec<(String, Arc<dyn CryptoProvider>)> = Vec::new();
    for membership in &memberships {
        match FoldNode::crypto_provider_for_org(membership) {
            Ok(provider) => {
                targets.push(fold_db::sync::org_sync::SyncTarget {
                    label: membership.org_name.clone(),
                    prefix: membership.org_hash.clone(),
                    crypto: provider.clone(),
                });
                crypto_pairs.push((membership.org_hash.clone(), provider));
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

    for rule in &share_rules {
        if !rule.active {
            continue;
        }

        let mut key = [0u8; 32];
        let bytes = &rule.share_e2e_secret;
        if bytes.len() == 32 {
            key.copy_from_slice(bytes);
            let provider = Arc::new(fold_db::crypto::LocalCryptoProvider::from_key(key));
            targets.push(fold_db::sync::org_sync::SyncTarget {
                label: format!("share -> {}", rule.recipient_display_name),
                prefix: rule.share_prefix.clone(),
                crypto: provider.clone(),
            });
            crypto_pairs.push((rule.share_prefix.clone(), provider));
        }
    }

    for sub in &share_subscriptions {
        if !sub.active {
            continue;
        }

        let mut key = [0u8; 32];
        let bytes = &sub.share_e2e_secret;
        if bytes.len() == 32 {
            key.copy_from_slice(bytes);
            let provider = Arc::new(fold_db::crypto::LocalCryptoProvider::from_key(key));
            targets.push(fold_db::sync::org_sync::SyncTarget {
                label: format!("share <- {}", sub.sender_pubkey),
                prefix: sub.share_prefix.clone(),
                crypto: provider.clone(),
            });
            crypto_pairs.push((sub.share_prefix.clone(), provider));
        }
    }

    let partitioner = SyncPartitioner::new(&memberships, &share_rules);

    Ok(Some(OrgSyncConfig {
        partitioner,
        targets,
        crypto_pairs,
        membership_count: memberships.len(),
    }))
}

impl FoldNode {
    /// Configure org sync on the sync engine if sync is enabled and the node
    /// is a member of any organizations.
    ///
    /// Called automatically at startup and should be called again when the
    /// node creates, joins, or leaves an org.
    pub async fn configure_org_sync_if_needed(&self) {
        // Check if sync is enabled
        let sync_engine = match self.db.sync_engine() {
            Some(engine) => engine,
            None => return, // Sync not configured (local mode)
        };

        // Load org memberships from Sled
        let pool = match self.db.sled_pool() {
            Some(p) => p.clone(),
            None => return,
        };

        let org_config = match build_org_sync_config_from_sled(&pool) {
            Ok(Some(cfg)) => cfg,
            Ok(None) => {
                log_feature!(
                    LogFeature::Database,
                    debug,
                    "No org memberships found, skipping org sync configuration"
                );
                return;
            }
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

        let OrgSyncConfig {
            partitioner,
            targets: org_targets,
            crypto_pairs: org_crypto_pairs,
            membership_count,
        } = org_config;

        log_feature!(
            LogFeature::Database,
            info,
            "Configuring org sync: {} org(s)",
            membership_count
        );

        sync_engine
            .configure_org_sync(partitioner, org_targets)
            .await;

        // Register org crypto providers on the encrypting store so org-scoped
        // keys are encrypted/decrypted with the org's shared E2E key.
        for (org_hash, crypto) in &org_crypto_pairs {
            self.db
                .register_org_crypto(org_hash, Arc::clone(crypto))
                .await;
        }

        // Load persisted cursors so incremental downloads resume
        sync_engine.load_download_cursors().await;
    }

    /// Create a CryptoProvider from an org's E2E secret (base64-encoded 32-byte key).
    pub(crate) fn crypto_provider_for_org(
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
        let sync_engine = match self.db.sync_engine() {
            Some(engine) => engine,
            None => return,
        };

        if let Err(e) = sync_engine.sync().await {
            log::warn!("Immediate sync trigger failed: {}", e);
        }
    }

    /// Get the org sync status for a specific organization.
    ///
    /// Returns sync state, pending count, member list, and last sync time
    /// for the specified org, or None if sync is not enabled.
    pub async fn get_org_sync_status(&self, org_hash: &str) -> FoldDbResult<Option<OrgSyncStatus>> {
        let sync_engine = match self.db.sync_engine() {
            Some(engine) => engine,
            None => return Ok(None),
        };

        let pool = match self.db.sled_pool() {
            Some(p) => p.clone(),
            None => return Ok(None),
        };

        // Get the org membership
        let membership = org_ops::get_org(&pool, org_hash)?;
        let mut membership = match membership {
            Some(m) => m,
            None => {
                return Err(FoldDbError::Database(format!(
                    "Organization '{}' not found",
                    org_hash
                )));
            }
        };

        // Reconcile against the cloud-authoritative member roster so the status
        // reflects peers who joined via the cloud reconciler (e.g. Bob joining
        // after Alice created the org). The local `org_memberships` sled tree
        // is not mutated — placeholder entries live only in the response.
        if let Some(client) = crate::handlers::org::auth_client_for_node(self).await {
            crate::handlers::org::merge_cloud_members_into(&client, &mut membership).await;
        }

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
            state: sync_state_label(&status.state).to_string(),
            pending_count: status.pending_count,
            last_sync_at: status.last_sync_at,
            last_error: status.last_error,
            members,
        }))
    }
}

/// Convert a `SyncState` to a stable string label for API responses.
///
/// Uses explicit match arms (not Debug formatting) so the API contract
/// is independent of how the enum prints in debug output.
pub fn sync_state_label(state: &fold_db::sync::SyncState) -> &'static str {
    match state {
        fold_db::sync::SyncState::Idle => "idle",
        fold_db::sync::SyncState::Dirty => "dirty",
        fold_db::sync::SyncState::Syncing => "syncing",
        fold_db::sync::SyncState::Offline => "offline",
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
            .with_seed_identity(crate::identity::identity_from_keypair(&keypair));

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

    /// `build_org_sync_config_from_sled` returns `Ok(None)` on an empty Sled.
    ///
    /// Regression guard for the two-phase bootstrap restore path: the restore
    /// code must distinguish "no orgs" from "error loading orgs" so it can
    /// skip phase 2 cleanly when the user has no org memberships.
    #[tokio::test]
    async fn build_org_sync_config_empty_sled_returns_none() {
        let temp_dir = tempdir().unwrap();
        let pool = Arc::new(fold_db::storage::SledPool::new(
            temp_dir.path().join("sled"),
        ));

        let result = build_org_sync_config_from_sled(&pool)
            .expect("build_org_sync_config_from_sled should succeed on empty sled");
        assert!(result.is_none(), "expected None on empty sled, got Some");
    }

    /// `build_org_sync_config_from_sled` reads `OrgMembership` rows that were
    /// just written to Sled and produces a fully populated `OrgSyncConfig`
    /// — one `SyncTarget` per membership, with matching crypto pairs.
    ///
    /// This is the phase 1.5 integration point of the two-phase bootstrap
    /// restore: after phase 1 replays the personal log into Sled, this
    /// function reads the resulting org memberships and hands the SyncEngine
    /// the targets it needs for phase 2.
    #[tokio::test]
    async fn build_org_sync_config_reads_memberships_from_sled() {
        let temp_dir = tempdir().unwrap();
        let pool = Arc::new(fold_db::storage::SledPool::new(
            temp_dir.path().join("sled"),
        ));

        // Create two orgs directly in Sled (simulating what phase 1 replay
        // would produce from restored log entries).
        let creator_kp = Ed25519KeyPair::generate().unwrap();
        let creator_pub = creator_kp.public_key_base64();

        let org_a = fold_db::org::operations::create_org(&pool, "Alpha", &creator_pub, "Tom")
            .expect("create_org Alpha");
        let org_b = fold_db::org::operations::create_org(&pool, "Bravo", &creator_pub, "Tom")
            .expect("create_org Bravo");

        let cfg = build_org_sync_config_from_sled(&pool)
            .expect("build should succeed")
            .expect("expected Some — two memberships present");

        assert_eq!(cfg.membership_count, 2);
        assert_eq!(cfg.targets.len(), 2);
        assert_eq!(cfg.crypto_pairs.len(), 2);

        let target_prefixes: Vec<&str> = cfg.targets.iter().map(|t| t.prefix.as_str()).collect();
        assert!(target_prefixes.contains(&org_a.org_hash.as_str()));
        assert!(target_prefixes.contains(&org_b.org_hash.as_str()));

        let crypto_hashes: Vec<&str> = cfg.crypto_pairs.iter().map(|(h, _)| h.as_str()).collect();
        assert!(crypto_hashes.contains(&org_a.org_hash.as_str()));
        assert!(crypto_hashes.contains(&org_b.org_hash.as_str()));
    }
}
