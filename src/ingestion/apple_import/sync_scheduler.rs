//! Background scheduler that re-imports enabled Apple sources on a timer.
//!
//! The scheduler runs as a `tokio::spawn`-ed task. It checks once per minute
//! whether the next sync time has been reached and, if so, triggers imports
//! for all enabled sources. Content-hash dedup in the ingestion pipeline
//! ensures unchanged items are skipped.
//!
//! ## Reliability invariants
//!
//! - Firing decision is wall-clock based (`Utc::now() >= next_sync`), not
//!   tokio-timer based, so macOS sleep/wake cycles are tolerated — on wake,
//!   the next tick sees `next_sync` in the past and fires immediately.
//! - Every failure is propagated: per-source helpers return `Result<(),
//!   String>`, the scheduler aggregates and records them in
//!   `SyncConfig.last_error` so the UI and logs reveal the problem.

use std::sync::Arc;
use tokio::sync::RwLock;

use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use fold_db::progress::ProgressTracker;

use super::sync_config::AppleSyncConfig;
use crate::ingestion::ingestion_service::IngestionService;

/// Shared handle to the sync config so routes and the scheduler can both read/write it.
pub type SyncConfigState = Arc<RwLock<AppleSyncConfig>>;

/// Create the shared sync config state, loading from disk.
pub fn create_sync_config_state() -> SyncConfigState {
    Arc::new(RwLock::new(AppleSyncConfig::load()))
}

/// Execute all enabled Apple-source imports once. Returns the list of
/// per-source error messages collected during the run — empty on success.
///
/// The scheduler loop in `routes::apple_import::spawn_sync_scheduler`
/// translates an empty result into `mark_sync_complete` and a non-empty
/// result into `mark_sync_error`, ensuring every extractor failure
/// surfaces in `last_error` (and therefore in the UI + structured logs).
pub async fn run_sync(
    sources: &super::sync_config::EnabledSources,
    photos_limit: usize,
    user_id: &str,
    node_arc: Arc<crate::fold_node::FoldNode>,
    service: Arc<IngestionService>,
    tracker: ProgressTracker,
    upload_storage: fold_db::storage::UploadStorage,
) -> Vec<String> {
    let mut errors: Vec<String> = Vec::new();

    if sources.notes {
        log_feature!(
            LogFeature::Ingestion,
            info,
            "Apple auto-sync: importing notes"
        );
        if let Err(e) =
            sync_notes(user_id, node_arc.clone(), service.clone(), tracker.clone()).await
        {
            errors.push(format!("notes: {e}"));
        }
    }

    if sources.reminders {
        log_feature!(
            LogFeature::Ingestion,
            info,
            "Apple auto-sync: importing reminders"
        );
        if let Err(e) =
            sync_reminders(user_id, node_arc.clone(), service.clone(), tracker.clone()).await
        {
            errors.push(format!("reminders: {e}"));
        }
    }

    if sources.photos {
        log_feature!(
            LogFeature::Ingestion,
            info,
            "Apple auto-sync: importing photos (limit: {})",
            photos_limit
        );
        if let Err(e) = sync_photos(
            user_id,
            node_arc.clone(),
            service.clone(),
            tracker.clone(),
            photos_limit,
            upload_storage.clone(),
        )
        .await
        {
            errors.push(format!("photos: {e}"));
        }
    }

    if sources.calendar {
        log_feature!(
            LogFeature::Ingestion,
            info,
            "Apple auto-sync: importing calendar"
        );
        if let Err(e) =
            sync_calendar(user_id, node_arc.clone(), service.clone(), tracker.clone()).await
        {
            errors.push(format!("calendar: {e}"));
        }
    }

    if sources.contacts {
        log_feature!(
            LogFeature::Ingestion,
            info,
            "Apple auto-sync: importing contacts"
        );
        if let Err(e) =
            sync_contacts(user_id, node_arc.clone(), service.clone(), tracker.clone()).await
        {
            errors.push(format!("contacts: {e}"));
        }
    }

    errors
}

// ── Per-source import helpers (macOS) ────────────────────────────────

#[cfg(target_os = "macos")]
async fn sync_notes(
    user_id: &str,
    node_arc: Arc<crate::fold_node::FoldNode>,
    service: Arc<IngestionService>,
    tracker: ProgressTracker,
) -> Result<(), String> {
    use super::notes;
    use crate::ingestion::IngestionRequest;

    let notes = match tokio::task::spawn_blocking(|| notes::extract(None)).await {
        Ok(Ok(n)) => n,
        Ok(Err(e)) => {
            log_feature!(
                LogFeature::Ingestion,
                error,
                "Auto-sync notes extract failed: {}",
                e
            );
            return Err(format!("extract failed: {e}"));
        }
        Err(e) => {
            log_feature!(
                LogFeature::Ingestion,
                error,
                "Auto-sync notes task panicked: {}",
                e
            );
            return Err(format!("task panicked: {e}"));
        }
    };

    if notes.is_empty() {
        return Ok(());
    }

    let records = notes::to_json_records(&notes);
    let node = node_arc.as_ref();
    let uid = user_id.to_string();
    let total_batches = records.chunks(10).count();

    let mut batch_errors: Vec<String> = Vec::new();
    for chunk in records.chunks(10) {
        let request = IngestionRequest {
            data: serde_json::Value::Array(chunk.to_vec()),
            auto_execute: true,
            pub_key: "default".to_string(),
            source_file_name: None,
            progress_id: None,
            file_hash: None,
            source_folder: None,
            image_descriptive_name: None,
            org_hash: None,
            image_bytes: None,
        };

        if let Err(e) =
            crate::handlers::ingestion::process_json(request, &uid, &tracker, node, service.clone())
                .await
        {
            log_feature!(
                LogFeature::Ingestion,
                error,
                "Auto-sync notes batch error: {}",
                e
            );
            batch_errors.push(e.to_string());
        }
    }

    if batch_errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{}/{} batches failed: {}",
            batch_errors.len(),
            total_batches,
            batch_errors.join("; ")
        ))
    }
}

#[cfg(target_os = "macos")]
async fn sync_reminders(
    user_id: &str,
    node_arc: Arc<crate::fold_node::FoldNode>,
    service: Arc<IngestionService>,
    tracker: ProgressTracker,
) -> Result<(), String> {
    use super::reminders;
    use crate::ingestion::IngestionRequest;

    let rems = match tokio::task::spawn_blocking(|| reminders::extract(None)).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            log_feature!(
                LogFeature::Ingestion,
                error,
                "Auto-sync reminders extract failed: {}",
                e
            );
            return Err(format!("extract failed: {e}"));
        }
        Err(e) => {
            log_feature!(
                LogFeature::Ingestion,
                error,
                "Auto-sync reminders task panicked: {}",
                e
            );
            return Err(format!("task panicked: {e}"));
        }
    };

    if rems.is_empty() {
        return Ok(());
    }

    let records = reminders::to_json_records(&rems);
    let node = node_arc.as_ref();

    let request = IngestionRequest {
        data: serde_json::Value::Array(records),
        auto_execute: true,
        pub_key: "default".to_string(),
        source_file_name: None,
        progress_id: None,
        file_hash: None,
        source_folder: None,
        image_descriptive_name: None,
        org_hash: None,
        image_bytes: None,
    };

    if let Err(e) =
        crate::handlers::ingestion::process_json(request, user_id, &tracker, node, service).await
    {
        log_feature!(
            LogFeature::Ingestion,
            error,
            "Auto-sync reminders error: {}",
            e
        );
        return Err(e.to_string());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
async fn sync_photos(
    user_id: &str,
    node_arc: Arc<crate::fold_node::FoldNode>,
    service: Arc<IngestionService>,
    tracker: ProgressTracker,
    limit: usize,
    upload_storage: fold_db::storage::UploadStorage,
) -> Result<(), String> {
    use super::photos;
    use crate::ingestion::helpers::store_file_content_addressed;
    use crate::ingestion::IngestionRequest;

    let paths = match tokio::task::spawn_blocking(move || photos::export(None, limit)).await {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            log_feature!(
                LogFeature::Ingestion,
                error,
                "Auto-sync photos export failed: {}",
                e
            );
            return Err(format!("export failed: {e}"));
        }
        Err(e) => {
            log_feature!(
                LogFeature::Ingestion,
                error,
                "Auto-sync photos task panicked: {}",
                e
            );
            return Err(format!("task panicked: {e}"));
        }
    };

    if paths.is_empty() {
        return Ok(());
    }

    let node = node_arc.as_ref();
    let encryption_key = node.get_encryption_key();
    let mut per_photo_errors: Vec<String> = Vec::new();

    for path in &paths {
        let file_path = path.to_path_buf();
        match crate::ingestion::file_handling::json_processor::convert_file_to_json(&file_path)
            .await
        {
            Ok(mut json_value) => {
                let file_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("photo.jpg");
                let descriptive_name =
                    crate::ingestion::file_handling::json_processor::enrich_image_json(
                        &mut json_value,
                        &file_path,
                        Some(file_name),
                    );

                if json_value
                    .get("visibility")
                    .and_then(|v| v.as_str())
                    .is_none()
                {
                    if let Ok(visibility) =
                        crate::ingestion::file_handling::json_processor::classify_visibility(
                            &json_value,
                            &service,
                        )
                        .await
                    {
                        if let serde_json::Value::Object(ref mut map) = json_value {
                            map.insert(
                                "visibility".to_string(),
                                serde_json::Value::String(visibility),
                            );
                        }
                    }
                }

                // Read raw bytes once. Used for both face detection and the
                // content-addressed store (which the data browser fetches from
                // via `/api/file/<hash>` to render inline previews).
                let raw_bytes = match std::fs::read(&file_path) {
                    Ok(b) => b,
                    Err(e) => {
                        log_feature!(
                            LogFeature::Ingestion,
                            warn,
                            "Auto-sync photo {} read failed: {}",
                            file_name,
                            e
                        );
                        per_photo_errors.push(format!("{file_name}: read: {e}"));
                        continue;
                    }
                };

                let file_hash = match store_file_content_addressed(
                    &raw_bytes,
                    &upload_storage,
                    &encryption_key,
                )
                .await
                {
                    Ok(h) => Some(h),
                    Err(e) => {
                        log_feature!(
                            LogFeature::Ingestion,
                            warn,
                            "Auto-sync photo {} store failed, continuing without preview: {}",
                            file_name,
                            e
                        );
                        None
                    }
                };

                let request = IngestionRequest {
                    data: json_value,
                    auto_execute: true,
                    pub_key: "default".to_string(),
                    source_file_name: Some(file_name.to_string()),
                    progress_id: None,
                    file_hash,
                    source_folder: None,
                    image_descriptive_name: descriptive_name,
                    org_hash: None,
                    image_bytes: Some(raw_bytes),
                };

                if let Err(e) = crate::handlers::ingestion::process_json(
                    request,
                    user_id,
                    &tracker,
                    node,
                    service.clone(),
                )
                .await
                {
                    log_feature!(
                        LogFeature::Ingestion,
                        error,
                        "Auto-sync photo {} error: {}",
                        file_name,
                        e
                    );
                    per_photo_errors.push(format!("{file_name}: {e}"));
                }
            }
            Err(e) => {
                log_feature!(
                    LogFeature::Ingestion,
                    error,
                    "Auto-sync photo convert error {}: {}",
                    path.display(),
                    e
                );
                per_photo_errors.push(format!("{}: convert: {e}", path.display()));
            }
        }
    }

    if per_photo_errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{}/{} photos failed: {}",
            per_photo_errors.len(),
            paths.len(),
            per_photo_errors.join("; ")
        ))
    }
}

#[cfg(target_os = "macos")]
async fn sync_calendar(
    user_id: &str,
    node_arc: Arc<crate::fold_node::FoldNode>,
    service: Arc<IngestionService>,
    tracker: ProgressTracker,
) -> Result<(), String> {
    use super::calendar as cal;
    use crate::ingestion::IngestionRequest;

    let events = match tokio::task::spawn_blocking(|| cal::extract(None)).await {
        Ok(Ok(e)) => e,
        Ok(Err(e)) => {
            log_feature!(
                LogFeature::Ingestion,
                error,
                "Auto-sync calendar extract failed: {}",
                e
            );
            return Err(format!("extract failed: {e}"));
        }
        Err(e) => {
            log_feature!(
                LogFeature::Ingestion,
                error,
                "Auto-sync calendar task panicked: {}",
                e
            );
            return Err(format!("task panicked: {e}"));
        }
    };

    if events.is_empty() {
        return Ok(());
    }

    let records = cal::to_json_records(&events);
    let node = node_arc.as_ref();
    let total_batches = records.chunks(10).count();

    let mut batch_errors: Vec<String> = Vec::new();
    for chunk in records.chunks(10) {
        let request = IngestionRequest {
            data: serde_json::Value::Array(chunk.to_vec()),
            auto_execute: true,
            pub_key: "default".to_string(),
            source_file_name: None,
            progress_id: None,
            file_hash: None,
            source_folder: None,
            image_descriptive_name: None,
            org_hash: None,
            image_bytes: None,
        };

        if let Err(e) = crate::handlers::ingestion::process_json(
            request,
            user_id,
            &tracker,
            node,
            service.clone(),
        )
        .await
        {
            log_feature!(
                LogFeature::Ingestion,
                error,
                "Auto-sync calendar batch error: {}",
                e
            );
            batch_errors.push(e.to_string());
        }
    }

    if batch_errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{}/{} batches failed: {}",
            batch_errors.len(),
            total_batches,
            batch_errors.join("; ")
        ))
    }
}

#[cfg(target_os = "macos")]
async fn sync_contacts(
    user_id: &str,
    node_arc: Arc<crate::fold_node::FoldNode>,
    service: Arc<IngestionService>,
    tracker: ProgressTracker,
) -> Result<(), String> {
    use super::contacts;
    use crate::ingestion::IngestionRequest;

    let contacts_vec = match tokio::task::spawn_blocking(contacts::extract).await {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            log_feature!(
                LogFeature::Ingestion,
                error,
                "Auto-sync contacts extract failed: {}",
                e
            );
            return Err(format!("extract failed: {e}"));
        }
        Err(e) => {
            log_feature!(
                LogFeature::Ingestion,
                error,
                "Auto-sync contacts task panicked: {}",
                e
            );
            return Err(format!("task panicked: {e}"));
        }
    };

    if contacts_vec.is_empty() {
        return Ok(());
    }

    let records = contacts::to_json_records(&contacts_vec);
    let node = node_arc.as_ref();
    let total_batches = records.chunks(10).count();

    let mut batch_errors: Vec<String> = Vec::new();
    for chunk in records.chunks(10) {
        let request = IngestionRequest {
            data: serde_json::Value::Array(chunk.to_vec()),
            auto_execute: true,
            pub_key: "default".to_string(),
            source_file_name: None,
            progress_id: None,
            file_hash: None,
            source_folder: None,
            image_descriptive_name: None,
            org_hash: None,
            image_bytes: None,
        };

        if let Err(e) = crate::handlers::ingestion::process_json(
            request,
            user_id,
            &tracker,
            node,
            service.clone(),
        )
        .await
        {
            log_feature!(
                LogFeature::Ingestion,
                error,
                "Auto-sync contacts batch error: {}",
                e
            );
            batch_errors.push(e.to_string());
        }
    }

    if batch_errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{}/{} batches failed: {}",
            batch_errors.len(),
            total_batches,
            batch_errors.join("; ")
        ))
    }
}

// ── Non-macOS stubs ──────────────────────────────────────────────────

#[cfg(not(target_os = "macos"))]
async fn sync_notes(
    _user_id: &str,
    _node_arc: Arc<crate::fold_node::FoldNode>,
    _service: Arc<IngestionService>,
    _tracker: ProgressTracker,
) -> Result<(), String> {
    log_feature!(
        LogFeature::Ingestion,
        warn,
        "Auto-sync notes: not available on this platform"
    );
    Ok(())
}

#[cfg(not(target_os = "macos"))]
async fn sync_reminders(
    _user_id: &str,
    _node_arc: Arc<crate::fold_node::FoldNode>,
    _service: Arc<IngestionService>,
    _tracker: ProgressTracker,
) -> Result<(), String> {
    log_feature!(
        LogFeature::Ingestion,
        warn,
        "Auto-sync reminders: not available on this platform"
    );
    Ok(())
}

#[cfg(not(target_os = "macos"))]
async fn sync_photos(
    _user_id: &str,
    _node_arc: Arc<crate::fold_node::FoldNode>,
    _service: Arc<IngestionService>,
    _tracker: ProgressTracker,
    _limit: usize,
    _upload_storage: fold_db::storage::UploadStorage,
) -> Result<(), String> {
    log_feature!(
        LogFeature::Ingestion,
        warn,
        "Auto-sync photos: not available on this platform"
    );
    Ok(())
}

#[cfg(not(target_os = "macos"))]
async fn sync_calendar(
    _user_id: &str,
    _node_arc: Arc<crate::fold_node::FoldNode>,
    _service: Arc<IngestionService>,
    _tracker: ProgressTracker,
) -> Result<(), String> {
    log_feature!(
        LogFeature::Ingestion,
        warn,
        "Auto-sync calendar: not available on this platform"
    );
    Ok(())
}

#[cfg(not(target_os = "macos"))]
async fn sync_contacts(
    _user_id: &str,
    _node_arc: Arc<crate::fold_node::FoldNode>,
    _service: Arc<IngestionService>,
    _tracker: ProgressTracker,
) -> Result<(), String> {
    log_feature!(
        LogFeature::Ingestion,
        warn,
        "Auto-sync contacts: not available on this platform"
    );
    Ok(())
}
