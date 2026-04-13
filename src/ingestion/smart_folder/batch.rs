//! Batch coordinator internals for smart folder ingestion.
//!
//! Contains the concurrent task coordinator that processes files in parallel,
//! respecting spend limits and cancellation.

use crate::ingestion::batch_controller::{BatchControllerMap, BatchStatus, PendingFile};
use crate::ingestion::ingestion_service::IngestionService;
use crate::ingestion::progress::ProgressService;
use crate::ingestion::routes_helpers::process_single_file_via_smart_folder;
use crate::ingestion::ProgressTracker;
use actix_web::web;
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use std::sync::Arc;
use tokio::sync::Notify;

/// Result of trying to pop the next file from the batch controller.
enum PopResult {
    /// A file was successfully popped and is ready for processing.
    File(PendingFile),
    /// The spend limit was hit; the batch is now paused.
    SpendLimitHit(Arc<Notify>),
    /// All pending files have been popped (queue empty).
    Done,
    /// The batch was cancelled.
    Cancelled,
}

/// Lock the controller briefly, check cancellation/empty/spend, pop a file.
async fn try_pop_file(map: &BatchControllerMap, batch_id: &str) -> PopResult {
    let map_guard = map.lock().await;
    let ctrl_arc = match map_guard.get(batch_id) {
        Some(c) => c.clone(),
        None => return PopResult::Cancelled,
    };
    let mut ctrl = ctrl_arc.lock().await;

    if ctrl.status == BatchStatus::Cancelled {
        return PopResult::Cancelled;
    }

    let next_file = match ctrl.pending_files.first() {
        Some(f) => f.clone(),
        None => return PopResult::Done,
    };

    if !ctrl.can_proceed(next_file.estimated_cost) {
        ctrl.pause();
        return PopResult::SpendLimitHit(ctrl.resume_notifier());
    }

    let file = ctrl.pop_next_file().expect("pending_files was non-empty");
    let name = file
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();
    ctrl.add_in_flight(name, file.progress_id.clone());
    PopResult::File(file)
}

/// Record the result of processing a single file back into the controller.
async fn record_file_result(
    map: &BatchControllerMap,
    batch_id: &str,
    file: &PendingFile,
    result: &Result<(), String>,
    estimated_cost: f64,
    progress_service: &ProgressService,
) {
    let map_guard = map.lock().await;
    if let Some(ctrl_arc) = map_guard.get(batch_id) {
        let mut ctrl = ctrl_arc.lock().await;
        match result {
            Ok(()) => ctrl.record_completed(&file.progress_id, estimated_cost),
            Err(e) => {
                log_feature!(
                    LogFeature::Ingestion,
                    error,
                    "Batch {}: file {} failed: {}",
                    batch_id,
                    file.path.display(),
                    e
                );
                progress_service
                    .fail_progress(&file.progress_id, format!("Processing failed: {}", e))
                    .await;
                ctrl.record_failed(
                    &file.progress_id,
                    file.path.display().to_string(),
                    e.clone(),
                );
            }
        }
    }
}

/// Wait for the resume notification, then check for cancellation.
/// Returns `true` if the batch was cancelled while waiting.
async fn wait_for_resume(notifier: Arc<Notify>, map: &BatchControllerMap, batch_id: &str) -> bool {
    notifier.notified().await;
    let map_guard = map.lock().await;
    if let Some(ctrl_arc) = map_guard.get(batch_id) {
        let ctrl = ctrl_arc.lock().await;
        ctrl.status == BatchStatus::Cancelled
    } else {
        true
    }
}

/// Spawn a concurrent coordinator task that processes up to `max_concurrent`
/// files in parallel, checking the spend limit before each file and pausing
/// when the limit is hit.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_batch_coordinator(
    batch_id: String,
    batch_controller_map: web::Data<BatchControllerMap>,
    progress_tracker: &ProgressTracker,
    node_arc: &Arc<crate::fold_node::FoldNode>,
    user_id: &str,
    auto_execute: bool,
    ingestion_service: Arc<IngestionService>,
    upload_storage: fold_db::storage::UploadStorage,
    encryption_key: [u8; 32],
    force_reingest: bool,
    max_concurrent: usize,
    org_hash: Option<String>,
) {
    let progress_tracker = progress_tracker.clone();
    let node_arc = node_arc.clone();
    let user_id = user_id.to_string();
    let map = batch_controller_map.get_ref().clone();

    tokio::spawn(async move {
        let user_id_inner = user_id.clone();
        fold_db::logging::core::run_with_user(&user_id, async move {
            let batch_user_id =
                fold_db::logging::core::get_current_user_id().unwrap_or(user_id_inner);
            let mut join_set = tokio::task::JoinSet::new();
            let mut all_popped = false;

            loop {
                // Fill up to max_concurrent workers
                while !all_popped && join_set.len() < max_concurrent {
                    match try_pop_file(&map, &batch_id).await {
                        PopResult::File(file) => {
                            let node_arc = node_arc.clone();
                            let service = ingestion_service.clone();
                            let storage = upload_storage.clone();
                            let tracker = progress_tracker.clone();
                            let enc_key = encryption_key;
                            let task_uid = batch_user_id.clone();
                            let task_org_hash = org_hash.clone();
                            join_set.spawn(async move {
                                fold_db::logging::core::run_with_user(&task_uid, async move {
                                    let progress_service = ProgressService::new(tracker);
                                    let result = process_single_file_via_smart_folder(
                                        &file.path,
                                        &file.progress_id,
                                        &progress_service,
                                        &node_arc,
                                        auto_execute,
                                        &service,
                                        &storage,
                                        &enc_key,
                                        force_reingest,
                                        task_org_hash.as_deref(),
                                    )
                                    .await;
                                    (file, result)
                                })
                                .await
                            });
                        }
                        PopResult::SpendLimitHit(notifier) => {
                            // If tasks are still in flight, let them finish
                            // before blocking on resume.
                            if join_set.is_empty() {
                                let cancelled = wait_for_resume(notifier, &map, &batch_id).await;
                                if cancelled {
                                    all_popped = true;
                                }
                            }
                            // Either way, stop filling for now — the loop will
                            // drain completions and try filling again.
                            break;
                        }
                        PopResult::Done | PopResult::Cancelled => {
                            all_popped = true;
                            break;
                        }
                    }
                }

                if join_set.is_empty() {
                    break;
                }

                // Wait for the next completion, record result, loop to fill more slots
                if let Some(Ok((file, result))) = join_set.join_next().await {
                    let estimated_cost = file.estimated_cost;
                    let progress_service = ProgressService::new(progress_tracker.clone());
                    record_file_result(
                        &map,
                        &batch_id,
                        &file,
                        &result,
                        estimated_cost,
                        &progress_service,
                    )
                    .await;
                }
            }

            // Mark completed if not already cancelled/failed
            let batch_completed = {
                let map_guard = map.lock().await;
                if let Some(ctrl_arc) = map_guard.get(&batch_id) {
                    let mut ctrl = ctrl_arc.lock().await;
                    if ctrl.status == BatchStatus::Running {
                        ctrl.status = BatchStatus::Completed;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            };

            // Run interest detection after successful batch completion
            if batch_completed {
                let node_for_interests = node_arc.clone();
                tokio::spawn(async move {
                    if let Err(e) =
                        crate::discovery::interests::run_interest_detection(&node_for_interests)
                            .await
                    {
                        log_feature!(
                            LogFeature::Ingestion,
                            warn,
                            "Interest detection after batch completion failed: {}",
                            e
                        );
                    }
                });
            }

            // Clean up the controller after a short delay so final status
            // polls can still read it before it's removed.
            let map_cleanup = map.clone();
            let batch_id_cleanup = batch_id.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(300)).await;
                let mut map_guard = map_cleanup.lock().await;
                map_guard.remove(&batch_id_cleanup);
            });
        })
        .await
    });
}
