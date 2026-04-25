use crate::fold_node::config::DatabaseConfig;
use crate::ingestion::ingestion_service::IngestionService;
use fold_db::error::{FoldDbError, FoldDbResult};
use fold_db::fold_db_core::orchestration::IndexingStatus;
use fold_db::storage::SledPool;
use std::collections::HashMap;
use std::sync::Arc;

/// Raw `(key, value)` pairs read from a Sled tree, used by the reset
/// flow to round-trip the preserved trees through a directory wipe.
type StashedEntries = Vec<(Vec<u8>, Vec<u8>)>;

/// Sled trees that survive a database reset.
///
/// `node_identity` holds the device's Ed25519 keypair (from which the
/// E2E sync key is derived). Wiping it would force a fresh identity on
/// next start, which means the device can no longer decrypt anything it
/// previously wrote — even if the data weren't already gone.
///
/// `org_memberships` holds each joined org's `org_e2e_secret`. Losing
/// them disconnects the user from every org without a server-side
/// "leave org" call; the user would have to re-accept invites to regain
/// access, even though they're still members on the cloud.
const RESET_PRESERVED_TREES: &[&str] = &["node_identity", "org_memberships"];

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
    ) -> FoldDbResult<fold_db::fold_db_core::event_statistics::EventStatistics> {
        let db = self.get_db()?;
        Ok(db.get_event_statistics())
    }

    /// Get indexing status.
    pub async fn get_indexing_status(&self) -> FoldDbResult<IndexingStatus> {
        let db = self.get_db()?;
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
    ///
    /// Wipes user data (molecules, schemas, atoms, sync cursors, all
    /// other Sled trees) AND, when cloud sync is enabled, deletes the
    /// remote sync log + snapshots so the next sync cycle does not
    /// re-bootstrap the just-deleted state.
    ///
    /// Two trees are preserved across the reset:
    /// - `node_identity` — the Ed25519 keypair (E2E key derives from it)
    /// - `org_memberships` — joined orgs and their shared E2E secrets
    ///
    /// Org cloud logs (`{org_hash}/log/*`) are NOT touched. They are
    /// shared state across org members; leaving an org is a separate
    /// flow (`org_service`, not reset).
    ///
    /// Failure modes:
    /// - **Cloud purge fails**: returns an error WITHOUT wiping local
    ///   data. The opposite ordering would leave the user with an empty
    ///   local DB plus an intact cloud log — the next sync would replay
    ///   the cloud log on top, producing exactly the bug this method
    ///   exists to fix.
    /// - **Stash read fails / restore write fails**: also surfaced as
    ///   errors. Identity loss is irreversible without the recovery
    ///   phrase, so we'd rather abort than silently regenerate.
    pub async fn perform_database_reset(
        &self,
        #[allow(unused_variables)] user_id_override: Option<&str>,
    ) -> FoldDbResult<()> {
        let config = self.node.config.clone();
        let db_path = config.get_storage_path();

        // Hold a clone of the running FoldDB (and its SledPool) so we
        // can read the preserved trees AND drive the cloud purge before
        // wiping the directory. Both must happen with the live pool
        // because the tree contents and the SyncEngine become
        // unreachable once the dir is gone.
        let fold_db = self
            .node
            .get_fold_db()
            .map_err(|e| FoldDbError::Config(format!("Failed to get FoldDB: {e}")))?;
        let pool = fold_db
            .sled_pool()
            .ok_or_else(|| {
                FoldDbError::Config(
                    "FoldDB has no SledPool — cannot reset a non-Sled-backed database".into(),
                )
            })?
            .clone();

        // === Step 1: Stash the preserved trees as raw bytes. ===
        //
        // Raw byte copy avoids any decrypt/re-encrypt round-trip
        // (identity is encrypted with the OS keychain master key when
        // `os-keychain` is on, plaintext otherwise — we don't care
        // which, the bytes are valid in the new tree as-is).
        let mut stashed: Vec<(&str, StashedEntries)> =
            Vec::with_capacity(RESET_PRESERVED_TREES.len());
        for name in RESET_PRESERVED_TREES {
            let entries = read_tree_raw(&pool, name)?;
            log::info!("reset: stashed {} entries from '{}'", entries.len(), name);
            stashed.push((name, entries));
        }

        // === Step 2: Stop sync + purge personal cloud log. ===
        //
        // `stop_sync` aborts the background timer so no new entries
        // race the purge. `purge_personal_log` then takes the device
        // lock and deletes every `{user_hash}/log/*.enc` and every
        // `{user_hash}/snapshots/*.enc`. Org prefixes are untouched.
        if let Some(engine) = fold_db.sync_engine() {
            if let Err(e) = fold_db.stop_sync().await {
                // Final-sync failures are non-fatal here — the purge
                // will delete whatever was/wasn't uploaded.
                log::warn!("reset: stop_sync returned error (continuing with purge): {e}");
            }
            match engine.purge_personal_log().await {
                Ok(outcome) => log::info!(
                    "reset: purged cloud log ({} log objects, {} snapshots)",
                    outcome.deleted_log_objects,
                    outcome.deleted_snapshots,
                ),
                Err(e) => {
                    return Err(FoldDbError::Other(format!(
                        "Failed to purge cloud sync log: {e} — local data NOT wiped \
                         (cloud would otherwise restore it on next sync). \
                         Check connectivity and retry."
                    )));
                }
            }
        }

        // === Step 3: Drop refs to the running pool, wipe the dir. ===
        //
        // The running pool's Arc is shared with the FoldNode (which
        // outlives us — `node_manager.invalidate_all_nodes()` runs
        // *after* this method returns), so we don't get exclusive
        // ownership of the flock here. We drop our own clones, wipe
        // the directory anyway (the existing pre-fix reset also did
        // this and worked in production), and let the new standalone
        // pool below acquire a flock on the freshly-created inode.
        drop(pool);
        drop(fold_db);
        if let Ok(db) = self.get_db() {
            drop(db);
        }

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

        // === Step 4: Restore preserved trees into a fresh pool. ===
        //
        // Standalone pool acquires a brand-new flock on the freshly
        // recreated directory inode (the running pool's flock, if it
        // still exists, is on the deleted inode and doesn't conflict).
        // Drop releases the flock so `invalidate_all_nodes()` can build
        // a fresh FoldNode against the same path.
        let restore_pool = Arc::new(SledPool::new(db_path.clone()));
        for (name, entries) in &stashed {
            write_tree_raw(&restore_pool, name, entries)?;
        }
        drop(restore_pool);

        log::info!(
            "reset: complete — identity preserved, {} org memberships preserved, cloud log purged",
            stashed
                .iter()
                .find(|(n, _)| *n == "org_memberships")
                .map(|(_, e)| e.len())
                .unwrap_or(0)
        );
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
            Some(self.node.as_ref()),
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
        self.ingest_single_file_with_tracker(file_path, auto_execute, None, None)
            .await
    }

    /// Like `ingest_single_file` but accepts an optional external `ProgressTracker`
    /// and optional `org_hash` for org-scoped ingestion.
    pub async fn ingest_single_file_with_tracker(
        &self,
        file_path: &std::path::Path,
        auto_execute: bool,
        external_tracker: Option<crate::ingestion::ProgressTracker>,
        org_hash: Option<String>,
    ) -> FoldDbResult<crate::ingestion::IngestionResponse> {
        use crate::ingestion::file_handling::json_processor::convert_file_to_json;
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
            org_hash,
            image_bytes: None,
        };

        let service =
            IngestionService::from_env().map_err(|e| FoldDbError::Other(e.to_string()))?;

        let progress_tracker = match external_tracker {
            Some(t) => t,
            None => crate::ingestion::create_progress_tracker().await,
        };
        let progress_service = ProgressService::new(progress_tracker);
        progress_service
            .start_progress(progress_id.clone(), "cli".to_string())
            .await;

        let response = service
            .process_json_with_node_and_progress(
                request,
                self.node.as_ref(),
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

        let config = IngestionConfig::load_or_default();
        let service = LlmQueryService::new(config).map_err(FoldDbError::Other)?;

        let schemas = self.list_schemas().await?;

        service
            .run_agent_query(
                user_query,
                &schemas,
                self.node.as_ref(),
                user_hash,
                max_iterations,
                &[],
                None,
            )
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
    /// The caller should update the config to enable `cloud_sync` and
    /// restart the node after this completes.
    pub async fn migrate_to_cloud(&self, api_url: &str, api_key: &str) -> FoldDbResult<()> {
        log::info!("Starting cloud sync setup: {}", api_url);

        // Create a temporary SyncEngine to perform the initial upload.
        // The existing Sled data is already encrypted — we just need to snapshot
        // it and upload to S3.
        // Derive E2E keys from the node's Ed25519 identity (unified identity).
        let e2e_keys = {
            let priv_key = &self.node.identity.private_key;
            let seed = crate::fold_node::FoldNode::extract_ed25519_seed(priv_key)
                .map_err(|e| FoldDbError::Config(format!("Failed to extract seed: {e}")))?;
            fold_db::crypto::E2eKeys::from_ed25519_seed(&seed)
                .map_err(|e| FoldDbError::Config(format!("Failed to derive E2E keys: {e}")))?
        };

        let data_dir = std::env::var("FOLD_STORAGE_PATH").unwrap_or_else(|_| "data".to_string());
        let sync_setup = fold_db::sync::SyncSetup::from_exemem(api_url, api_key, &data_dir);
        let sync_crypto: std::sync::Arc<dyn fold_db::crypto::CryptoProvider> = std::sync::Arc::new(
            fold_db::crypto::LocalCryptoProvider::from_key(e2e_keys.encryption_key()),
        );

        let http = std::sync::Arc::new(reqwest::Client::new());
        let s3 = fold_db::sync::s3::S3Client::new(http.clone());
        let auth = fold_db::sync::auth::AuthClient::new(http, sync_setup.auth_url, sync_setup.auth);

        // Reuse the running FoldDB's SledPool for the snapshot read.
        // Opening a bespoke `SledPool::new(db_path)` here would hold a
        // second OS file lock on the same directory as the main HTTP
        // server's pool and fail with `WouldBlock`.
        let pool = self
            .node
            .get_fold_db()
            .map_err(|e| FoldDbError::Config(format!("Failed to get FoldDB: {e}")))?
            .sled_pool()
            .ok_or_else(|| {
                FoldDbError::Config(
                    "Running FoldDB has no SledPool — cannot snapshot for migration".to_string(),
                )
            })?
            .clone();
        let base_store: std::sync::Arc<dyn fold_db::storage::traits::NamespacedStore> =
            std::sync::Arc::new(fold_db::storage::SledNamespacedStore::new(pool));

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

/// Read every `(key, value)` from `tree_name` as raw bytes.
///
/// Returns an empty vec if the tree doesn't exist yet (e.g., fresh
/// install before any identity is generated). Used by
/// [`OperationProcessor::perform_database_reset`] to stash preserved
/// trees before wiping the storage directory.
fn read_tree_raw(pool: &Arc<SledPool>, tree_name: &str) -> FoldDbResult<StashedEntries> {
    let guard = pool
        .acquire_arc()
        .map_err(|e| FoldDbError::Database(format!("acquire pool to read '{tree_name}': {e}")))?;
    let tree = guard
        .db()
        .open_tree(tree_name)
        .map_err(|e| FoldDbError::Database(format!("open tree '{tree_name}' for read: {e}")))?;
    let mut entries = Vec::new();
    for entry in tree.iter() {
        let (k, v) =
            entry.map_err(|e| FoldDbError::Database(format!("iterate '{tree_name}': {e}")))?;
        entries.push((k.to_vec(), v.to_vec()));
    }
    Ok(entries)
}

/// Write each `(key, value)` raw byte pair into `tree_name`.
///
/// The tree is created if it doesn't exist. Flushed before returning so
/// the bytes are durable before the standalone pool's flock releases.
fn write_tree_raw(
    pool: &Arc<SledPool>,
    tree_name: &str,
    entries: &StashedEntries,
) -> FoldDbResult<()> {
    let guard = pool
        .acquire_arc()
        .map_err(|e| FoldDbError::Database(format!("acquire pool to write '{tree_name}': {e}")))?;
    let tree = guard
        .db()
        .open_tree(tree_name)
        .map_err(|e| FoldDbError::Database(format!("open tree '{tree_name}' for write: {e}")))?;
    for (k, v) in entries {
        tree.insert(k.as_slice(), v.as_slice())
            .map_err(|e| FoldDbError::Database(format!("insert into '{tree_name}': {e}")))?;
    }
    tree.flush()
        .map_err(|e| FoldDbError::Database(format!("flush '{tree_name}': {e}")))?;
    Ok(())
}

#[cfg(test)]
mod reset_helpers_tests {
    //! Unit tests for the raw-byte tree round-trip helpers used by
    //! [`OperationProcessor::perform_database_reset`].
    //!
    //! End-to-end testing of `perform_database_reset` itself requires a
    //! full FoldNode + SyncEngine stack — that lives in integration
    //! tests. These tests cover the part that's pure file I/O so the
    //! reset's preserve-and-restore contract has unit-level coverage.

    use super::*;

    fn temp_pool() -> (Arc<SledPool>, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let pool = Arc::new(SledPool::new(tmp.path().to_path_buf()));
        (pool, tmp)
    }

    #[test]
    fn read_then_write_round_trips_node_identity() {
        let (pool, _tmp) = temp_pool();
        // Seed the source tree with two entries.
        {
            let guard = pool.acquire_arc().unwrap();
            let tree = guard.db().open_tree("node_identity").unwrap();
            tree.insert(b"private_key", b"priv-bytes".as_ref()).unwrap();
            tree.insert(b"public_key", b"pub-bytes".as_ref()).unwrap();
            tree.flush().unwrap();
        }

        // Stash, then restore into a fresh pool against a different
        // path — this models the wipe-and-recreate flow that
        // `perform_database_reset` performs.
        let stashed = read_tree_raw(&pool, "node_identity").unwrap();
        assert_eq!(stashed.len(), 2);

        let (dest_pool, _dest_tmp) = temp_pool();
        write_tree_raw(&dest_pool, "node_identity", &stashed).unwrap();

        let restored = read_tree_raw(&dest_pool, "node_identity").unwrap();
        assert_eq!(stashed, restored, "raw bytes must round-trip exactly");
    }

    #[test]
    fn read_returns_empty_for_missing_tree() {
        let (pool, _tmp) = temp_pool();
        // Tree never created — should return empty, not error. This
        // matches the fresh-install case where `node_identity` doesn't
        // exist yet at the moment of reset.
        let entries = read_tree_raw(&pool, "node_identity").unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn write_empty_entries_is_a_noop() {
        let (pool, _tmp) = temp_pool();
        write_tree_raw(&pool, "org_memberships", &Vec::new()).unwrap();
        let entries = read_tree_raw(&pool, "org_memberships").unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn preserved_trees_constant_includes_identity_and_orgs() {
        // Regression guard: the preserved-tree list is the entire
        // contract this reset relies on. If a refactor accidentally
        // drops one, the symptom is silent identity loss on next
        // reset — a lot worse than a failing test.
        assert!(
            RESET_PRESERVED_TREES.contains(&"node_identity"),
            "node_identity must survive reset (E2E key derives from it)"
        );
        assert!(
            RESET_PRESERVED_TREES.contains(&"org_memberships"),
            "org_memberships must survive reset (org E2E secrets are not recoverable)"
        );
    }
}
