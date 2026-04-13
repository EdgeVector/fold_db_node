//! Background scheduler that re-imports enabled Apple sources on a timer.
//!
//! The scheduler runs as a `tokio::spawn`-ed task. It checks once per minute
//! whether the next sync time has been reached and, if so, triggers imports
//! for all enabled sources. Content-hash dedup in the ingestion pipeline
//! ensures unchanged items are skipped.

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

/// Execute all enabled Apple-source imports once.
///
/// Framework-agnostic — the HTTP layer's scheduler loop calls this after
/// resolving `node_arc` / `service` / `tracker` for the current user.
pub async fn run_sync(
    sources: &super::sync_config::EnabledSources,
    photos_limit: usize,
    user_id: &str,
    node_arc: Arc<tokio::sync::RwLock<crate::fold_node::FoldNode>>,
    service: Arc<IngestionService>,
    tracker: ProgressTracker,
) {
    if sources.notes {
        log_feature!(
            LogFeature::Ingestion,
            info,
            "Apple auto-sync: importing notes"
        );
        sync_notes(user_id, node_arc.clone(), service.clone(), tracker.clone()).await;
    }

    if sources.reminders {
        log_feature!(
            LogFeature::Ingestion,
            info,
            "Apple auto-sync: importing reminders"
        );
        sync_reminders(user_id, node_arc.clone(), service.clone(), tracker.clone()).await;
    }

    if sources.photos {
        log_feature!(
            LogFeature::Ingestion,
            info,
            "Apple auto-sync: importing photos (limit: {})",
            photos_limit
        );
        sync_photos(
            user_id,
            node_arc.clone(),
            service.clone(),
            tracker.clone(),
            photos_limit,
        )
        .await;
    }
}

// ── Per-source import helpers (macOS) ────────────────────────────────

#[cfg(target_os = "macos")]
async fn sync_notes(
    user_id: &str,
    node_arc: Arc<tokio::sync::RwLock<crate::fold_node::FoldNode>>,
    service: Arc<IngestionService>,
    tracker: ProgressTracker,
) {
    use super::notes;
    use crate::ingestion::IngestionRequest;

    let notes = match tokio::task::spawn_blocking(|| notes::extract(None)).await {
        Ok(Ok(n)) => n,
        Ok(Err(e)) => {
            log_feature!(
                LogFeature::Ingestion,
                warn,
                "Auto-sync notes extract failed: {}",
                e
            );
            return;
        }
        Err(e) => {
            log_feature!(
                LogFeature::Ingestion,
                warn,
                "Auto-sync notes task panicked: {}",
                e
            );
            return;
        }
    };

    if notes.is_empty() {
        return;
    }

    let records = notes::to_json_records(&notes);
    let node = node_arc.read().await;
    let uid = user_id.to_string();

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
            &uid,
            &tracker,
            &node,
            service.clone(),
        )
        .await
        {
            log_feature!(
                LogFeature::Ingestion,
                warn,
                "Auto-sync notes batch error: {}",
                e
            );
        }
    }
}

#[cfg(target_os = "macos")]
async fn sync_reminders(
    user_id: &str,
    node_arc: Arc<tokio::sync::RwLock<crate::fold_node::FoldNode>>,
    service: Arc<IngestionService>,
    tracker: ProgressTracker,
) {
    use super::reminders;
    use crate::ingestion::IngestionRequest;

    let rems = match tokio::task::spawn_blocking(|| reminders::extract(None)).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            log_feature!(
                LogFeature::Ingestion,
                warn,
                "Auto-sync reminders extract failed: {}",
                e
            );
            return;
        }
        Err(e) => {
            log_feature!(
                LogFeature::Ingestion,
                warn,
                "Auto-sync reminders task panicked: {}",
                e
            );
            return;
        }
    };

    if rems.is_empty() {
        return;
    }

    let records = reminders::to_json_records(&rems);
    let node = node_arc.read().await;

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
        crate::handlers::ingestion::process_json(request, user_id, &tracker, &node, service).await
    {
        log_feature!(
            LogFeature::Ingestion,
            warn,
            "Auto-sync reminders error: {}",
            e
        );
    }
}

#[cfg(target_os = "macos")]
async fn sync_photos(
    user_id: &str,
    node_arc: Arc<tokio::sync::RwLock<crate::fold_node::FoldNode>>,
    service: Arc<IngestionService>,
    tracker: ProgressTracker,
    limit: usize,
) {
    use super::photos;
    use crate::ingestion::IngestionRequest;

    let paths = match tokio::task::spawn_blocking(move || photos::export(None, limit)).await {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            log_feature!(
                LogFeature::Ingestion,
                warn,
                "Auto-sync photos export failed: {}",
                e
            );
            return;
        }
        Err(e) => {
            log_feature!(
                LogFeature::Ingestion,
                warn,
                "Auto-sync photos task panicked: {}",
                e
            );
            return;
        }
    };

    if paths.is_empty() {
        return;
    }

    let node = node_arc.read().await;

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

                // Read image bytes for face detection before ingestion
                let image_bytes = std::fs::read(&file_path).ok();

                let request = IngestionRequest {
                    data: json_value,
                    auto_execute: true,
                    pub_key: "default".to_string(),
                    source_file_name: Some(file_name.to_string()),
                    progress_id: None,
                    file_hash: None,
                    source_folder: None,
                    image_descriptive_name: descriptive_name,
                    org_hash: None,
                    image_bytes,
                };

                if let Err(e) = crate::handlers::ingestion::process_json(
                    request,
                    user_id,
                    &tracker,
                    &node,
                    service.clone(),
                )
                .await
                {
                    log_feature!(
                        LogFeature::Ingestion,
                        warn,
                        "Auto-sync photo {} error: {}",
                        file_name,
                        e
                    );
                }
            }
            Err(e) => {
                log_feature!(
                    LogFeature::Ingestion,
                    warn,
                    "Auto-sync photo convert error {}: {}",
                    path.display(),
                    e
                );
            }
        }
    }
}

// ── Non-macOS stubs ──────────────────────────────────────────────────

#[cfg(not(target_os = "macos"))]
async fn sync_notes(
    _user_id: &str,
    _node_arc: Arc<tokio::sync::RwLock<crate::fold_node::FoldNode>>,
    _service: Arc<IngestionService>,
    _tracker: ProgressTracker,
) {
    log_feature!(
        LogFeature::Ingestion,
        warn,
        "Auto-sync notes: not available on this platform"
    );
}

#[cfg(not(target_os = "macos"))]
async fn sync_reminders(
    _user_id: &str,
    _node_arc: Arc<tokio::sync::RwLock<crate::fold_node::FoldNode>>,
    _service: Arc<IngestionService>,
    _tracker: ProgressTracker,
) {
    log_feature!(
        LogFeature::Ingestion,
        warn,
        "Auto-sync reminders: not available on this platform"
    );
}

#[cfg(not(target_os = "macos"))]
async fn sync_photos(
    _user_id: &str,
    _node_arc: Arc<tokio::sync::RwLock<crate::fold_node::FoldNode>>,
    _service: Arc<IngestionService>,
    _tracker: ProgressTracker,
    _limit: usize,
) {
    log_feature!(
        LogFeature::Ingestion,
        warn,
        "Auto-sync photos: not available on this platform"
    );
}
