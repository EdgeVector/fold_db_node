//! Background scheduler that re-imports enabled Apple sources on a timer.
//!
//! The scheduler runs as a `tokio::spawn`-ed task. It checks once per minute
//! whether the next sync time has been reached and, if so, triggers imports
//! for all enabled sources. Content-hash dedup in the ingestion pipeline
//! ensures unchanged items are skipped.

use std::sync::Arc;
use tokio::sync::RwLock;

use chrono::Utc;
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use fold_db::progress::ProgressTracker;

use super::sync_config::AppleSyncConfig;
use crate::ingestion::ingestion_service::IngestionService;
use crate::ingestion::routes_helpers::IngestionServiceState;
use crate::server::http_server::AppState;

/// Shared handle to the sync config so routes and the scheduler can both read/write it.
pub type SyncConfigState = Arc<RwLock<AppleSyncConfig>>;

/// Create the shared sync config state, loading from disk.
pub fn create_sync_config_state() -> SyncConfigState {
    Arc::new(RwLock::new(AppleSyncConfig::load()))
}

/// Spawn the background sync scheduler.
///
/// The task wakes every 60 seconds, checks if `next_sync` has passed, and if
/// so runs imports for each enabled source. After completion it updates
/// `last_sync` / `next_sync` and persists the config.
pub fn spawn_sync_scheduler(
    sync_config: SyncConfigState,
    app_state: actix_web::web::Data<AppState>,
    ingestion_service: actix_web::web::Data<IngestionServiceState>,
    progress_tracker: actix_web::web::Data<ProgressTracker>,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;

            let should_sync = {
                let cfg = sync_config.read().await;
                cfg.enabled && cfg.next_sync.is_some_and(|next| Utc::now() >= next)
            };

            if !should_sync {
                continue;
            }

            log_feature!(
                LogFeature::Ingestion,
                info,
                "Apple auto-sync: starting scheduled import"
            );

            let (sources, photos_limit) = {
                let cfg = sync_config.read().await;
                (cfg.sources.clone(), cfg.photos_limit)
            };

            run_sync(
                &sources,
                photos_limit,
                &app_state,
                &ingestion_service,
                &progress_tracker,
            )
            .await;

            // Mark complete and persist
            {
                let mut cfg = sync_config.write().await;
                cfg.mark_sync_complete(Utc::now());
                if let Err(e) = cfg.save() {
                    log_feature!(
                        LogFeature::Ingestion,
                        error,
                        "Apple auto-sync: failed to persist config: {}",
                        e
                    );
                }
            }

            log_feature!(
                LogFeature::Ingestion,
                info,
                "Apple auto-sync: scheduled import complete"
            );
        }
    });
}

/// Execute the actual imports for enabled sources.
async fn run_sync(
    sources: &super::sync_config::EnabledSources,
    photos_limit: usize,
    app_state: &actix_web::web::Data<AppState>,
    ingestion_service: &actix_web::web::Data<IngestionServiceState>,
    progress_tracker: &actix_web::web::Data<ProgressTracker>,
) {
    use crate::server::routes::common::require_node;

    let (user_id, node_arc) = match require_node(app_state).await {
        Ok(ctx) => ctx,
        Err(_) => {
            log_feature!(
                LogFeature::Ingestion,
                warn,
                "Apple auto-sync: no active node, skipping"
            );
            return;
        }
    };

    let service: Arc<IngestionService> = match ingestion_service.read().await.clone() {
        Some(s) => s,
        None => {
            log_feature!(
                LogFeature::Ingestion,
                warn,
                "Apple auto-sync: ingestion service not available, skipping"
            );
            return;
        }
    };

    let tracker = progress_tracker.get_ref().clone();

    if sources.notes {
        log_feature!(
            LogFeature::Ingestion,
            info,
            "Apple auto-sync: importing notes"
        );
        sync_notes(&user_id, node_arc.clone(), service.clone(), tracker.clone()).await;
    }

    if sources.reminders {
        log_feature!(
            LogFeature::Ingestion,
            info,
            "Apple auto-sync: importing reminders"
        );
        sync_reminders(&user_id, node_arc.clone(), service.clone(), tracker.clone()).await;
    }

    if sources.photos {
        log_feature!(
            LogFeature::Ingestion,
            info,
            "Apple auto-sync: importing photos (limit: {})",
            photos_limit
        );
        sync_photos(
            &user_id,
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
