//! HTTP route handlers for Apple data import (Notes, Reminders, Photos, Calendar).
//!
//! Each endpoint spawns a background task that extracts data via osascript,
//! then feeds it into the ingestion pipeline. Progress is tracked via the
//! standard `ProgressTracker` / `Job` infrastructure.

use actix_web::{web, HttpResponse, Responder};
#[cfg(target_os = "macos")]
use fold_db::log_feature;
#[cfg(target_os = "macos")]
use fold_db::logging::features::LogFeature;
use fold_db::progress::{Job, JobStatus, JobType, ProgressTracker};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::ingestion::apple_import;
use crate::ingestion::apple_import::sync_scheduler::SyncConfigState;
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

/// Persist a failed job via `Job::fail`, preserving the in-flight job's
/// `user_id` so the Sled-backed tracker can actually write it, and ensuring
/// `completed_at` + `error` are populated (not just `status_message`).
///
/// Falls back to a fresh `Job` if the in-flight job can't be loaded — the
/// save will then fail under Sled (needs `user_id`) but the in-memory
/// tracker used by tests still captures the failure.
#[cfg(any(target_os = "macos", test))]
async fn save_failed_job(
    tracker: &ProgressTracker,
    progress_id: &str,
    job_type: &str,
    error: String,
) {
    let mut job = match tracker.load(progress_id).await {
        Ok(Some(existing)) => existing,
        _ => Job::new(progress_id.to_string(), JobType::Other(job_type.into())),
    };
    job.fail(error);
    let _ = tracker.save(&job).await;
}

/// Translate a raw `IngestionError::Extraction` from the Apple Contacts
/// extractor into a user-facing message. `run_osascript` already wraps
/// timeouts with a Settings hint; this adds a product-level preamble so the
/// UI shows "Apple Contacts import failed …" rather than a bare AppleScript
/// error code.
#[cfg(target_os = "macos")]
fn wrap_contacts_extract_error(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("timed out") {
        // run_osascript timeout message is already user-facing; keep it verbatim
        // behind the product prefix.
        format!("Apple Contacts import failed: {}", raw)
    } else if lower.contains("not authorized") || lower.contains("permission") {
        format!(
            "Apple Contacts import failed: macOS did not allow Contacts access. \
             Grant permission in System Settings → Privacy & Security → Automation \
             (or Contacts), then retry. Underlying error: {}",
            raw
        )
    } else {
        format!(
            "Apple Contacts import failed while talking to Contacts.app. \
             Make sure Contacts.app is installed and responsive, then retry. \
             Underlying error: {}",
            raw
        )
    }
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
    if !apple_import::is_available() {
        return HttpResponse::BadRequest().json(json!({
            "success": false,
            "error": "Apple import is only available on macOS",
        }));
    }

    let (user_id, node_arc) = match require_node(&state).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };

    let service = match ingestion_service.read().await.clone() {
        Some(s) => s,
        None => {
            return HttpResponse::ServiceUnavailable().json(json!({
                "success": false,
                "error": "Ingestion service not available",
            }))
        }
    };

    let progress_id = uuid::Uuid::new_v4().to_string();
    let tracker = progress_tracker.get_ref().clone();

    let mut job = Job::new(progress_id.clone(), JobType::Other("apple-notes".into()));
    job = job.with_user(user_id.clone());
    job.message = "Extracting notes from Apple Notes...".into();
    job.progress_percentage = 5;
    let _ = tracker.save(&job).await;

    let folder = request.folder.clone();
    let pid = progress_id.clone();

    tokio::spawn(async move {
        fold_db::logging::core::run_with_user(&user_id, async move {
            run_apple_notes_import(folder, pid, tracker, node_arc, service).await;
        })
        .await;
    });

    HttpResponse::Accepted().json(json!({
        "success": true,
        "progress_id": progress_id,
    }))
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
            let _ = tracker.save(&job).await;
            return;
        }
        Err(e) => {
            let mut job = Job::new(progress_id.clone(), JobType::Other("apple-notes".into()));
            job.status = JobStatus::Failed;
            job.message = format!("Extraction task panicked: {}", e);
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
        };

        match crate::handlers::ingestion::process_json(
            request,
            &fold_db::logging::core::get_current_user_id().unwrap_or_default(),
            &tracker,
            node,
            service.clone(),
        )
        .await
        {
            Ok(_) => ingested += chunk.len(),
            Err(e) => {
                log_feature!(
                    LogFeature::Ingestion,
                    warn,
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
    if !apple_import::is_available() {
        return HttpResponse::BadRequest().json(json!({
            "success": false,
            "error": "Apple import is only available on macOS",
        }));
    }

    let (user_id, node_arc) = match require_node(&state).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };

    let service = match ingestion_service.read().await.clone() {
        Some(s) => s,
        None => {
            return HttpResponse::ServiceUnavailable().json(json!({
                "success": false,
                "error": "Ingestion service not available",
            }))
        }
    };

    let progress_id = uuid::Uuid::new_v4().to_string();
    let tracker = progress_tracker.get_ref().clone();

    let mut job = Job::new(
        progress_id.clone(),
        JobType::Other("apple-reminders".into()),
    );
    job = job.with_user(user_id.clone());
    job.message = "Extracting reminders...".into();
    job.progress_percentage = 5;
    let _ = tracker.save(&job).await;

    let list = request.list.clone();
    let pid = progress_id.clone();

    tokio::spawn(async move {
        fold_db::logging::core::run_with_user(&user_id, async move {
            run_apple_reminders_import(list, pid, tracker, node_arc, service).await;
        })
        .await;
    });

    HttpResponse::Accepted().json(json!({
        "success": true,
        "progress_id": progress_id,
    }))
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
    };

    let (ingested, ingest_error) = match crate::handlers::ingestion::process_json(
        request,
        &fold_db::logging::core::get_current_user_id().unwrap_or_default(),
        &tracker,
        node,
        service,
    )
    .await
    {
        Ok(_) => (total, None),
        Err(e) => {
            log_feature!(
                LogFeature::Ingestion,
                warn,
                "Apple Reminders ingestion failed: {}",
                e
            );
            (0, Some(e.to_string()))
        }
    };

    let job = build_reminders_final_job(progress_id.clone(), total, ingested, ingest_error);
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
    let _ = tracker.save(&job).await;
}

#[derive(Deserialize)]
pub struct ApplePhotosRequest {
    pub album: Option<String>,
    pub limit: Option<usize>,
}

/// POST /api/ingestion/apple-import/photos
pub async fn apple_import_photos(
    request: web::Json<ApplePhotosRequest>,
    state: web::Data<AppState>,
    ingestion_service: web::Data<IngestionServiceState>,
    progress_tracker: web::Data<ProgressTracker>,
) -> impl Responder {
    if !apple_import::is_available() {
        return HttpResponse::BadRequest().json(json!({
            "success": false,
            "error": "Apple import is only available on macOS",
        }));
    }

    let (user_id, node_arc) = match require_node(&state).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };

    let service = match ingestion_service.read().await.clone() {
        Some(s) => s,
        None => {
            return HttpResponse::ServiceUnavailable().json(json!({
                "success": false,
                "error": "Ingestion service not available",
            }));
        }
    };

    let progress_id = uuid::Uuid::new_v4().to_string();
    let tracker = progress_tracker.get_ref().clone();

    let mut job = Job::new(progress_id.clone(), JobType::Other("apple-photos".into()));
    job = job.with_user(user_id.clone());
    job.message = "Exporting photos from Apple Photos...".into();
    job.progress_percentage = 5;
    let _ = tracker.save(&job).await;

    let album = request.album.clone();
    let limit = request.limit.unwrap_or(50);
    let pid = progress_id.clone();

    tokio::spawn(async move {
        fold_db::logging::core::run_with_user(&user_id, async move {
            run_apple_photos_import(album, limit, pid, tracker, node_arc, service).await;
        })
        .await;
    });

    HttpResponse::Accepted().json(json!({
        "success": true,
        "progress_id": progress_id,
    }))
}

#[cfg(target_os = "macos")]
async fn run_apple_photos_import(
    album: Option<String>,
    limit: usize,
    progress_id: String,
    tracker: ProgressTracker,
    node_arc: std::sync::Arc<crate::fold_node::FoldNode>,
    service: std::sync::Arc<crate::ingestion::ingestion_service::IngestionService>,
) {
    use crate::ingestion::apple_import::photos;

    let photos_result =
        tokio::task::spawn_blocking(move || photos::export(album.as_deref(), limit)).await;

    let paths = match photos_result {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            let mut job = Job::new(progress_id.clone(), JobType::Other("apple-photos".into()));
            job.status = JobStatus::Failed;
            job.message = format!("Failed to export photos: {}", e);
            let _ = tracker.save(&job).await;
            return;
        }
        Err(e) => {
            let mut job = Job::new(progress_id.clone(), JobType::Other("apple-photos".into()));
            job.status = JobStatus::Failed;
            job.message = format!("Export task panicked: {}", e);
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
    let mut ingested = 0;

    for (i, path) in paths.iter().enumerate() {
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
                            log_feature!(
                                LogFeature::Ingestion,
                                warn,
                                "Visibility classification failed, skipping: {}",
                                e
                            );
                        }
                    }
                }

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

                match crate::handlers::ingestion::process_json(
                    request,
                    &fold_db::logging::core::get_current_user_id().unwrap_or_default(),
                    &tracker,
                    node,
                    service.clone(),
                )
                .await
                {
                    Ok(_) => ingested += 1,
                    Err(e) => {
                        log_feature!(
                            LogFeature::Ingestion,
                            warn,
                            "Failed to ingest photo {}: {}",
                            file_name,
                            e
                        );
                    }
                }
            }
            Err(e) => {
                log_feature!(
                    LogFeature::Ingestion,
                    warn,
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
) {
    let mut job = Job::new(progress_id, JobType::Other("apple-photos".into()));
    job.status = JobStatus::Failed;
    job.message = "Apple import is only available on macOS".into();
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
    if !apple_import::is_available() {
        return HttpResponse::BadRequest().json(json!({
            "success": false,
            "error": "Apple import is only available on macOS",
        }));
    }

    let (user_id, node_arc) = match require_node(&state).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };

    let service = match ingestion_service.read().await.clone() {
        Some(s) => s,
        None => {
            return HttpResponse::ServiceUnavailable().json(json!({
                "success": false,
                "error": "Ingestion service not available",
            }))
        }
    };

    let progress_id = uuid::Uuid::new_v4().to_string();
    let tracker = progress_tracker.get_ref().clone();

    let mut job = Job::new(progress_id.clone(), JobType::Other("apple-calendar".into()));
    job = job.with_user(user_id.clone());
    job.message = "Extracting events from Apple Calendar...".into();
    job.progress_percentage = 5;
    let _ = tracker.save(&job).await;

    let calendar = request.calendar.clone();
    let pid = progress_id.clone();

    tokio::spawn(async move {
        fold_db::logging::core::run_with_user(&user_id, async move {
            run_apple_calendar_import(calendar, pid, tracker, node_arc, service).await;
        })
        .await;
    });

    HttpResponse::Accepted().json(json!({
        "success": true,
        "progress_id": progress_id,
    }))
}

/// Build the `TextRecordDto` batch that feeds the attendee-email
/// fingerprint extractor after a calendar import. Pure transform —
/// no network, no node access — so it's cheap to unit-test and easy
/// to reason about:
///
/// - Events with an empty attendees list are dropped (nothing to
///   extract). The text extractor would be a no-op on them anyway
///   and the cost of emitting empty rows compounds quickly on large
///   calendars.
/// - `source_key` is the same `content_hash(summary|start|calendar)`
///   that `cal::to_json_records` uses, so Mention records join back
///   to the calendar event cleanly.
/// - Attendees are joined with `", "` so the regex extractor treats
///   them as one pass of the email rule. Joining is not a Fingerprint
///   deduplication concern — the extractor content-hashes each email
///   independently.
///
/// Gated to macOS because the only caller is the macOS calendar
/// import path, and the two helpers it depends on (`CalendarEvent`
/// plus `content_hash`) are themselves macOS-gated. The unit tests
/// below share the gate.
#[cfg(target_os = "macos")]
fn build_attendee_ingestion_records(
    events: &[crate::ingestion::apple_import::calendar::CalendarEvent],
) -> Vec<crate::handlers::fingerprints::ingest_text::TextRecordDto> {
    events
        .iter()
        .filter(|e| !e.attendees.is_empty())
        .map(|e| {
            let hash_input = format!("{}|{}|{}", e.summary, e.start_time, e.calendar);
            let source_key = crate::ingestion::apple_import::content_hash(&hash_input);
            crate::handlers::fingerprints::ingest_text::TextRecordDto {
                source_key,
                text: e.attendees.join(", "),
            }
        })
        .collect()
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
            let _ = tracker.save(&job).await;
            return;
        }
        Err(e) => {
            let mut job = Job::new(progress_id.clone(), JobType::Other("apple-calendar".into()));
            job.status = JobStatus::Failed;
            job.message = format!("Extraction task panicked: {}", e);
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
        };

        match crate::handlers::ingestion::process_json(
            request,
            &fold_db::logging::core::get_current_user_id().unwrap_or_default(),
            &tracker,
            node,
            service.clone(),
        )
        .await
        {
            Ok(_) => ingested += chunk.len(),
            Err(e) => {
                log_feature!(
                    LogFeature::Ingestion,
                    warn,
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

    // Fingerprint extraction for attendee emails. Each event with a
    // non-empty attendees list becomes one text-ingestion record; the
    // existing email regex extractor in `ingest_text_signals_batch`
    // then writes a Fingerprint (email) + Mention (pointing back at
    // the calendar event by content_hash) per address. Attendees are
    // joined into one text blob per event so a single extractor pass
    // picks up every email without caring about ordering.
    //
    // Failures here are logged but do NOT fail the calendar import —
    // events have already been ingested and are queryable. A broken
    // fingerprint pipeline is a separate bug and shouldn't poison a
    // successful calendar sync.
    let attendee_records = build_attendee_ingestion_records(&events);
    if !attendee_records.is_empty() {
        let attendee_count = attendee_records.len();
        let request = crate::handlers::fingerprints::ingest_text::IngestTextSignalsRequest {
            source_schema: "apple_calendar".to_string(),
            records: attendee_records,
        };
        match crate::handlers::fingerprints::ingest_text::ingest_text_signals_batch(
            node_arc.clone(),
            request,
        )
        .await
        {
            Ok(response) => {
                let signal_count = response.data.as_ref().map(|r| r.total_signals).unwrap_or(0);
                let written = response
                    .data
                    .as_ref()
                    .map(|r| r.total_records_written)
                    .unwrap_or(0);
                log_feature!(
                    LogFeature::Ingestion,
                    info,
                    "Apple Calendar: extracted {} email signals from {} events ({} records written)",
                    signal_count,
                    attendee_count,
                    written,
                );
            }
            Err(e) => {
                log_feature!(
                    LogFeature::Ingestion,
                    warn,
                    "Apple Calendar: attendee fingerprint extraction failed for {} events: {:?}",
                    attendee_count,
                    e,
                );
            }
        }
    }

    let mut job = Job::new(progress_id.clone(), JobType::Other("apple-calendar".into()));
    job.status = JobStatus::Completed;
    job.progress_percentage = 100;
    job.message = format!("Imported {} calendar events", ingested);
    job.result = Some(json!({ "total": total, "ingested": ingested }));
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
    if !apple_import::is_available() {
        return HttpResponse::BadRequest().json(json!({
            "success": false,
            "error": "Apple import is only available on macOS",
        }));
    }

    let (user_id, node_arc) = match require_node(&state).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };

    let service = match ingestion_service.read().await.clone() {
        Some(s) => s,
        None => {
            return HttpResponse::ServiceUnavailable().json(json!({
                "success": false,
                "error": "Ingestion service not available",
            }))
        }
    };

    let progress_id = uuid::Uuid::new_v4().to_string();
    let tracker = progress_tracker.get_ref().clone();

    let mut job = Job::new(progress_id.clone(), JobType::Other("apple-contacts".into()));
    job = job.with_user(user_id.clone());
    job.message = "Extracting contacts from Apple Contacts...".into();
    job.progress_percentage = 5;
    let _ = tracker.save(&job).await;

    let pid = progress_id.clone();

    tokio::spawn(async move {
        fold_db::logging::core::run_with_user(&user_id, async move {
            run_apple_contacts_import(pid, tracker, node_arc, service).await;
        })
        .await;
    });

    HttpResponse::Accepted().json(json!({
        "success": true,
        "progress_id": progress_id,
    }))
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
            save_failed_job(
                &tracker,
                &progress_id,
                "apple-contacts",
                wrap_contacts_extract_error(&e.to_string()),
            )
            .await;
            return;
        }
        Err(e) => {
            save_failed_job(
                &tracker,
                &progress_id,
                "apple-contacts",
                format!("Apple Contacts import crashed unexpectedly: {}", e),
            )
            .await;
            return;
        }
    };

    if contacts.is_empty() {
        let mut job = Job::new(progress_id.clone(), JobType::Other("apple-contacts".into()));
        job.status = JobStatus::Completed;
        job.progress_percentage = 100;
        job.message = "No contacts found".into();
        job.result = Some(json!({ "total": 0, "ingested": 0 }));
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
        };

        match crate::handlers::ingestion::process_json(
            request,
            &fold_db::logging::core::get_current_user_id().unwrap_or_default(),
            &tracker,
            node,
            service.clone(),
        )
        .await
        {
            Ok(_) => ingested += chunk.len(),
            Err(e) => {
                log_feature!(
                    LogFeature::Ingestion,
                    warn,
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

/// Spawn the background sync scheduler loop.
///
/// The task wakes every 60 seconds, checks if `next_sync` has passed, and if
/// so calls `sync_scheduler::run_sync` with the current user's node. After
/// completion it updates `last_sync` / `next_sync` and persists the config.
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
                cfg.enabled && cfg.next_sync.is_some_and(|next| chrono::Utc::now() >= next)
            };

            if !should_sync {
                continue;
            }

            fold_db::log_feature!(
                fold_db::logging::features::LogFeature::Ingestion,
                info,
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
                    fold_db::log_feature!(
                        fold_db::logging::features::LogFeature::Ingestion,
                        warn,
                        "Apple auto-sync: no active node, skipping"
                    );
                    continue;
                }
            };

            let service = match ingestion_service.read().await.clone() {
                Some(s) => s,
                None => {
                    fold_db::log_feature!(
                        fold_db::logging::features::LogFeature::Ingestion,
                        warn,
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
            )
            .await;

            {
                let mut cfg = sync_config.write().await;
                let now = chrono::Utc::now();
                if errors.is_empty() {
                    cfg.mark_sync_complete(now);
                } else {
                    let aggregated = errors.join(" | ");
                    fold_db::log_feature!(
                        fold_db::logging::features::LogFeature::Ingestion,
                        error,
                        "Apple auto-sync: scheduled import finished with errors: {}",
                        aggregated
                    );
                    cfg.mark_sync_error(now, aggregated);
                }
                if let Err(e) = cfg.save() {
                    fold_db::log_feature!(
                        fold_db::logging::features::LogFeature::Ingestion,
                        error,
                        "Apple auto-sync: failed to persist config: {}",
                        e
                    );
                }
            }

            fold_db::log_feature!(
                fold_db::logging::features::LogFeature::Ingestion,
                info,
                "Apple auto-sync: scheduled import complete"
            );
        }
    });
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::build_attendee_ingestion_records;
    use crate::ingestion::apple_import::calendar::CalendarEvent;

    fn evt(summary: &str, start: &str, calendar: &str, attendees: Vec<&str>) -> CalendarEvent {
        CalendarEvent {
            summary: summary.into(),
            start_time: start.into(),
            end_time: "end".into(),
            location: String::new(),
            description: String::new(),
            calendar: calendar.into(),
            all_day: false,
            recurring: false,
            attendees: attendees.into_iter().map(String::from).collect(),
        }
    }

    #[test]
    fn skips_events_with_no_attendees() {
        let events = vec![evt("Empty", "2026-04-17", "Work", vec![])];
        let out = build_attendee_ingestion_records(&events);
        assert!(out.is_empty());
    }

    #[test]
    fn one_record_per_event_with_attendees_joined() {
        let events = vec![evt(
            "Standup",
            "2026-04-17",
            "Work",
            vec!["alice@x.com", "bob@y.com"],
        )];
        let out = build_attendee_ingestion_records(&events);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "alice@x.com, bob@y.com");
        // source_key is a sha256-ish hex string; we only assert
        // it's non-empty here — the exact hash is pinned by the
        // content_hash helper's contract.
        assert!(!out[0].source_key.is_empty());
    }

    #[test]
    fn source_key_differs_for_different_events() {
        let events = vec![
            evt("A", "t1", "c1", vec!["a@b.c"]),
            evt("B", "t2", "c2", vec!["d@e.f"]),
        ];
        let out = build_attendee_ingestion_records(&events);
        assert_eq!(out.len(), 2);
        assert_ne!(out[0].source_key, out[1].source_key);
    }

    #[test]
    fn source_key_stable_for_same_identifying_fields() {
        // Same summary + start + calendar → same key even if
        // description / location change. This matches the key
        // contract in cal::to_json_records.
        let a = evt("Standup", "t", "Work", vec!["x@y.z"]);
        let b = evt("Standup", "t", "Work", vec!["x@y.z"]);
        let out_a = build_attendee_ingestion_records(&[a]);
        let out_b = build_attendee_ingestion_records(&[b]);
        assert_eq!(out_a[0].source_key, out_b[0].source_key);
    }

    #[test]
    fn drops_only_empty_events_keeps_the_rest() {
        let events = vec![
            evt("Empty", "t1", "c", vec![]),
            evt("One", "t2", "c", vec!["a@b.c"]),
            evt("Also empty", "t3", "c", vec![]),
            evt("Two", "t4", "c", vec!["d@e.f", "g@h.i"]),
        ];
        let out = build_attendee_ingestion_records(&events);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].text, "a@b.c");
        assert_eq!(out[1].text, "d@e.f, g@h.i");
    }
}

#[cfg(all(test, target_os = "macos"))]
mod contacts_error_tests {
    use super::{save_failed_job, wrap_contacts_extract_error};
    use fold_db::progress::{create_tracker, Job, JobStatus, JobType};

    #[test]
    fn wrap_timeout_preserves_run_osascript_hint() {
        let raw = "osascript timed out after 300 seconds talking to Contacts.app. \
                   The app may be unresponsive, syncing with iCloud, or missing \
                   Automation permission. Grant access in System Settings → \
                   Privacy & Security → Automation (and Full Disk Access for Photos.app).";
        let wrapped = wrap_contacts_extract_error(raw);
        assert!(wrapped.starts_with("Apple Contacts import failed: "));
        assert!(wrapped.contains("timed out"));
        assert!(wrapped.contains("Automation"));
    }

    #[test]
    fn wrap_permission_error_points_to_settings() {
        let raw = "AppleScript error (Contacts.app): Not authorized to send Apple events to Contacts.";
        let wrapped = wrap_contacts_extract_error(raw);
        assert!(wrapped.contains("System Settings"));
        assert!(wrapped.contains("Privacy & Security"));
        assert!(wrapped.contains(raw), "raw error must be preserved for debuggability");
    }

    #[test]
    fn wrap_generic_error_wraps_with_product_prefix() {
        let raw = "AppleScript error (Contacts.app): execution error: -1728";
        let wrapped = wrap_contacts_extract_error(raw);
        assert!(wrapped.starts_with("Apple Contacts import failed while talking to Contacts.app."));
        assert!(wrapped.contains(raw));
    }

    #[tokio::test]
    async fn save_failed_job_populates_error_and_completed_at() {
        // Repro for a2a0c: before this fix, the contacts handler created a bare
        // Job and only set status + message, leaving error=None and
        // completed_at=None in the IngestionProgress response.
        let tracker = create_tracker().await;

        let mut seed = Job::new("pid".into(), JobType::Other("apple-contacts".into()));
        seed = seed.with_user("user-123".into());
        seed.update_progress(5, "Extracting contacts from Apple Contacts...".into());
        tracker.save(&seed).await.expect("seed save");

        save_failed_job(&tracker, "pid", "apple-contacts", "boom".into()).await;

        let loaded = tracker.load("pid").await.unwrap().expect("job present");
        assert!(matches!(loaded.status, JobStatus::Failed));
        assert_eq!(loaded.error.as_deref(), Some("boom"));
        assert!(
            loaded.completed_at.is_some(),
            "completed_at must be set on failure, got None",
        );
        assert!(
            loaded.message.contains("boom"),
            "status_message should wrap the error, got: {}",
            loaded.message,
        );
        assert_eq!(
            loaded.user_id.as_deref(),
            Some("user-123"),
            "user_id from the in-flight job must be preserved so Sled saves",
        );
    }

    #[tokio::test]
    async fn save_failed_job_falls_back_when_job_missing() {
        // If the seed save never landed (e.g. tracker hiccup) we should still
        // record a terminal failure rather than silently drop it.
        let tracker = create_tracker().await;

        save_failed_job(&tracker, "missing", "apple-contacts", "oops".into()).await;

        let loaded = tracker
            .load("missing")
            .await
            .unwrap()
            .expect("fallback job persisted");
        assert!(matches!(loaded.status, JobStatus::Failed));
        assert_eq!(loaded.error.as_deref(), Some("oops"));
        assert!(loaded.completed_at.is_some());
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
