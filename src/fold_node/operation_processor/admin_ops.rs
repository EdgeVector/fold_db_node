use fold_db::error::{FoldDbError, FoldDbResult};
use fold_db::fold_db_core::orchestration::IndexingStatus;
use crate::fold_node::config::DatabaseConfig;
use crate::ingestion::ingestion_service::IngestionService;
use std::collections::HashMap;

use super::OperationProcessor;

impl OperationProcessor {
    // --- Logging Operations ---

    /// List logs with optional filtering.
    pub async fn list_logs(
        &self,
        since: Option<i64>,
        limit: Option<usize>,
    ) -> Vec<fold_db::logging::core::LogEntry> {
        fold_db::logging::LoggingSystem::query_logs(limit, since)
            .await
            .unwrap_or_default()
    }

    /// Get current logging configuration.
    pub async fn get_log_config(&self) -> Option<fold_db::logging::config::LogConfig> {
        fold_db::logging::LoggingSystem::get_config().await
    }

    /// Reload logging configuration from file.
    pub async fn reload_log_config(&self, path: &str) -> FoldDbResult<()> {
        fold_db::logging::LoggingSystem::reload_config_from_file(path)
            .await
            .map_err(|e| FoldDbError::Config(format!("Failed to reload log config: {}", e)))
    }

    /// Get available log features and their levels.
    pub async fn get_log_features(&self) -> Option<HashMap<String, String>> {
        fold_db::logging::LoggingSystem::get_features().await
    }

    /// Update log level for a specific feature.
    pub async fn update_log_feature_level(&self, feature: &str, level: &str) -> FoldDbResult<()> {
        fold_db::logging::LoggingSystem::update_feature_level(feature, level)
            .await
            .map_err(|e| FoldDbError::Config(format!("Failed to update log level: {}", e)))
    }

    /// Get event statistics.
    pub async fn get_event_statistics(
        &self,
    ) -> FoldDbResult<fold_db::fold_db_core::infrastructure::event_statistics::EventStatistics> {
        let db = self
            .node
            .get_fold_db()
            .await?;
        Ok(db.get_event_statistics())
    }

    /// Get indexing status.
    pub async fn get_indexing_status(&self) -> FoldDbResult<IndexingStatus> {
        let db = self
            .node
            .get_fold_db()
            .await?;
        Ok(db.get_indexing_status().await)
    }

    // --- Security Operations ---

    /// Get the node's private key
    pub fn get_node_private_key(&self) -> String {
        self.node.get_node_private_key().to_string()
    }

    /// Get the node's public key
    pub fn get_node_public_key(&self) -> String {
        self.node.get_node_public_key().to_string()
    }

    /// Get the system public key
    pub fn get_system_public_key(&self) -> FoldDbResult<Option<fold_db::security::PublicKeyInfo>> {
        let security_manager = self.node.get_security_manager();
        security_manager
            .get_system_public_key()
            .map_err(|e| FoldDbError::Other(e.to_string()))
    }

    /// Get database configuration
    pub fn get_database_config(&self) -> DatabaseConfig {
        self.node.config.database.clone()
    }

    /// Reset the database (destructive operation).
    pub async fn perform_database_reset(
        &self,
        #[allow(unused_variables)] user_id_override: Option<&str>,
    ) -> FoldDbResult<()> {
        let config = self.node.config.clone();
        let db_path = config.get_storage_path();

        if let Ok(db) = self.node.get_fold_db().await {
            if let Err(e) = db.close() {
                log::warn!("Failed to close database during reset: {}", e);
            }
        }

        match &config.database {
            #[cfg(feature = "aws-backend")]
            DatabaseConfig::Cloud(cloud_config) => {
                let aws_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
                    .region(aws_sdk_dynamodb::config::Region::new(
                        cloud_config.region.clone(),
                    ))
                    .load()
                    .await;
                let client = std::sync::Arc::new(aws_sdk_dynamodb::Client::new(&aws_config));

                let uid = user_id_override
                    .map(|s| s.to_string())
                    .or_else(fold_db::logging::core::get_current_user_id)
                    .or_else(|| cloud_config.user_id.clone())
                    .unwrap_or_else(|| self.node.get_node_public_key().to_string());

                log::info!(
                    "Resetting database for user_id={} using scan-free DynamoDbResetManager",
                    uid
                );

                let manager = fold_db::storage::reset_manager::DynamoDbResetManager::new(
                    client.clone(),
                    cloud_config.tables.clone(),
                );

                if let Err(e) = manager.reset_user(&uid).await {
                    log::error!("Failed to reset user data: {}", e);
                    return Err(FoldDbError::Other(format!(
                        "Failed to reset user data: {}",
                        e
                    )));
                }
            }
            // Both Local and Exemem use local Sled storage
            DatabaseConfig::Local { .. } | DatabaseConfig::Exemem { .. } => {
                if db_path.exists() {
                    if let Err(e) = std::fs::remove_dir_all(&db_path) {
                        log::error!("Failed to delete database folder: {}", e);
                        return Err(FoldDbError::Io(e));
                    }
                }
                if let Err(e) = std::fs::create_dir_all(&db_path) {
                    log::error!("Failed to recreate database folder: {}", e);
                    return Err(FoldDbError::Io(e));
                }
            }
        }

        Ok(())
    }

    // --- Ingestion Operations ---

    /// Scan a folder using LLM to classify files and return recommendations.
    pub async fn smart_folder_scan(
        &self,
        folder_path: &std::path::Path,
        max_depth: usize,
        max_files: usize,
    ) -> FoldDbResult<crate::ingestion::smart_folder::SmartFolderScanResponse> {
        crate::ingestion::smart_folder::perform_smart_folder_scan(
            folder_path,
            max_depth,
            max_files,
            None,
            Some(&self.node),
        )
        .await
        .map_err(|e| FoldDbError::Other(e.to_string()))
    }

    /// Ingest a single file through the AI ingestion pipeline.
    pub async fn ingest_single_file(
        &self,
        file_path: &std::path::Path,
        auto_execute: bool,
    ) -> FoldDbResult<crate::ingestion::IngestionResponse> {
        self.ingest_single_file_with_tracker(file_path, auto_execute, None)
            .await
    }

    /// Like `ingest_single_file` but accepts an optional external `ProgressTracker`.
    pub async fn ingest_single_file_with_tracker(
        &self,
        file_path: &std::path::Path,
        auto_execute: bool,
        external_tracker: Option<crate::ingestion::ProgressTracker>,
    ) -> FoldDbResult<crate::ingestion::IngestionResponse> {
        use crate::ingestion::json_processor::convert_file_to_json;
        use crate::ingestion::progress::ProgressService;
        use crate::ingestion::smart_folder;
        use crate::ingestion::IngestionRequest;

        let data = match smart_folder::read_file_as_json(file_path) {
            Ok(json) => json,
            Err(_) => convert_file_to_json(&file_path.to_path_buf())
                .await
                .map_err(|e| FoldDbError::Other(e.to_string()))?,
        };

        let progress_id = uuid::Uuid::new_v4().to_string();
        let pub_key = self.get_node_public_key();

        let request = IngestionRequest {
            data,
            auto_execute,
            pub_key,
            source_file_name: file_path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string()),
            progress_id: Some(progress_id.clone()),
            file_hash: None,
            source_folder: file_path.parent().map(|p| p.to_string_lossy().to_string()),
            image_descriptive_name: None,
        };

        let service =
            IngestionService::from_env().map_err(|e| FoldDbError::Other(e.to_string()))?;

        let progress_tracker = match external_tracker {
            Some(t) => t,
            None => crate::ingestion::create_progress_tracker(None).await,
        };
        let progress_service = ProgressService::new(progress_tracker);
        progress_service
            .start_progress(progress_id.clone(), "cli".to_string())
            .await;

        let response = service
            .process_json_with_node_and_progress(
                request,
                &self.node,
                &progress_service,
                progress_id,
            )
            .await
            .map_err(|e| FoldDbError::Other(e.to_string()))?;

        Ok(response)
    }

    // --- LLM Query Operations ---

    /// Run an LLM agent query against the database.
    pub async fn llm_query(
        &self,
        user_query: &str,
        user_hash: &str,
        max_iterations: usize,
    ) -> FoldDbResult<(
        String,
        Vec<crate::fold_node::llm_query::types::ToolCallRecord>,
    )> {
        use crate::fold_node::llm_query::service::LlmQueryService;
        use crate::ingestion::config::IngestionConfig;

        let config = IngestionConfig::from_env_allow_empty();
        let service = LlmQueryService::new(config).map_err(FoldDbError::Other)?;

        let schemas = self.list_schemas().await?;

        service
            .run_agent_query(user_query, &schemas, &self.node, user_hash, max_iterations, &[], None)
            .await
            .map_err(FoldDbError::Other)
    }

    // --- Cloud Migration Operations ---

    /// Migrate local database to Exemem cloud (S3 sync).
    ///
    /// With E2E encryption, "migration" means enabling S3 sync on the existing
    /// local Sled database. The data is already encrypted locally — we just need
    /// to force an initial sync to upload everything to S3.
    ///
    /// The caller should update the config to `DatabaseConfig::Exemem` and
    /// restart the node after this completes.
    pub async fn migrate_to_cloud(&self, api_url: &str, api_key: &str) -> FoldDbResult<()> {
        log::info!("Starting cloud sync setup: {}", api_url);

        // Create a temporary SyncEngine to perform the initial upload.
        // The existing Sled data is already encrypted — we just need to snapshot
        // it and upload to S3.
        let home = std::env::var("HOME")
            .map(std::path::PathBuf::from)
            .map_err(|e| FoldDbError::Config(format!("HOME not set: {e}")))?;
        let e2e_key_path = home.join(".fold_db/e2e.key");
        let e2e_keys = fold_db::crypto::E2eKeys::load_or_generate(&e2e_key_path)
            .await
            .map_err(|e| FoldDbError::Config(format!("Failed to load E2E keys: {e}")))?;

        let sync_setup = fold_db::sync::SyncSetup::from_exemem(api_url, api_key);
        let sync_crypto: std::sync::Arc<dyn fold_db::crypto::CryptoProvider> =
            std::sync::Arc::new(fold_db::crypto::LocalCryptoProvider::from_key(
                e2e_keys.encryption_key(),
            ));

        let http = std::sync::Arc::new(reqwest::Client::new());
        let s3 = fold_db::sync::s3::S3Client::new(http.clone());
        let auth = fold_db::sync::auth::AuthClient::new(
            http,
            sync_setup.auth_url,
            sync_setup.auth,
        );

        // Open the existing Sled database to snapshot it
        let config = self.node.config.clone();
        let db_path = config.get_storage_path();
        let db = sled::open(&db_path)
            .map_err(|e| FoldDbError::Config(format!("Failed to open sled for sync: {e}")))?;
        let base_store: std::sync::Arc<dyn fold_db::storage::traits::NamespacedStore> =
            std::sync::Arc::new(fold_db::storage::SledNamespacedStore::new(db));

        let engine = std::sync::Arc::new(fold_db::sync::SyncEngine::new(
            sync_setup.device_id,
            sync_crypto.clone(),
            s3,
            auth,
            base_store.clone(),
            fold_db::sync::SyncConfig::default(),
        ));

        // Acquire the device lock
        engine
            .acquire_lock()
            .await
            .map_err(|e| FoldDbError::Other(format!("Failed to acquire sync lock: {e}")))?;

        // Create and upload a full snapshot
        let snapshot =
            fold_db::sync::snapshot::Snapshot::create(base_store.as_ref(), engine.device_id(), 0)
                .await
                .map_err(|e| FoldDbError::Other(format!("Failed to create snapshot: {e}")))?;

        let ns_count = snapshot.namespaces.len();
        let entry_count: usize = snapshot.namespaces.iter().map(|n| n.entries.len()).sum();
        log::info!(
            "Created snapshot: {} namespaces, {} entries",
            ns_count,
            entry_count
        );

        let sealed = snapshot
            .seal(&sync_crypto)
            .await
            .map_err(|e| FoldDbError::Other(format!("Failed to seal snapshot: {e}")))?;

        // Upload as latest.enc
        let auth_client = fold_db::sync::auth::AuthClient::new(
            std::sync::Arc::new(reqwest::Client::new()),
            api_url.to_string(),
            fold_db::sync::auth::SyncAuth::ApiKey(api_key.to_string()),
        );

        let url = auth_client
            .presign_snapshot_upload("latest.enc")
            .await
            .map_err(|e| FoldDbError::Other(format!("Failed to get presigned URL: {e}")))?;

        let s3_client =
            fold_db::sync::s3::S3Client::new(std::sync::Arc::new(reqwest::Client::new()));
        s3_client
            .upload(&url, sealed)
            .await
            .map_err(|e| FoldDbError::Other(format!("Failed to upload snapshot: {e}")))?;

        // Release lock
        let _ = engine.release_lock().await;

        log::info!(
            "Cloud sync setup complete: uploaded {} namespaces ({} entries)",
            ns_count,
            entry_count
        );
        Ok(())
    }
}
