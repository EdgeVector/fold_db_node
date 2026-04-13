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

    let ingested = match crate::handlers::ingestion::process_json(
        request,
        &fold_db::logging::core::get_current_user_id().unwrap_or_default(),
        &tracker,
        node,
        service,
    )
    .await
    {
        Ok(_) => total,
        Err(e) => {
            log_feature!(
                LogFeature::Ingestion,
                warn,
                "Apple Reminders ingestion failed: {}",
                e
            );
            0
        }
    };

    let mut job = Job::new(
        progress_id.clone(),
        JobType::Other("apple-reminders".into()),
    );
    job.status = JobStatus::Completed;
    job.progress_percentage = 100;
    job.message = format!("Imported {} reminders", ingested);
    job.result = Some(json!({ "total": total, "ingested": ingested }));
    let _ = tracker.save(&job).await;
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

            apple_import::sync_scheduler::run_sync(
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
                cfg.mark_sync_complete(chrono::Utc::now());
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
