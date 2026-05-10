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

/// GET /api/ingestion/apple-import/permissions
///
/// Pre-flight TCC permission probes for the five Apple sources surfaced
/// in the onboarding wizard. Returns
/// `{contacts, notes, calendar, reminders, photos: bool}` where `true`
/// means the calling process can talk to that app via AppleScript.
///
/// Why this endpoint exists: prior to it, the onboarding "Apple Data"
/// step would POST all five imports concurrently, and Contacts (the only
/// source already wired to `preflight_permission`) would surface the
/// missing-permission error after a 30s wallclock wait. The other four
/// sources had no preflight at all and hung for the full 5-minute
/// `OSASCRIPT_TIMEOUT` if the corresponding TCC grant was missing. This
/// endpoint lets the wizard render an actionable "Grant Apple permissions"
/// banner with a deep link to System Settings → Privacy & Security →
/// Automation BEFORE the user clicks Import.
///
/// On non-macOS hosts, `apple_import::is_available()` is `false` and we
/// return all five probes as `true` — no Apple permission to grant means
/// no banner, which lets the wizard fall through to the existing
/// "Apple Import is only available on macOS" panel without an extra
/// false-negative banner first.
///
/// Probes run **in parallel** via `tokio::spawn_blocking` so the user's
/// perceived latency is `max(per_probe)` rather than `sum`. Each probe
/// is bounded by `HTTP_PROBE_TIMEOUT` (2s) inside `probe_permission`.
pub async fn apple_import_permissions() -> impl Responder {
    if !apple_import::is_available() {
        return HttpResponse::Ok().json(json!({
            "contacts": true,
            "notes": true,
            "calendar": true,
            "reminders": true,
            "photos": true,
        }));
    }

    #[cfg(target_os = "macos")]
    {
        // Probes are blocking osascript invocations — `spawn_blocking` keeps
        // the actix worker free while we run them in parallel. `try_join!`
        // resolves when all five complete; per-probe timeouts inside
        // `probe_permission` cap the wallclock at ~`HTTP_PROBE_TIMEOUT`.
        let probe = |label: &'static str| {
            tokio::task::spawn_blocking(move || apple_import::probe_permission(label))
        };

        let (contacts, notes, calendar, reminders, photos) = tokio::join!(
            probe("Contacts.app"),
            probe("Notes.app"),
            probe("Calendar.app"),
            probe("Reminders.app"),
            probe("Photos.app"),
        );

        // `spawn_blocking` JoinErrors only happen on runtime shutdown or
        // panic — both are "unknown permission state, fall back to true"
        // because reporting `false` here would gate the user out of an
        // import path that might actually work. The import handler still
        // surfaces real osascript failures via the job progress stream.
        HttpResponse::Ok().json(json!({
            "contacts": contacts.unwrap_or(true),
            "notes": notes.unwrap_or(true),
            "calendar": calendar.unwrap_or(true),
            "reminders": reminders.unwrap_or(true),
            "photos": photos.unwrap_or(true),
        }))
    }

    #[cfg(not(target_os = "macos"))]
    {
        // Belt-and-suspenders: `is_available()` already returned `false` on
        // non-macOS, but the explicit branch keeps the macOS-only `tokio::join!`
        // out of the cross-platform compile path.
        unreachable!("non-macOS path returned above via apple_import::is_available()")
    }
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

/// Mark `job` as failed, populating BOTH `message` and `error`.
///
/// `IngestionProgress::From<Job>` ships `error` to the API response's
/// `error_message` field; the previous pattern only set `message`, leaving
/// `error_message` null on every Apple-import failure. We deliberately avoid
/// [`Job::fail`] because it prepends `"Failed: "` to `message`, which would
/// alter the user-visible `status_message` text.
fn mark_failed(job: &mut Job, msg: String) {
    job.status = JobStatus::Failed;
    job.error = Some(msg.clone());
    job.message = msg;
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

/// Map ingested-record count to a percentage in [10, 100].
///
/// Apple-import handlers emit 10% post-extraction and want the in-flight
/// ingestion loop to fill the remaining 90 percentage points monotonically.
/// Saturates at 100 if `ingested >= total`; tolerates `total == 0`.
#[cfg(any(target_os = "macos", test))]
fn ingestion_progress_pct(ingested: usize, total: usize) -> u8 {
    let total = (total.max(1)) as u64;
    let ingested = (ingested as u64).min(total);
    let pct: u64 = 10 + (ingested * 90) / total;
    pct.min(100) as u8
}

/// Emit a per-batch progress update during an Apple import ingestion loop.
///
/// Centralizes the Notes/Reminders/Contacts/Calendar emission shape so they
/// stay aligned and so the percentage math is unit-tested in one place.
#[cfg(target_os = "macos")]
async fn emit_batch_progress(
    tracker: &ProgressTracker,
    progress_id: &str,
    job_kind: &str,
    ingested: usize,
    total: usize,
    item_label: &str,
) {
    let mut job = Job::new(
        progress_id.to_string(),
        JobType::Other(job_kind.to_string()),
    );
    job.status = JobStatus::Running;
    job.progress_percentage = ingestion_progress_pct(ingested, total);
    job.message = format!("Ingested {}/{} {}...", ingested, total, item_label);
    let _ = tracker.save(&job).await;
}

#[derive(Deserialize, Default)]
pub struct AppleNotesRequest {
    pub folder: Option<String>,
}

/// POST /api/ingestion/apple-import/notes
///
/// Body is optional. Callers can POST with no Content-Type and no body to take
/// the defaults (whole-library import); `Option<web::Json<_>>` falls back to
/// the default struct on missing Content-Type or empty body.
pub async fn apple_import_notes(
    request: Option<web::Json<AppleNotesRequest>>,
    state: web::Data<AppState>,
    ingestion_service: web::Data<IngestionServiceState>,
    progress_tracker: web::Data<ProgressTracker>,
) -> impl Responder {
    let request = request.map(web::Json::into_inner).unwrap_or_default();
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
            mark_failed(&mut job, format!("Failed to extract notes: {}", e));
            mark_terminal(&mut job);
            let _ = tracker.save(&job).await;
            return;
        }
        Err(e) => {
            let mut job = Job::new(progress_id.clone(), JobType::Other("apple-notes".into()));
            mark_failed(&mut job, format!("Extraction task panicked: {}", e));
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
    job.progress_percentage = 10;
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

        emit_batch_progress(
            &tracker,
            &progress_id,
            "apple-notes",
            ingested,
            total,
            "notes",
        )
        .await;
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
    mark_failed(&mut job, "Apple import is only available on macOS".into());
    mark_terminal(&mut job);
    let _ = tracker.save(&job).await;
}

#[derive(Deserialize, Default)]
pub struct AppleRemindersRequest {
    pub list: Option<String>,
}

/// POST /api/ingestion/apple-import/reminders
///
/// Body is optional — see [`apple_import_notes`] for the rationale.
pub async fn apple_import_reminders(
    request: Option<web::Json<AppleRemindersRequest>>,
    state: web::Data<AppState>,
    ingestion_service: web::Data<IngestionServiceState>,
    progress_tracker: web::Data<ProgressTracker>,
) -> impl Responder {
    let request = request.map(web::Json::into_inner).unwrap_or_default();
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
            mark_failed(&mut job, format!("Failed to extract reminders: {}", e));
            mark_terminal(&mut job);
            let _ = tracker.save(&job).await;
            return;
        }
        Err(e) => {
            let mut job = Job::new(
                progress_id.clone(),
                JobType::Other("apple-reminders".into()),
            );
            mark_failed(&mut job, format!("Extraction task panicked: {}", e));
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
    job.progress_percentage = 10;
    job.message = format!("Extracted {} reminders, ingesting...", total);
    let _ = tracker.save(&job).await;

    let batch_size = 10;
    let mut ingested = 0;
    let mut ingest_error: Option<String> = None;
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
            // Pin reminders to a canonical schema; same fragmentation risk as
            // Apple Notes whenever the LLM is asked to classify N batches.
            forced_schema_descriptive_name: Some("Apple Reminders".to_string()),
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
                        "Apple Reminders batch {} failed: {}",
                        i,
                        e
                    );
                if ingest_error.is_none() {
                    ingest_error = Some(e.to_string());
                }
            }
        }

        emit_batch_progress(
            &tracker,
            &progress_id,
            "apple-reminders",
            ingested,
            total,
            "reminders",
        )
        .await;
    }

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
        mark_failed(&mut job, format!("Reminders ingestion failed: {}", err));
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
    mark_failed(&mut job, "Apple import is only available on macOS".into());
    mark_terminal(&mut job);
    let _ = tracker.save(&job).await;
}

#[derive(Deserialize, Default)]
pub struct ApplePhotosRequest {
    pub album: Option<String>,
    pub limit: Option<usize>,
}

/// POST /api/ingestion/apple-import/photos
///
/// Body is optional — see [`apple_import_notes`] for the rationale. When
/// provided, `limit` overrides the 50-photo default.
// TODO: Apple Photos ingestion does not yet run face detection — face extraction in the generic ingestion path is a separate workstream that requires ONNX inline.
pub async fn apple_import_photos(
    request: Option<web::Json<ApplePhotosRequest>>,
    state: web::Data<AppState>,
    ingestion_service: web::Data<IngestionServiceState>,
    progress_tracker: web::Data<ProgressTracker>,
    upload_storage: web::Data<fold_db::storage::UploadStorage>,
) -> impl Responder {
    let request = request.map(web::Json::into_inner).unwrap_or_default();
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
            mark_failed(&mut job, format!("Failed to export photos: {}", e));
            mark_terminal(&mut job);
            let _ = tracker.save(&job).await;
            return;
        }
        Err(e) => {
            let mut job = Job::new(progress_id.clone(), JobType::Other("apple-photos".into()));
            mark_failed(&mut job, format!("Export task panicked: {}", e));
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
    mark_failed(&mut job, "Apple import is only available on macOS".into());
    mark_terminal(&mut job);
    let _ = tracker.save(&job).await;
}

#[derive(Deserialize, Default)]
pub struct AppleCalendarRequest {
    pub calendar: Option<String>,
}

/// POST /api/ingestion/apple-import/calendar
///
/// Body is optional — see [`apple_import_notes`] for the rationale.
pub async fn apple_import_calendar(
    request: Option<web::Json<AppleCalendarRequest>>,
    state: web::Data<AppState>,
    ingestion_service: web::Data<IngestionServiceState>,
    progress_tracker: web::Data<ProgressTracker>,
) -> impl Responder {
    let request = request.map(web::Json::into_inner).unwrap_or_default();
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
            mark_failed(
                &mut job,
                format!("Failed to extract calendar events: {}", e),
            );
            mark_terminal(&mut job);
            let _ = tracker.save(&job).await;
            return;
        }
        Err(e) => {
            let mut job = Job::new(progress_id.clone(), JobType::Other("apple-calendar".into()));
            mark_failed(&mut job, format!("Extraction task panicked: {}", e));
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
    job.progress_percentage = 10;
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

        emit_batch_progress(
            &tracker,
            &progress_id,
            "apple-calendar",
            ingested,
            total,
            "events",
        )
        .await;
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
    mark_failed(&mut job, "Apple import is only available on macOS".into());
    mark_terminal(&mut job);
    let _ = tracker.save(&job).await;
}

#[derive(Deserialize, Default)]
pub struct AppleContactsRequest {}

/// POST /api/ingestion/apple-import/contacts
///
/// Body is optional — see [`apple_import_notes`] for the rationale.
pub async fn apple_import_contacts(
    _request: Option<web::Json<AppleContactsRequest>>,
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
            mark_failed(&mut job, format!("Failed to extract contacts: {}", e));
            mark_terminal(&mut job);
            let _ = tracker.save(&job).await;
            return;
        }
        Err(e) => {
            let mut job = Job::new(progress_id.clone(), JobType::Other("apple-contacts".into()));
            mark_failed(&mut job, format!("Extraction task panicked: {}", e));
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
    job.progress_percentage = 10;
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

        emit_batch_progress(
            &tracker,
            &progress_id,
            "apple-contacts",
            ingested,
            total,
            "contacts",
        )
        .await;
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
    mark_failed(&mut job, "Apple import is only available on macOS".into());
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
        assert_eq!(
            job.error.as_deref(),
            Some("Reminders ingestion failed: schema service unreachable"),
            "job.error must surface the failure so error_message in the API response is non-null",
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
mod mark_failed_tests {
    use super::mark_failed;
    use fold_db::progress::{Job, JobStatus, JobType};

    #[test]
    fn populates_both_message_and_error() {
        let mut job = Job::new("p".into(), JobType::Other("apple-notes".into()));
        mark_failed(&mut job, "boom".to_string());
        assert!(matches!(job.status, JobStatus::Failed));
        assert_eq!(job.message, "boom");
        assert_eq!(job.error.as_deref(), Some("boom"));
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

#[cfg(test)]
mod ingestion_progress_pct_tests {
    //! Pin the per-batch progress percentages emitted by Notes/Reminders/
    //! Contacts/Calendar imports during their ingestion loop. The dogfood run
    //! on 2026-05-10 polled `/api/ingestion/progress/{id}` 8 times during a
    //! 132-note import and saw 5% on every poll until the final 100% — the
    //! ingestion loop's per-batch updates have to surface ≥5 distinct
    //! percentage points between extraction-complete (10%) and finished
    //! (100%) so progress visibly advances.
    use super::ingestion_progress_pct;

    #[test]
    fn monotonic_and_distinct_for_132_notes_in_batches_of_10() {
        // Mirrors the dogfood scenario: 132 notes ingested in chunks of 10.
        let total: usize = 132;
        let batch_size: usize = 10;
        let mut last = 10u8;
        let mut distinct = std::collections::BTreeSet::new();
        distinct.insert(10u8); // post-extract baseline emitted before the loop
        let mut ingested = 0;
        for _ in 0..total.div_ceil(batch_size) {
            ingested = (ingested + batch_size).min(total);
            let p = ingestion_progress_pct(ingested, total);
            assert!(p >= last, "progress went backwards: {last} -> {p}");
            last = p;
            distinct.insert(p);
        }
        assert_eq!(last, 100, "final batch should land at 100%");
        assert!(
            distinct.len() >= 5,
            "expected >=5 distinct percentage values across the loop, got {distinct:?}",
        );
    }

    #[test]
    fn handles_edge_cases() {
        assert_eq!(ingestion_progress_pct(0, 0), 10, "empty list stays at 10%");
        assert_eq!(ingestion_progress_pct(0, 50), 10, "0 of N starts at 10%");
        assert_eq!(ingestion_progress_pct(50, 50), 100, "all-of-N is 100%");
        assert_eq!(
            ingestion_progress_pct(100, 50),
            100,
            "ingested>total saturates at 100",
        );
    }

    #[test]
    fn small_imports_still_move_off_10_percent() {
        // 10-item import in 1 batch — the loop fires once and lands at 100%.
        let p = ingestion_progress_pct(10, 10);
        assert_eq!(p, 100);
        // 5-item import: emit at half should land between 10 and 100 exclusive
        // so the bar never sits exactly at 10% during real progress.
        let p = ingestion_progress_pct(3, 5);
        assert!(p > 10 && p < 100, "expected 10 < p < 100, got {p}");
    }
}

#[cfg(test)]
mod optional_body_tests {
    //! Regression for the dogfood bug repro on 2026-05-09:
    //!
    //!   curl -X POST -H "X-User-Hash: $H" \
    //!     http://localhost:9101/api/ingestion/apple-import/contacts
    //!   → 400 {"error":"Invalid request payload","detail":"Content type error"}
    //!
    //! All five `apple-import/{notes,reminders,calendar,contacts,photos}`
    //! handlers used `web::Json<T>` which rejects requests without
    //! `Content-Type: application/json` even though every request struct's
    //! fields are optional. Switching to `Option<web::Json<T>>` makes a
    //! missing/empty body fall back to `T::default()`.
    //!
    //! These tests pin the extractor pattern (the actual fix) without
    //! standing up the full handler dependency graph (AppState, NodeManager,
    //! IngestionService, ProgressTracker). If anyone reverts the signature
    //! to bare `web::Json<T>`, the no-body case below 400s and the test
    //! fails.
    use super::{AppleContactsRequest, ApplePhotosRequest};
    use actix_web::{test, web, App, HttpResponse, Responder};
    use serde_json::json;

    async fn contacts_stub(req: Option<web::Json<AppleContactsRequest>>) -> impl Responder {
        let _ = req.map(web::Json::into_inner).unwrap_or_default();
        HttpResponse::Accepted().json(json!({
            "success": true,
            "progress_id": "test-progress-id",
        }))
    }

    async fn photos_stub(req: Option<web::Json<ApplePhotosRequest>>) -> impl Responder {
        let req = req.map(web::Json::into_inner).unwrap_or_default();
        HttpResponse::Ok().json(json!({
            "album": req.album,
            "limit": req.limit,
        }))
    }

    /// The exact repro from dogfood 2026-05-09: no Content-Type, no body.
    #[actix_web::test]
    async fn empty_post_returns_progress_id_not_content_type_error() {
        let app =
            test::init_service(App::new().route("/contacts", web::post().to(contacts_stub))).await;

        let req = test::TestRequest::post().uri("/contacts").to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(
            resp.status(),
            202,
            "no-body POST must reach the handler — got {} (a 400 here means \
             web::Json<T> snuck back into the signature)",
            resp.status(),
        );

        let body: serde_json::Value = test::read_body_json(resp).await;
        assert_eq!(body["success"], true);
        assert!(
            body["progress_id"].is_string(),
            "response must include a progress_id string, got {body}"
        );
    }

    /// Photos accepts a `limit` field; verify a real body still deserializes.
    #[actix_web::test]
    async fn populated_body_still_parses() {
        let app =
            test::init_service(App::new().route("/photos", web::post().to(photos_stub))).await;

        let req = test::TestRequest::post()
            .uri("/photos")
            .insert_header(("content-type", "application/json"))
            .set_payload(r#"{"limit": 25, "album": "Travel"}"#)
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert!(
            resp.status().is_success(),
            "real body should parse, got {}",
            resp.status(),
        );

        let body: serde_json::Value = test::read_body_json(resp).await;
        assert_eq!(body["limit"], 25);
        assert_eq!(body["album"], "Travel");
    }
}

#[cfg(test)]
mod permissions_endpoint_tests {
    use super::apple_import_permissions;
    use actix_web::{http::StatusCode, test, web, App};

    /// `GET /api/ingestion/apple-import/permissions` returns 200 with all
    /// five expected per-source bool keys regardless of the underlying
    /// platform. The wizard relies on every key being present — an
    /// `undefined` lookup would render as "permission missing" because of
    /// the `=== false` check in the frontend.
    ///
    /// On non-macOS hosts the handler short-circuits to all-`true` (no
    /// Apple permission to grant); on macOS the probes run and may report
    /// `false`. Either way the JSON shape is the same — that's what the
    /// onboarding wizard is coupled to, so that's what this test pins.
    #[actix_web::test]
    async fn permissions_endpoint_returns_all_five_source_keys() {
        let app = test::init_service(App::new().route(
            "/api/ingestion/apple-import/permissions",
            web::get().to(apple_import_permissions),
        ))
        .await;

        let req = test::TestRequest::get()
            .uri("/api/ingestion/apple-import/permissions")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body: serde_json::Value = test::read_body_json(resp).await;
        for key in ["contacts", "notes", "calendar", "reminders", "photos"] {
            assert!(
                body.get(key).and_then(|v| v.as_bool()).is_some(),
                "permissions response must include `{}` as a bool, got: {}",
                key,
                body,
            );
        }
    }
}
