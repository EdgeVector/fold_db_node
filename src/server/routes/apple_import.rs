//! HTTP route handlers for Apple data import (Notes, Reminders, Photos, Calendar).
//!
//! Each endpoint spawns a background task that extracts data via osascript,
//! then feeds it into the ingestion pipeline. Progress is tracked via the
//! standard `ProgressTracker` / `Job` infrastructure.

use actix_web::{web, HttpResponse, Responder};
use fold_db::progress::{Job, JobStatus, JobType, ProgressTracker};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::Instrument;

use crate::ingestion::apple_import;
use crate::ingestion::apple_import::sync_scheduler::SyncConfigState;
use crate::ingestion::ingestion_service::IngestionService;
use crate::ingestion::service_state::IngestionServiceState;
#[cfg(target_os = "macos")]
use crate::ingestion::IngestionRequest;
use crate::server::http_server::AppState;
use crate::server::routes::common::require_node;

/// GET /api/ingestion/apple-import/status
/// Returns whether Apple import is available (macOS only).
pub async fn apple_import_status() -> impl Responder {
    HttpResponse::Ok().json(json!({
        "available": apple_import::is_available(),
    }))
}

/// Context shared by every Apple import handler.
///
/// Constructed by [`init_apple_import_job`] after all preflight checks pass
/// and the initial job row has been written. Each handler destructures this
/// to access the per-user node, ingestion service, and progress bookkeeping.
struct AppleImportContext {
    user_id: String,
    node_arc: std::sync::Arc<crate::fold_node::FoldNode>,
    service: std::sync::Arc<IngestionService>,
    progress_id: String,
    tracker: ProgressTracker,
}

/// Run the preflight for an Apple import handler and record the initial job.
///
/// Replaces the ~25 lines of identical boilerplate at the top of every
/// `apple_import_*` handler: platform check, per-user node resolution,
/// ingestion service lookup, progress id + tracker setup, and the initial
/// `progress_percentage = 5` job save.
///
/// On failure, returns the appropriate `HttpResponse` for the caller to
/// propagate unchanged.
async fn init_apple_import_job(
    job_type: &str,
    initial_message: &str,
    state: &web::Data<AppState>,
    ingestion_service: &web::Data<IngestionServiceState>,
    progress_tracker: &web::Data<ProgressTracker>,
) -> Result<AppleImportContext, HttpResponse> {
    if !apple_import::is_available() {
        return Err(HttpResponse::BadRequest().json(json!({
            "success": false,
            "error": "Apple import is only available on macOS",
        })));
    }

    let (user_id, node_arc) = require_node(state).await?;
    let service = ingestion_service.read().await.clone().ok_or_else(|| {
        HttpResponse::ServiceUnavailable().json(json!({
            "success": false,
            "error": "Ingestion service not available",
        }))
    })?;

    let progress_id = uuid::Uuid::new_v4().to_string();
    let tracker = progress_tracker.get_ref().clone();

    let mut job = Job::new(progress_id.clone(), JobType::Other(job_type.into()));
    job = job.with_user(user_id.clone());
    job.message = initial_message.into();
    job.progress_percentage = 5;
    let _ = tracker.save(&job).await;

    Ok(AppleImportContext {
        user_id,
        node_arc,
        service,
        progress_id,
        tracker,
    })
}

/// Stamp a freshly-built terminal `Job` with `completed_at` and `updated_at`.
///
/// Apple import handlers build the terminal job via `Job::new(...)` + direct
/// status assignment, bypassing `Job::complete` / `Job::fail` — so without
/// this call the API surfaces `is_complete: true, completed_at: null`.
fn mark_terminal(job: &mut Job) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    job.updated_at = now;
    job.completed_at = Some(now);
}

/// Spawn `work` on the runtime under the caller's user context and return the
/// standard `202 Accepted { success, progress_id }` response that every Apple
/// import handler emits.
fn spawn_apple_import_task<F, Fut>(user_id: String, progress_id: String, work: F) -> HttpResponse
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let response_id = progress_id.clone();
    tokio::spawn(
        async move {
            fold_db::user_context::run_with_user(&user_id, async move {
                work().await;
            })
            .await;
        }
        .instrument(tracing::Span::current()),
    );

    HttpResponse::Accepted().json(json!({
        "success": true,
        "progress_id": response_id,
    }))
}

#[derive(Deserialize)]
pub struct AppleNotesRequest {
    pub folder: Option<String>,
}

/// POST /api/ingestion/apple-import/notes
pub async fn apple_import_notes(
    request: web::Json<AppleNotesRequest>,
    state: web::Data<AppState>,
    ingestion_service: web::Data<IngestionServiceState>,
    progress_tracker: web::Data<ProgressTracker>,
) -> impl Responder {
    let AppleImportContext {
        user_id,
        node_arc,
        service,
        progress_id,
        tracker,
    } = match init_apple_import_job(
        "apple-notes",
        "Extracting notes from Apple Notes...",
        &state,
        &ingestion_service,
        &progress_tracker,
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };

    let folder = request.folder.clone();
    let pid = progress_id.clone();
    spawn_apple_import_task(user_id, progress_id, move || async move {
        run_apple_notes_import(folder, pid, tracker, node_arc, service).await;
    })
}

#[cfg(target_os = "macos")]
async fn run_apple_notes_import(
    folder: Option<String>,
    progress_id: String,
    tracker: ProgressTracker,
    node_arc: std::sync::Arc<crate::fold_node::FoldNode>,
    service: std::sync::Arc<crate::ingestion::ingestion_service::IngestionService>,
) {
    use crate::ingestion::apple_import::notes;

    let notes_result = tokio::task::spawn_blocking(move || notes::extract(folder.as_deref())).await;

    let notes = match notes_result {
        Ok(Ok(n)) => n,
        Ok(Err(e)) => {
            let mut job = Job::new(progress_id.clone(), JobType::Other("apple-notes".into()));
            job.status = JobStatus::Failed;
            job.message = format!("Failed to extract notes: {}", e);
            mark_terminal(&mut job);
            let _ = tracker.save(&job).await;
            return;
        }
        Err(e) => {
            let mut job = Job::new(progress_id.clone(), JobType::Other("apple-notes".into()));
            job.status = JobStatus::Failed;
            job.message = format!("Extraction task panicked: {}", e);
            mark_terminal(&mut job);
            let _ = tracker.save(&job).await;
            return;
        }
    };

    if notes.is_empty() {
        let mut job = Job::new(progress_id.clone(), JobType::Other("apple-notes".into()));
        job.status = JobStatus::Completed;
        job.progress_percentage = 100;
        job.message = "No notes found".into();
        job.result = Some(json!({ "total": 0, "ingested": 0 }));
        mark_terminal(&mut job);
        let _ = tracker.save(&job).await;
        return;
    }

    let total = notes.len();
    let records = notes::to_json_records(&notes);

    let mut job = Job::new(progress_id.clone(), JobType::Other("apple-notes".into()));
    job.status = JobStatus::Running;
    job.progress_percentage = 30;
    job.message = format!("Extracted {} notes, ingesting...", total);
    let _ = tracker.save(&job).await;

    let batch_size = 10;
    let mut ingested = 0;
    let node = node_arc.as_ref();

    for (i, chunk) in records.chunks(batch_size).enumerate() {
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
            // Pin every batch to the canonical "Apple Notes" schema so
            // 132 notes don't fragment across 3+ schemas via LLM
            // non-determinism. See dogfood repro on 2026-05-09.
            forced_schema_descriptive_name: Some("Apple Notes".to_string()),
        };

        match crate::handlers::ingestion::process_json(
            request,
            &fold_db::user_context::get_current_user_id().unwrap_or_default(),
            &tracker,
            node,
            service.clone(),
        )
        .await
        {
            Ok(_) => ingested += chunk.len(),
            Err(e) => {
                tracing::warn!(
                target: "fold_node::ingestion",
                        "Apple Notes batch {} failed: {}",
                        i,
                        e
                    );
            }
        }

        let pct = 30 + ((i + 1) * 70 / total.div_ceil(batch_size)).min(70);
        let mut job = Job::new(progress_id.clone(), JobType::Other("apple-notes".into()));
        job.status = JobStatus::Running;
        job.progress_percentage = pct as u8;
        job.message = format!("Ingested {}/{} notes...", ingested, total);
        let _ = tracker.save(&job).await;
    }

    let mut job = Job::new(progress_id.clone(), JobType::Other("apple-notes".into()));
    job.status = JobStatus::Completed;
    job.progress_percentage = 100;
    job.message = format!("Imported {} notes", ingested);
    job.result = Some(json!({ "total": total, "ingested": ingested }));
    mark_terminal(&mut job);
    let _ = tracker.save(&job).await;
}

#[cfg(not(target_os = "macos"))]
async fn run_apple_notes_import(
    _folder: Option<String>,
    progress_id: String,
    tracker: ProgressTracker,
    _node_arc: std::sync::Arc<crate::fold_node::FoldNode>,
    _service: std::sync::Arc<crate::ingestion::ingestion_service::IngestionService>,
) {
    let mut job = Job::new(progress_id, JobType::Other("apple-notes".into()));
    job.status = JobStatus::Failed;
    job.message = "Apple import is only available on macOS".into();
    mark_terminal(&mut job);
    let _ = tracker.save(&job).await;
}

#[derive(Deserialize)]
pub struct AppleRemindersRequest {
    pub list: Option<String>,
}

/// POST /api/ingestion/apple-import/reminders
pub async fn apple_import_reminders(
    request: web::Json<AppleRemindersRequest>,
    state: web::Data<AppState>,
    ingestion_service: web::Data<IngestionServiceState>,
    progress_tracker: web::Data<ProgressTracker>,
) -> impl Responder {
    let AppleImportContext {
        user_id,
        node_arc,
        service,
        progress_id,
        tracker,
    } = match init_apple_import_job(
        "apple-reminders",
        "Extracting reminders...",
        &state,
        &ingestion_service,
        &progress_tracker,
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };

    let list = request.list.clone();
    let pid = progress_id.clone();
    spawn_apple_import_task(user_id, progress_id, move || async move {
        run_apple_reminders_import(list, pid, tracker, node_arc, service).await;
    })
}

#[cfg(target_os = "macos")]
async fn run_apple_reminders_import(
    list: Option<String>,
    progress_id: String,
    tracker: ProgressTracker,
    node_arc: std::sync::Arc<crate::fold_node::FoldNode>,
    service: std::sync::Arc<crate::ingestion::ingestion_service::IngestionService>,
) {
    use crate::ingestion::apple_import::reminders;

    let reminders_result =
        tokio::task::spawn_blocking(move || reminders::extract(list.as_deref())).await;

    let rems = match reminders_result {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            let mut job = Job::new(
                progress_id.clone(),
                JobType::Other("apple-reminders".into()),
            );
            job.status = JobStatus::Failed;
            job.message = format!("Failed to extract reminders: {}", e);
            mark_terminal(&mut job);
            let _ = tracker.save(&job).await;
            return;
        }
        Err(e) => {
            let mut job = Job::new(
                progress_id.clone(),
                JobType::Other("apple-reminders".into()),
            );
            job.status = JobStatus::Failed;
            job.message = format!("Extraction task panicked: {}", e);
            mark_terminal(&mut job);
            let _ = tracker.save(&job).await;
            return;
        }
    };

    if rems.is_empty() {
        let mut job = Job::new(
            progress_id.clone(),
            JobType::Other("apple-reminders".into()),
        );
        job.status = JobStatus::Completed;
        job.progress_percentage = 100;
        job.message = "No reminders found".into();
        job.result = Some(json!({ "total": 0, "ingested": 0 }));
        mark_terminal(&mut job);
        let _ = tracker.save(&job).await;
        return;
    }

    let total = rems.len();
    let records = reminders::to_json_records(&rems);

    let mut job = Job::new(
        progress_id.clone(),
        JobType::Other("apple-reminders".into()),
    );
    job.status = JobStatus::Running;
    job.progress_percentage = 40;
    job.message = format!("Extracted {} reminders, ingesting...", total);
    let _ = tracker.save(&job).await;

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
        // Pin reminders to a canonical schema; same fragmentation risk as
        // Apple Notes whenever the LLM is asked to classify N batches.
        forced_schema_descriptive_name: Some("Apple Reminders".to_string()),
    };

    let (ingested, ingest_error) = match crate::handlers::ingestion::process_json(
        request,
        &fold_db::user_context::get_current_user_id().unwrap_or_default(),
        &tracker,
        node,
        service,
    )
    .await
    {
        Ok(_) => (total, None),
        Err(e) => {
            tracing::warn!(
            target: "fold_node::ingestion",
                "Apple Reminders ingestion failed: {}",
                e
            );
            (0, Some(e.to_string()))
        }
    };

    let mut job = build_reminders_final_job(progress_id.clone(), total, ingested, ingest_error);
    mark_terminal(&mut job);
    let _ = tracker.save(&job).await;
}

/// Build the terminal job for an Apple Reminders import.
///
/// If the single-shot ingest call errored, the job is `Failed` with the error
/// surfaced in `message`. Previously both success and failure were marked
/// `Completed`, masking full-batch failures as a green checkmark with
/// `Imported 0 reminders` — indistinguishable from a genuinely empty list.
#[cfg(any(target_os = "macos", test))]
fn build_reminders_final_job(
    progress_id: String,
    total: usize,
    ingested: usize,
    ingest_error: Option<String>,
) -> Job {
    let mut job = Job::new(progress_id, JobType::Other("apple-reminders".into()));
    job.progress_percentage = 100;
    if let Some(err) = ingest_error {
        job.status = JobStatus::Failed;
        job.message = format!("Reminders ingestion failed: {}", err);
    } else {
        job.status = JobStatus::Completed;
        job.message = format!("Imported {} reminders", ingested);
    }
    job.result = Some(json!({ "total": total, "ingested": ingested }));
    job
}

#[cfg(not(target_os = "macos"))]
async fn run_apple_reminders_import(
    _list: Option<String>,
    progress_id: String,
    tracker: ProgressTracker,
    _node_arc: std::sync::Arc<crate::fold_node::FoldNode>,
    _service: std::sync::Arc<crate::ingestion::ingestion_service::IngestionService>,
) {
    let mut job = Job::new(progress_id, JobType::Other("apple-reminders".into()));
    job.status = JobStatus::Failed;
    job.message = "Apple import is only available on macOS".into();
    mark_terminal(&mut job);
    let _ = tracker.save(&job).await;
}

#[derive(Deserialize)]
pub struct ApplePhotosRequest {
    pub album: Option<String>,
    pub limit: Option<usize>,
}

/// POST /api/ingestion/apple-import/photos
// TODO: Apple Photos ingestion does not yet run face detection — face extraction in the generic ingestion path is a separate workstream that requires ONNX inline.
pub async fn apple_import_photos(
    request: web::Json<ApplePhotosRequest>,
    state: web::Data<AppState>,
    ingestion_service: web::Data<IngestionServiceState>,
    progress_tracker: web::Data<ProgressTracker>,
    upload_storage: web::Data<fold_db::storage::UploadStorage>,
) -> impl Responder {
    let AppleImportContext {
        user_id,
        node_arc,
        service,
        progress_id,
        tracker,
    } = match init_apple_import_job(
        "apple-photos",
        "Exporting photos from Apple Photos...",
        &state,
        &ingestion_service,
        &progress_tracker,
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };

    let album = request.album.clone();
    let limit = request.limit.unwrap_or(50);
    let pid = progress_id.clone();
    let upload_storage_clone = upload_storage.get_ref().clone();
    spawn_apple_import_task(user_id, progress_id, move || async move {
        run_apple_photos_import(
            album,
            limit,
            pid,
            tracker,
            node_arc,
            service,
            upload_storage_clone,
        )
        .await;
    })
}

#[cfg(target_os = "macos")]
async fn run_apple_photos_import(
    album: Option<String>,
    limit: usize,
    progress_id: String,
    tracker: ProgressTracker,
    node_arc: std::sync::Arc<crate::fold_node::FoldNode>,
    service: std::sync::Arc<crate::ingestion::ingestion_service::IngestionService>,
    upload_storage: fold_db::storage::UploadStorage,
) {
    use crate::ingestion::apple_import::photos;
    use crate::ingestion::helpers::store_file_content_addressed;

    let photos_result =
        tokio::task::spawn_blocking(move || photos::export(album.as_deref(), limit)).await;

    let paths = match photos_result {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            let mut job = Job::new(progress_id.clone(), JobType::Other("apple-photos".into()));
            job.status = JobStatus::Failed;
            job.message = format!("Failed to export photos: {}", e);
            mark_terminal(&mut job);
            let _ = tracker.save(&job).await;
            return;
        }
        Err(e) => {
            let mut job = Job::new(progress_id.clone(), JobType::Other("apple-photos".into()));
            job.status = JobStatus::Failed;
            job.message = format!("Export task panicked: {}", e);
            mark_terminal(&mut job);
            let _ = tracker.save(&job).await;
            return;
        }
    };

    if paths.is_empty() {
        let mut job = Job::new(progress_id.clone(), JobType::Other("apple-photos".into()));
        job.status = JobStatus::Completed;
        job.progress_percentage = 100;
        job.message = "No photos found".into();
        job.result = Some(json!({ "total": 0, "ingested": 0 }));
        mark_terminal(&mut job);
        let _ = tracker.save(&job).await;
        return;
    }

    let total = paths.len();
    let mut job = Job::new(progress_id.clone(), JobType::Other("apple-photos".into()));
    job.status = JobStatus::Running;
    job.progress_percentage = 30;
    job.message = format!("Exported {} photos, uploading...", total);
    let _ = tracker.save(&job).await;

    let node = node_arc.as_ref();
    let encryption_key = node.get_encryption_key();
    let mut ingested = 0;

    for (i, path) in paths.iter().enumerate() {
        let file_path = path.to_path_buf();
        match crate::ingestion::file_handling::json_processor::convert_file_to_json(
            &file_path,
            service.config(),
        )
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
                    match crate::ingestion::file_handling::json_processor::classify_visibility(
                        &json_value,
                        &service,
                    )
                    .await
                    {
                        Ok(visibility) => {
                            if let serde_json::Value::Object(ref mut map) = json_value {
                                map.insert(
                                    "visibility".to_string(),
                                    serde_json::Value::String(visibility),
                                );
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                            target: "fold_node::ingestion",
                                                "Visibility classification failed, skipping: {}",
                                                e
                                            );
                        }
                    }
                }

                let raw_bytes = match std::fs::read(&file_path) {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::warn!(
                        target: "fold_node::ingestion",
                                        "Failed to read photo {} for storage: {}",
                                        file_name,
                                        e
                                    );
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
                        tracing::warn!(
                        target: "fold_node::ingestion",
                                        "Failed to store photo {} content-addressed (preview unavailable): {}",
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
                    // Photos route through the existing image schema-override
                    // path; no canonical schema name to pin.
                    forced_schema_descriptive_name: None,
                };

                match crate::handlers::ingestion::process_json(
                    request,
                    &fold_db::user_context::get_current_user_id().unwrap_or_default(),
                    &tracker,
                    node,
                    service.clone(),
                )
                .await
                {
                    Ok(_) => ingested += 1,
                    Err(e) => {
                        tracing::warn!(
                        target: "fold_node::ingestion",
                                        "Failed to ingest photo {}: {}",
                                        file_name,
                                        e
                                    );
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                target: "fold_node::ingestion",
                        "Failed to convert photo {}: {}",
                        path.display(),
                        e
                    );
            }
        }

        let pct = 30 + ((i + 1) * 70 / total).min(70);
        let mut job = Job::new(progress_id.clone(), JobType::Other("apple-photos".into()));
        job.status = JobStatus::Running;
        job.progress_percentage = pct as u8;
        job.message = format!("Ingesting {}/{} photos...", i + 1, total);
        let _ = tracker.save(&job).await;
    }

    let mut job = Job::new(progress_id.clone(), JobType::Other("apple-photos".into()));
    job.status = JobStatus::Completed;
    job.progress_percentage = 100;
    job.message = format!("Imported {} photos", ingested);
    job.result = Some(json!({ "total": total, "ingested": ingested }));
    mark_terminal(&mut job);
    let _ = tracker.save(&job).await;
}

#[cfg(not(target_os = "macos"))]
async fn run_apple_photos_import(
    _album: Option<String>,
    _limit: usize,
    progress_id: String,
    tracker: ProgressTracker,
    _node_arc: std::sync::Arc<crate::fold_node::FoldNode>,
    _service: std::sync::Arc<crate::ingestion::ingestion_service::IngestionService>,
    _upload_storage: fold_db::storage::UploadStorage,
) {
    let mut job = Job::new(progress_id, JobType::Other("apple-photos".into()));
    job.status = JobStatus::Failed;
    job.message = "Apple import is only available on macOS".into();
    mark_terminal(&mut job);
    let _ = tracker.save(&job).await;
}

#[derive(Deserialize)]
pub struct AppleCalendarRequest {
    pub calendar: Option<String>,
}

/// POST /api/ingestion/apple-import/calendar
pub async fn apple_import_calendar(
    request: web::Json<AppleCalendarRequest>,
    state: web::Data<AppState>,
    ingestion_service: web::Data<IngestionServiceState>,
    progress_tracker: web::Data<ProgressTracker>,
) -> impl Responder {
    let AppleImportContext {
        user_id,
        node_arc,
        service,
        progress_id,
        tracker,
    } = match init_apple_import_job(
        "apple-calendar",
        "Extracting events from Apple Calendar...",
        &state,
        &ingestion_service,
        &progress_tracker,
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };

    let calendar = request.calendar.clone();
    let pid = progress_id.clone();
    spawn_apple_import_task(user_id, progress_id, move || async move {
        run_apple_calendar_import(calendar, pid, tracker, node_arc, service).await;
    })
}

#[cfg(target_os = "macos")]
async fn run_apple_calendar_import(
    calendar: Option<String>,
    progress_id: String,
    tracker: ProgressTracker,
    node_arc: std::sync::Arc<crate::fold_node::FoldNode>,
    service: std::sync::Arc<crate::ingestion::ingestion_service::IngestionService>,
) {
    use crate::ingestion::apple_import::calendar as cal;

    let events_result =
        tokio::task::spawn_blocking(move || cal::extract(calendar.as_deref())).await;

    let events = match events_result {
        Ok(Ok(e)) => e,
        Ok(Err(e)) => {
            let mut job = Job::new(progress_id.clone(), JobType::Other("apple-calendar".into()));
            job.status = JobStatus::Failed;
            job.message = format!("Failed to extract calendar events: {}", e);
            mark_terminal(&mut job);
            let _ = tracker.save(&job).await;
            return;
        }
        Err(e) => {
            let mut job = Job::new(progress_id.clone(), JobType::Other("apple-calendar".into()));
            job.status = JobStatus::Failed;
            job.message = format!("Extraction task panicked: {}", e);
            mark_terminal(&mut job);
            let _ = tracker.save(&job).await;
            return;
        }
    };

    if events.is_empty() {
        let mut job = Job::new(progress_id.clone(), JobType::Other("apple-calendar".into()));
        job.status = JobStatus::Completed;
        job.progress_percentage = 100;
        job.message = "No calendar events found".into();
        job.result = Some(json!({ "total": 0, "ingested": 0 }));
        mark_terminal(&mut job);
        let _ = tracker.save(&job).await;
        return;
    }

    let total = events.len();
    let records = cal::to_json_records(&events);

    let mut job = Job::new(progress_id.clone(), JobType::Other("apple-calendar".into()));
    job.status = JobStatus::Running;
    job.progress_percentage = 30;
    job.message = format!("Extracted {} events, ingesting...", total);
    let _ = tracker.save(&job).await;

    let batch_size = 10;
    let mut ingested = 0;
    let node = node_arc.as_ref();

    for (i, chunk) in records.chunks(batch_size).enumerate() {
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
            forced_schema_descriptive_name: Some("Apple Calendar".to_string()),
        };

        match crate::handlers::ingestion::process_json(
            request,
            &fold_db::user_context::get_current_user_id().unwrap_or_default(),
            &tracker,
            node,
            service.clone(),
        )
        .await
        {
            Ok(_) => ingested += chunk.len(),
            Err(e) => {
                tracing::warn!(
                target: "fold_node::ingestion",
                        "Apple Calendar batch {} failed: {}",
                        i,
                        e
                    );
            }
        }

        let pct = 30 + ((i + 1) * 70 / total.div_ceil(batch_size)).min(70);
        let mut job = Job::new(progress_id.clone(), JobType::Other("apple-calendar".into()));
        job.status = JobStatus::Running;
        job.progress_percentage = pct as u8;
        job.message = format!("Ingested {}/{} events...", ingested, total);
        let _ = tracker.save(&job).await;
    }

    let mut job = Job::new(progress_id.clone(), JobType::Other("apple-calendar".into()));
    job.status = JobStatus::Completed;
    job.progress_percentage = 100;
    job.message = format!("Imported {} calendar events", ingested);
    job.result = Some(json!({ "total": total, "ingested": ingested }));
    mark_terminal(&mut job);
    let _ = tracker.save(&job).await;
}

#[cfg(not(target_os = "macos"))]
async fn run_apple_calendar_import(
    _calendar: Option<String>,
    progress_id: String,
    tracker: ProgressTracker,
    _node_arc: std::sync::Arc<crate::fold_node::FoldNode>,
    _service: std::sync::Arc<crate::ingestion::ingestion_service::IngestionService>,
) {
    let mut job = Job::new(progress_id, JobType::Other("apple-calendar".into()));
    job.status = JobStatus::Failed;
    job.message = "Apple import is only available on macOS".into();
    mark_terminal(&mut job);
    let _ = tracker.save(&job).await;
}

#[derive(Deserialize)]
pub struct AppleContactsRequest {}

/// POST /api/ingestion/apple-import/contacts
pub async fn apple_import_contacts(
    _request: web::Json<AppleContactsRequest>,
    state: web::Data<AppState>,
    ingestion_service: web::Data<IngestionServiceState>,
    progress_tracker: web::Data<ProgressTracker>,
) -> impl Responder {
    let AppleImportContext {
        user_id,
        node_arc,
        service,
        progress_id,
        tracker,
    } = match init_apple_import_job(
        "apple-contacts",
        "Extracting contacts from Apple Contacts...",
        &state,
        &ingestion_service,
        &progress_tracker,
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };

    let pid = progress_id.clone();
    spawn_apple_import_task(user_id, progress_id, move || async move {
        run_apple_contacts_import(pid, tracker, node_arc, service).await;
    })
}

#[cfg(target_os = "macos")]
async fn run_apple_contacts_import(
    progress_id: String,
    tracker: ProgressTracker,
    node_arc: std::sync::Arc<crate::fold_node::FoldNode>,
    service: std::sync::Arc<crate::ingestion::ingestion_service::IngestionService>,
) {
    use crate::ingestion::apple_import::contacts as ctc;

    let extract_result = tokio::task::spawn_blocking(ctc::extract).await;

    let contacts = match extract_result {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            let mut job = Job::new(progress_id.clone(), JobType::Other("apple-contacts".into()));
            job.status = JobStatus::Failed;
            job.message = format!("Failed to extract contacts: {}", e);
            mark_terminal(&mut job);
            let _ = tracker.save(&job).await;
            return;
        }
        Err(e) => {
            let mut job = Job::new(progress_id.clone(), JobType::Other("apple-contacts".into()));
            job.status = JobStatus::Failed;
            job.message = format!("Extraction task panicked: {}", e);
            mark_terminal(&mut job);
            let _ = tracker.save(&job).await;
            return;
        }
    };

    if contacts.is_empty() {
        let mut job = Job::new(progress_id.clone(), JobType::Other("apple-contacts".into()));
        job.status = JobStatus::Completed;
        job.progress_percentage = 100;
        job.message = "No contacts found".into();
        job.result = Some(json!({ "total": 0, "ingested": 0 }));
        mark_terminal(&mut job);
        let _ = tracker.save(&job).await;
        return;
    }

    let total = contacts.len();
    let records = ctc::to_json_records(&contacts);

    let mut job = Job::new(progress_id.clone(), JobType::Other("apple-contacts".into()));
    job.status = JobStatus::Running;
    job.progress_percentage = 30;
    job.message = format!("Extracted {} contacts, ingesting...", total);
    let _ = tracker.save(&job).await;

    let batch_size = 10;
    let mut ingested = 0;
    let node = node_arc.as_ref();

    for (i, chunk) in records.chunks(batch_size).enumerate() {
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
            forced_schema_descriptive_name: Some("Apple Contacts".to_string()),
        };

        match crate::handlers::ingestion::process_json(
            request,
            &fold_db::user_context::get_current_user_id().unwrap_or_default(),
            &tracker,
            node,
            service.clone(),
        )
        .await
        {
            Ok(_) => ingested += chunk.len(),
            Err(e) => {
                tracing::warn!(
                target: "fold_node::ingestion",
                        "Apple Contacts batch {} failed: {}",
                        i,
                        e
                    );
            }
        }

        let pct = 30 + ((i + 1) * 70 / total.div_ceil(batch_size)).min(70);
        let mut job = Job::new(progress_id.clone(), JobType::Other("apple-contacts".into()));
        job.status = JobStatus::Running;
        job.progress_percentage = pct as u8;
        job.message = format!("Ingested {}/{} contacts...", ingested, total);
        let _ = tracker.save(&job).await;
    }

    let mut job = Job::new(progress_id.clone(), JobType::Other("apple-contacts".into()));
    job.status = JobStatus::Completed;
    job.progress_percentage = 100;
    job.message = format!("Imported {} contacts", ingested);
    job.result = Some(json!({ "total": total, "ingested": ingested }));
    mark_terminal(&mut job);
    let _ = tracker.save(&job).await;
}

#[cfg(not(target_os = "macos"))]
async fn run_apple_contacts_import(
    progress_id: String,
    tracker: ProgressTracker,
    _node_arc: std::sync::Arc<crate::fold_node::FoldNode>,
    _service: std::sync::Arc<crate::ingestion::ingestion_service::IngestionService>,
) {
    let mut job = Job::new(progress_id, JobType::Other("apple-contacts".into()));
    job.status = JobStatus::Failed;
    job.message = "Apple import is only available on macOS".into();
    mark_terminal(&mut job);
    let _ = tracker.save(&job).await;
}

// ── Auto-Sync Config Routes ─────────────────────────────────────────

/// GET /api/ingestion/apple-import/sync-config
pub async fn get_sync_config(sync_config: web::Data<SyncConfigState>) -> impl Responder {
    let cfg = sync_config.read().await;
    HttpResponse::Ok().json(&*cfg)
}

#[derive(Deserialize, Serialize)]
pub struct UpdateSyncConfigRequest {
    pub enabled: Option<bool>,
    pub schedule: Option<apple_import::sync_config::SyncSchedule>,
    pub sources: Option<apple_import::sync_config::EnabledSources>,
    pub photos_limit: Option<usize>,
}

/// POST /api/ingestion/apple-import/sync-config
pub async fn update_sync_config(
    req: web::Json<UpdateSyncConfigRequest>,
    sync_config: web::Data<SyncConfigState>,
) -> impl Responder {
    let mut cfg = sync_config.write().await;

    if let Some(enabled) = req.enabled {
        cfg.enabled = enabled;
    }
    if let Some(ref schedule) = req.schedule {
        cfg.schedule = schedule.clone();
    }
    if let Some(ref sources) = req.sources {
        cfg.sources = sources.clone();
    }
    if let Some(limit) = req.photos_limit {
        cfg.photos_limit = limit;
    }

    cfg.recompute_next_sync();

    match cfg.save() {
        Ok(()) => HttpResponse::Ok().json(&*cfg),
        Err(e) => HttpResponse::InternalServerError().json(json!({
            "success": false,
            "error": format!("Failed to save sync config: {}", e),
        })),
    }
}

/// GET /api/ingestion/apple-import/next-sync
pub async fn get_next_sync(sync_config: web::Data<SyncConfigState>) -> impl Responder {
    let cfg = sync_config.read().await;
    HttpResponse::Ok().json(json!({
        "enabled": cfg.enabled,
        "next_sync": cfg.next_sync,
        "last_sync": cfg.last_sync,
        "last_error": cfg.last_error,
        "last_error_at": cfg.last_error_at,
    }))
}

// ── Background auto-sync scheduler ─────────────────────────────────

/// Run the Apple auto-sync scheduler loop. Awaitable so the caller can track
/// it on a `JoinSet` (via `StartupCtx::spawn_workers`) for graceful shutdown.
///
/// The loop wakes every 60 seconds, checks if `next_sync` has passed, and if
/// so calls `sync_scheduler::run_sync` with the current user's node. After
/// completion it updates `last_sync` / `next_sync` and persists the config.
///
/// First tick is delayed by one period instead of firing immediately —
/// `tokio::time::interval`'s default would race a still-running boot-time
/// bootstrap-resume against `require_node()`'s eager FoldNode creation,
/// caching a node built from a half-restored Sled.
pub async fn run_sync_scheduler(
    sync_config: SyncConfigState,
    app_state: actix_web::web::Data<AppState>,
    ingestion_service: actix_web::web::Data<IngestionServiceState>,
    progress_tracker: actix_web::web::Data<ProgressTracker>,
    upload_storage: actix_web::web::Data<fold_db::storage::UploadStorage>,
) {
    let period = std::time::Duration::from_secs(60);
    let mut interval = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;

        let should_sync = {
            let cfg = sync_config.read().await;
            cfg.enabled && cfg.next_sync.is_some_and(|next| chrono::Utc::now() >= next)
        };

        if !should_sync {
            continue;
        }

        tracing::info!(
        target: "fold_node::ingestion",
            "Apple auto-sync: starting scheduled import"
        );

        let (sources, photos_limit) = {
            let cfg = sync_config.read().await;
            (cfg.sources.clone(), cfg.photos_limit)
        };

        // Resolve current user's node through the same path as HTTP routes.
        let (user_id, node_arc) = match require_node(&app_state).await {
            Ok(ctx) => ctx,
            Err(_) => {
                tracing::warn!(
                target: "fold_node::ingestion",
                            "Apple auto-sync: no active node, skipping"
                        );
                continue;
            }
        };

        let service = match ingestion_service.read().await.clone() {
            Some(s) => s,
            None => {
                tracing::warn!(
                target: "fold_node::ingestion",
                            "Apple auto-sync: ingestion service not available, skipping"
                        );
                continue;
            }
        };

        let tracker = progress_tracker.get_ref().clone();

        let errors = apple_import::sync_scheduler::run_sync(
            &sources,
            photos_limit,
            &user_id,
            node_arc,
            service,
            tracker,
            upload_storage.get_ref().clone(),
        )
        .await;

        {
            let mut cfg = sync_config.write().await;
            let now = chrono::Utc::now();
            if errors.is_empty() {
                cfg.mark_sync_complete(now);
            } else {
                let aggregated = errors.join(" | ");
                tracing::error!(
                target: "fold_node::ingestion",
                            "Apple auto-sync: scheduled import finished with errors: {}",
                            aggregated
                        );
                cfg.mark_sync_error(now, aggregated);
            }
            if let Err(e) = cfg.save() {
                tracing::error!(
                target: "fold_node::ingestion",
                            "Apple auto-sync: failed to persist config: {}",
                            e
                        );
            }
        }

        tracing::info!(
        target: "fold_node::ingestion",
            "Apple auto-sync: scheduled import complete"
        );
    }
}

#[cfg(test)]
mod reminders_final_job_tests {
    use super::build_reminders_final_job;
    use fold_db::progress::{JobStatus, JobType};

    #[test]
    fn success_marks_completed() {
        let job = build_reminders_final_job("p1".into(), 10, 10, None);
        assert!(matches!(job.status, JobStatus::Completed));
        assert_eq!(job.message, "Imported 10 reminders");
        assert_eq!(job.progress_percentage, 100);
        assert!(matches!(job.job_type, JobType::Other(ref s) if s == "apple-reminders"));
        let result = job.result.expect("result present");
        assert_eq!(result["total"], 10);
        assert_eq!(result["ingested"], 10);
    }

    #[test]
    fn ingest_error_marks_failed_and_surfaces_error() {
        // Regression: previously this was marked Completed with ingested=0,
        // masking a full-batch failure as a green checkmark.
        let job = build_reminders_final_job(
            "p2".into(),
            42,
            0,
            Some("schema service unreachable".into()),
        );
        assert!(matches!(job.status, JobStatus::Failed));
        assert!(
            job.message.contains("schema service unreachable"),
            "error should appear in job.message, got: {}",
            job.message,
        );
        let result = job.result.expect("result present");
        assert_eq!(result["total"], 42);
        assert_eq!(result["ingested"], 0);
    }

    #[test]
    fn empty_success_is_completed_not_failed() {
        // total=0, ingested=0, no error — this is a genuinely empty Reminders
        // list, not a failure. Job must be Completed so UI stays green.
        let job = build_reminders_final_job("p3".into(), 0, 0, None);
        assert!(matches!(job.status, JobStatus::Completed));
        assert_eq!(job.message, "Imported 0 reminders");
    }
}

#[cfg(test)]
mod mark_terminal_tests {
    use super::mark_terminal;
    use crate::ingestion::progress::IngestionProgress;
    use fold_db::progress::{Job, JobStatus, JobType};

    #[test]
    fn stamps_completed_at_on_completed_job() {
        let mut job = Job::new("pid-ok".into(), JobType::Other("apple-notes".into()));
        let started = job.created_at;
        assert!(
            job.completed_at.is_none(),
            "Job::new must leave completed_at unset"
        );

        job.status = JobStatus::Completed;
        mark_terminal(&mut job);

        let completed = job
            .completed_at
            .expect("mark_terminal must populate completed_at on Completed jobs");
        assert!(
            completed >= started,
            "completed_at ({}) must be >= started_at ({})",
            completed,
            started,
        );
        assert_eq!(
            job.updated_at, completed,
            "mark_terminal stamps updated_at to the same instant",
        );

        let progress: IngestionProgress = job.into();
        assert!(progress.is_complete);
        assert!(
            progress.completed_at.is_some(),
            "is_complete:true must imply completed_at:Some — that's the whole bug",
        );
        assert!(progress.completed_at.unwrap() >= progress.started_at);
    }

    #[test]
    fn stamps_completed_at_on_failed_job() {
        let mut job = Job::new("pid-fail".into(), JobType::Other("apple-notes".into()));
        let started = job.created_at;

        job.status = JobStatus::Failed;
        job.message = "Failed to extract notes: boom".into();
        mark_terminal(&mut job);

        let completed = job
            .completed_at
            .expect("mark_terminal must populate completed_at on Failed jobs too");
        assert!(completed >= started);

        let progress: IngestionProgress = job.into();
        assert!(progress.is_complete);
        assert!(progress.is_failed);
        assert!(progress.completed_at.is_some());
    }
}
