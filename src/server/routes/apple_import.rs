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
use crate::ingestion::progress::IngestionStep;
use crate::ingestion::service_state::IngestionServiceState;
#[cfg(target_os = "macos")]
use crate::ingestion::IngestionRequest;
use crate::server::http_server::AppState;
use crate::server::routes::common::require_node;

/// Stamp `step` into the job's metadata so `IngestionProgress::From<Job>`
/// surfaces `current_step` correctly instead of falling back to
/// `IngestionStep::ValidatingConfig` (the source of the "Apple import looks
/// frozen at 5%" dogfood bug observed 2026-05-11). Every save site in this
/// module must call this before `tracker.save(&job).await` — otherwise
/// `current_step` stays stuck on the default even as `progress_percentage`
/// and `message` move.
fn set_step(job: &mut Job, step: IngestionStep) {
    job.metadata = json!({ "step": step });
}

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
    set_step(&mut job, IngestionStep::ValidatingConfig);
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

/// Mark an Apple-import job as unavailable on non-macOS platforms.
///
/// Body shared by every `run_apple_*_import` non-macOS stub — the only
/// per-kind variation is the `JobType` label.
#[cfg(not(target_os = "macos"))]
async fn mark_apple_import_unavailable_on_non_macos(
    progress_id: String,
    tracker: ProgressTracker,
    job_kind: &'static str,
) {
    let mut job = Job::new(progress_id, JobType::Other(job_kind.into()));
    mark_failed(&mut job, "Apple import is only available on macOS".into());
    set_step(&mut job, IngestionStep::Failed);
    mark_terminal(&mut job);
    let _ = tracker.save(&job).await;
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
    set_step(&mut job, IngestionStep::ExecutingMutations);
    let _ = tracker.save(&job).await;
}

/// Run `work` while emitting a 2s heartbeat that re-saves the same 5%-progress
/// job with `(Ns)` appended so a polling client sees the import is alive.
///
/// Each Apple data extractor runs `osascript` end-to-end inside a
/// `tokio::task::spawn_blocking`. That call returns nothing until the whole
/// library has been pulled (5+ minutes for 100+ records, capped at the
/// `OSASCRIPT_TIMEOUT`), so without a heartbeat the UI is stuck on the
/// initial "5% — Extracting…" frame for the full extraction wallclock.
///
/// The percentage stays at 5; only `message` and `updated_at` change so the
/// UI keeps the "still in extraction" semantic but sees fresh activity.
/// The 2s cadence keeps tracker save load negligible (~150 saves across the
/// 5-minute timeout ceiling).
#[cfg(any(target_os = "macos", test))]
async fn with_extraction_heartbeat<F, T>(
    tracker: &ProgressTracker,
    progress_id: &str,
    job_kind: &str,
    base_message: &str,
    work: F,
) -> T
where
    F: std::future::Future<Output = T>,
{
    let started = tokio::time::Instant::now();
    let pid = progress_id.to_string();
    let kind = job_kind.to_string();
    let msg = base_message.to_string();
    let tracker_clone = tracker.clone();

    let heartbeat = tokio::spawn(
        async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(2));
            // `interval` fires immediately on the first `tick().await`; skip
            // that one so the first heartbeat lands at +2s, not +0s with an
            // "(0s)" suffix that's noisier than helpful.
            tick.tick().await;
            loop {
                tick.tick().await;
                let elapsed = started.elapsed().as_secs();
                let mut job = Job::new(pid.clone(), JobType::Other(kind.clone()));
                job.status = JobStatus::Running;
                job.progress_percentage = 5;
                job.message = format!("{} ({}s)", msg, elapsed);
                set_step(&mut job, IngestionStep::FlatteningData);
                let _ = tracker_clone.save(&job).await;
            }
        }
        .instrument(tracing::Span::current()),
    );

    let result = work.await;
    heartbeat.abort();
    result
}

/// Per-source configuration for [`run_record_batch_import`].
///
/// Notes / Reminders / Calendar / Contacts share the same shape:
/// extract via `osascript` on a blocking thread (wrapped in
/// [`with_extraction_heartbeat`]), emit a `Running 10%` "Extracted N
/// {label}, ingesting..." job, feed records into the ingestion pipeline in
/// batches of 10, then emit a terminal job. The per-source variations are
/// nouns in messages, the canonical schema name pinned via
/// `forced_schema_descriptive_name`, and (for Reminders) whether to fail
/// the job on a partial-batch error.
#[cfg(target_os = "macos")]
struct BatchImportConfig {
    /// `JobType::Other(job_kind.into())` value. e.g. `"apple-notes"`.
    job_kind: &'static str,
    /// Display name of the Apple app. Drives both
    /// `forced_schema_descriptive_name` (so 132 records don't fragment
    /// across 3+ schemas via LLM non-determinism — see PR #946) and the
    /// `tracing::warn!` prefix on a failed batch. e.g. `"Apple Notes"`.
    app_name: &'static str,
    /// Base extraction message — passed to [`with_extraction_heartbeat`]
    /// as the prefix it appends `(Ns)` to. Should match the
    /// `initial_message` passed to [`init_apple_import_job`] in the route
    /// handler.
    base_message: &'static str,
    /// Noun used in mid-progress messages: "Extracted N {progress_label},
    /// ingesting..." and the per-batch label fed to
    /// [`emit_batch_progress`]. Calendar is the odd one out: progress
    /// messages say "events" while the terminal/empty/extract-failed
    /// messages say "calendar events".
    progress_label: &'static str,
    /// Noun used in "Failed to extract {terminal_label}: ...", "No
    /// {terminal_label} found", and "Imported N {terminal_label}".
    terminal_label: &'static str,
    /// Per-batch-error policy for the ingestion loop.
    error_policy: BatchErrorPolicy,
}

/// Per-batch-error policy for [`run_record_batch_import`].
#[cfg(target_os = "macos")]
enum BatchErrorPolicy {
    /// Notes / Calendar / Contacts: each failed batch is `tracing::warn!`-ed
    /// and skipped, then the job is marked `Completed` with the running
    /// `ingested` count. Partial success is the expected shape.
    LogAndContinue,
    /// Reminders: same per-batch warn-log, but the first error is captured
    /// and the terminal job goes through [`build_reminders_final_job`] so a
    /// total failure surfaces as `Failed` with `error_message` populated
    /// (PR #970), rather than a green `Imported 0 reminders` checkmark.
    LogAndCaptureFirstError,
}

#[cfg(target_os = "macos")]
const APPLE_NOTES_IMPORT_CFG: BatchImportConfig = BatchImportConfig {
    job_kind: "apple-notes",
    app_name: "Apple Notes",
    base_message: "Extracting notes from Apple Notes...",
    progress_label: "notes",
    terminal_label: "notes",
    error_policy: BatchErrorPolicy::LogAndContinue,
};

#[cfg(target_os = "macos")]
const APPLE_REMINDERS_IMPORT_CFG: BatchImportConfig = BatchImportConfig {
    job_kind: "apple-reminders",
    app_name: "Apple Reminders",
    base_message: "Extracting reminders...",
    progress_label: "reminders",
    terminal_label: "reminders",
    error_policy: BatchErrorPolicy::LogAndCaptureFirstError,
};

#[cfg(target_os = "macos")]
const APPLE_CALENDAR_IMPORT_CFG: BatchImportConfig = BatchImportConfig {
    job_kind: "apple-calendar",
    app_name: "Apple Calendar",
    base_message: "Extracting events from Apple Calendar...",
    progress_label: "events",
    terminal_label: "calendar events",
    error_policy: BatchErrorPolicy::LogAndContinue,
};

#[cfg(target_os = "macos")]
const APPLE_CONTACTS_IMPORT_CFG: BatchImportConfig = BatchImportConfig {
    job_kind: "apple-contacts",
    app_name: "Apple Contacts",
    base_message: "Extracting contacts from Apple Contacts...",
    progress_label: "contacts",
    terminal_label: "contacts",
    error_policy: BatchErrorPolicy::LogAndContinue,
};

/// Generic record-batch import driver shared by Notes / Reminders /
/// Calendar / Contacts.
///
/// Replaces ~115 lines of identical scaffolding per source: the
/// heartbeat-wrapped extract on a blocking thread, the three-armed
/// extract-error match, the empty-result early-return, the post-extract
/// `Running 10%` job, the chunked ingest loop with `forced_schema_descriptive_name`
/// pinned to `cfg.app_name` and per-batch progress emission, and the
/// terminal `Completed` (or Reminders `Failed`) job.
///
/// `extract` runs on a blocking thread (osascript is blocking) inside
/// [`with_extraction_heartbeat`] so the UI sees `(Ns)` ticks instead of
/// hanging at 5%. `to_json` converts the typed records into the
/// `serde_json::Value` array the ingestion pipeline expects.
///
/// Photos uses a different shape entirely (file-by-file with
/// content-addressed storage, image enrichment, and visibility
/// classification) and stays separate.
#[cfg(target_os = "macos")]
fn truncate_to_limit<T>(mut v: Vec<T>, limit: Option<usize>) -> Vec<T> {
    if let Some(n) = limit {
        v.truncate(n);
    }
    v
}

#[cfg(target_os = "macos")]
async fn run_record_batch_import<T, E, J, ExtractErr>(
    progress_id: String,
    tracker: ProgressTracker,
    node_arc: std::sync::Arc<crate::fold_node::FoldNode>,
    service: std::sync::Arc<IngestionService>,
    cfg: &BatchImportConfig,
    extract: E,
    to_json: J,
) where
    T: Send + 'static,
    E: FnOnce() -> Result<Vec<T>, ExtractErr> + Send + 'static,
    ExtractErr: std::fmt::Display + Send + 'static,
    J: FnOnce(&[T]) -> Vec<serde_json::Value> + Send,
{
    let extract_result = with_extraction_heartbeat(
        &tracker,
        &progress_id,
        cfg.job_kind,
        cfg.base_message,
        tokio::task::spawn_blocking(extract),
    )
    .await;

    let items = match extract_result {
        Ok(Ok(it)) => it,
        Ok(Err(e)) => {
            let mut job = Job::new(progress_id.clone(), JobType::Other(cfg.job_kind.into()));
            mark_failed(
                &mut job,
                format!("Failed to extract {}: {}", cfg.terminal_label, e),
            );
            set_step(&mut job, IngestionStep::Failed);
            mark_terminal(&mut job);
            let _ = tracker.save(&job).await;
            return;
        }
        Err(e) => {
            let mut job = Job::new(progress_id.clone(), JobType::Other(cfg.job_kind.into()));
            mark_failed(&mut job, format!("Extraction task panicked: {}", e));
            set_step(&mut job, IngestionStep::Failed);
            mark_terminal(&mut job);
            let _ = tracker.save(&job).await;
            return;
        }
    };

    if items.is_empty() {
        let mut job = Job::new(progress_id.clone(), JobType::Other(cfg.job_kind.into()));
        job.status = JobStatus::Completed;
        job.progress_percentage = 100;
        job.message = format!("No {} found", cfg.terminal_label);
        job.result = Some(json!({
            "source": cfg.job_kind,
            "total": 0,
            "ingested": 0,
        }));
        set_step(&mut job, IngestionStep::Completed);
        mark_terminal(&mut job);
        let _ = tracker.save(&job).await;
        return;
    }

    let total = items.len();
    let records = to_json(&items);

    let mut job = Job::new(progress_id.clone(), JobType::Other(cfg.job_kind.into()));
    job.status = JobStatus::Running;
    job.progress_percentage = 10;
    job.message = format!("Extracted {} {}, ingesting...", total, cfg.progress_label);
    set_step(&mut job, IngestionStep::GettingAIRecommendation);
    let _ = tracker.save(&job).await;

    let batch_size = 10;
    let mut ingested = 0usize;
    let mut ingest_error: Option<String> = None;
    let node = node_arc.as_ref();
    // run_with_user pins the task-local user id for the duration of this task,
    // so reading once outside the loop is equivalent to per-iteration reads.
    let user_id = fold_db::user_context::get_current_user_id().unwrap_or_default();

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
            forced_schema_descriptive_name: Some(cfg.app_name.to_string()),
        };

        match crate::handlers::ingestion::process_json(
            request,
            &user_id,
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
                    "{} batch {} failed: {}",
                    cfg.app_name,
                    i,
                    e,
                );
                if matches!(cfg.error_policy, BatchErrorPolicy::LogAndCaptureFirstError)
                    && ingest_error.is_none()
                {
                    ingest_error = Some(e.to_string());
                }
            }
        }

        emit_batch_progress(
            &tracker,
            &progress_id,
            cfg.job_kind,
            ingested,
            total,
            cfg.progress_label,
        )
        .await;
    }

    // Reminders routes through `build_reminders_final_job` so a total failure
    // surfaces as Failed/error_message instead of a green Completed/0 — see
    // PR #970. Notes/Calendar/Contacts treat partial success as Completed.
    let mut job = match cfg.error_policy {
        BatchErrorPolicy::LogAndContinue => {
            let mut j = Job::new(progress_id.clone(), JobType::Other(cfg.job_kind.into()));
            j.status = JobStatus::Completed;
            j.progress_percentage = 100;
            j.message = format!("Imported {} {}", ingested, cfg.terminal_label);
            j.result = Some(json!({
                "source": cfg.job_kind,
                "total": total,
                "ingested": ingested,
            }));
            set_step(&mut j, IngestionStep::Completed);
            j
        }
        BatchErrorPolicy::LogAndCaptureFirstError => {
            // Caller must use APPLE_REMINDERS_IMPORT_CFG here — the helper
            // hardcodes the "apple-reminders" job kind and tags step itself.
            build_reminders_final_job(progress_id.clone(), total, ingested, ingest_error)
        }
    };
    mark_terminal(&mut job);
    let _ = tracker.save(&job).await;
}

/// Parse an Apple-import request body.
///
/// `Option<web::Json<T>>` swallows deserialization errors — that lets a
/// missing Content-Type / empty body fall back to `T::default()` (the
/// dogfood 2026-05-09 fix) but ALSO silently drops a body with unknown
/// fields, which is how `{"limit": 5}` against `/notes` got eaten on
/// 2026-05-11. Parsing the raw bytes ourselves gives both: empty body
/// → default, malformed body / unknown field → 400 with a clear
/// `serde_json` error.
fn parse_apple_request_body<T>(bytes: &web::Bytes) -> Result<T, HttpResponse>
where
    T: Default + serde::de::DeserializeOwned,
{
    if bytes.is_empty() {
        return Ok(T::default());
    }
    serde_json::from_slice(bytes).map_err(|e| {
        HttpResponse::BadRequest().json(json!({
            "success": false,
            "error": "Invalid request payload",
            "detail": e.to_string(),
        }))
    })
}

/// Request body for `POST /api/ingestion/apple-import/notes`.
///
/// `limit` caps how many notes are ingested. When `None`, imports the
/// whole library. Set a low limit (e.g. 5) for a smoke test before
/// committing the full library — Anthropic embedding spend on a 100+
/// note library is non-trivial.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AppleNotesRequest {
    pub folder: Option<String>,
    pub limit: Option<usize>,
}

/// POST /api/ingestion/apple-import/notes
///
/// Body is optional. Callers can POST with no Content-Type and no body to take
/// the defaults (whole-library import); empty body falls back to the default
/// struct. A non-empty body with unknown fields 400s — see
/// [`parse_apple_request_body`].
pub async fn apple_import_notes(
    body: web::Bytes,
    state: web::Data<AppState>,
    ingestion_service: web::Data<IngestionServiceState>,
    progress_tracker: web::Data<ProgressTracker>,
) -> impl Responder {
    let request: AppleNotesRequest = match parse_apple_request_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
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
    let limit = request.limit;
    let pid = progress_id.clone();
    spawn_apple_import_task(user_id, progress_id, move || async move {
        run_apple_notes_import(folder, limit, pid, tracker, node_arc, service).await;
    })
}

#[cfg(target_os = "macos")]
async fn run_apple_notes_import(
    folder: Option<String>,
    limit: Option<usize>,
    progress_id: String,
    tracker: ProgressTracker,
    node_arc: std::sync::Arc<crate::fold_node::FoldNode>,
    service: std::sync::Arc<crate::ingestion::ingestion_service::IngestionService>,
) {
    use crate::ingestion::apple_import::notes;
    run_record_batch_import(
        progress_id,
        tracker,
        node_arc,
        service,
        &APPLE_NOTES_IMPORT_CFG,
        move || notes::extract(folder.as_deref()).map(|v| truncate_to_limit(v, limit)),
        notes::to_json_records,
    )
    .await;
}

#[cfg(not(target_os = "macos"))]
async fn run_apple_notes_import(
    _folder: Option<String>,
    _limit: Option<usize>,
    progress_id: String,
    tracker: ProgressTracker,
    _node_arc: std::sync::Arc<crate::fold_node::FoldNode>,
    _service: std::sync::Arc<crate::ingestion::ingestion_service::IngestionService>,
) {
    mark_apple_import_unavailable_on_non_macos(progress_id, tracker, "apple-notes").await;
}

/// Request body for `POST /api/ingestion/apple-import/reminders`.
///
/// `limit` caps how many reminders are ingested. When `None`, imports
/// the whole library. Set a low limit (e.g. 5) for a smoke test before
/// committing the full library.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AppleRemindersRequest {
    pub list: Option<String>,
    pub limit: Option<usize>,
}

/// POST /api/ingestion/apple-import/reminders
///
/// Body is optional — see [`apple_import_notes`] for the rationale.
pub async fn apple_import_reminders(
    body: web::Bytes,
    state: web::Data<AppState>,
    ingestion_service: web::Data<IngestionServiceState>,
    progress_tracker: web::Data<ProgressTracker>,
) -> impl Responder {
    let request: AppleRemindersRequest = match parse_apple_request_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
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
    let limit = request.limit;
    let pid = progress_id.clone();
    spawn_apple_import_task(user_id, progress_id, move || async move {
        run_apple_reminders_import(list, limit, pid, tracker, node_arc, service).await;
    })
}

#[cfg(target_os = "macos")]
async fn run_apple_reminders_import(
    list: Option<String>,
    limit: Option<usize>,
    progress_id: String,
    tracker: ProgressTracker,
    node_arc: std::sync::Arc<crate::fold_node::FoldNode>,
    service: std::sync::Arc<crate::ingestion::ingestion_service::IngestionService>,
) {
    use crate::ingestion::apple_import::reminders;
    run_record_batch_import(
        progress_id,
        tracker,
        node_arc,
        service,
        &APPLE_REMINDERS_IMPORT_CFG,
        move || reminders::extract(list.as_deref()).map(|v| truncate_to_limit(v, limit)),
        reminders::to_json_records,
    )
    .await;
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
        set_step(&mut job, IngestionStep::Failed);
    } else {
        job.status = JobStatus::Completed;
        job.message = format!("Imported {} reminders", ingested);
        set_step(&mut job, IngestionStep::Completed);
    }
    job.result = Some(json!({ "source": "apple-reminders", "total": total, "ingested": ingested }));
    job
}

#[cfg(not(target_os = "macos"))]
async fn run_apple_reminders_import(
    _list: Option<String>,
    _limit: Option<usize>,
    progress_id: String,
    tracker: ProgressTracker,
    _node_arc: std::sync::Arc<crate::fold_node::FoldNode>,
    _service: std::sync::Arc<crate::ingestion::ingestion_service::IngestionService>,
) {
    mark_apple_import_unavailable_on_non_macos(progress_id, tracker, "apple-reminders").await;
}

/// Request body for `POST /api/ingestion/apple-import/photos`.
///
/// `limit` caps how many photos are ingested. When `None`, defaults to
/// 50 (photos are heavier than the other Apple sources, so the default
/// stays bounded). Set a low limit (e.g. 5) for a smoke test before
/// committing the full library.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
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
    body: web::Bytes,
    state: web::Data<AppState>,
    ingestion_service: web::Data<IngestionServiceState>,
    progress_tracker: web::Data<ProgressTracker>,
    upload_storage: web::Data<fold_db::storage::UploadStorage>,
) -> impl Responder {
    let request: ApplePhotosRequest = match parse_apple_request_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
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

    let photos_result = with_extraction_heartbeat(
        &tracker,
        &progress_id,
        "apple-photos",
        "Exporting photos from Apple Photos...",
        tokio::task::spawn_blocking(move || photos::export(album.as_deref(), limit)),
    )
    .await;

    let paths = match photos_result {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            let mut job = Job::new(progress_id.clone(), JobType::Other("apple-photos".into()));
            mark_failed(&mut job, format!("Failed to export photos: {}", e));
            set_step(&mut job, IngestionStep::Failed);
            mark_terminal(&mut job);
            let _ = tracker.save(&job).await;
            return;
        }
        Err(e) => {
            let mut job = Job::new(progress_id.clone(), JobType::Other("apple-photos".into()));
            mark_failed(&mut job, format!("Export task panicked: {}", e));
            set_step(&mut job, IngestionStep::Failed);
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
        job.result = Some(json!({ "source": "apple-photos", "total": 0, "ingested": 0 }));
        set_step(&mut job, IngestionStep::Completed);
        mark_terminal(&mut job);
        let _ = tracker.save(&job).await;
        return;
    }

    let total = paths.len();
    let mut job = Job::new(progress_id.clone(), JobType::Other("apple-photos".into()));
    job.status = JobStatus::Running;
    job.progress_percentage = 30;
    job.message = format!("Exported {} photos, uploading...", total);
    set_step(&mut job, IngestionStep::GettingAIRecommendation);
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
        set_step(&mut job, IngestionStep::ExecutingMutations);
        let _ = tracker.save(&job).await;
    }

    let mut job = Job::new(progress_id.clone(), JobType::Other("apple-photos".into()));
    job.status = JobStatus::Completed;
    job.progress_percentage = 100;
    job.message = format!("Imported {} photos", ingested);
    job.result = Some(json!({ "source": "apple-photos", "total": total, "ingested": ingested }));
    set_step(&mut job, IngestionStep::Completed);
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
    mark_apple_import_unavailable_on_non_macos(progress_id, tracker, "apple-photos").await;
}

/// Request body for `POST /api/ingestion/apple-import/calendar`.
///
/// `limit` caps how many events are ingested. When `None`, imports the
/// whole library. Set a low limit (e.g. 5) for a smoke test before
/// committing the full library.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AppleCalendarRequest {
    pub calendar: Option<String>,
    pub limit: Option<usize>,
}

/// POST /api/ingestion/apple-import/calendar
///
/// Body is optional — see [`apple_import_notes`] for the rationale.
pub async fn apple_import_calendar(
    body: web::Bytes,
    state: web::Data<AppState>,
    ingestion_service: web::Data<IngestionServiceState>,
    progress_tracker: web::Data<ProgressTracker>,
) -> impl Responder {
    let request: AppleCalendarRequest = match parse_apple_request_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
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
    let limit = request.limit;
    let pid = progress_id.clone();
    spawn_apple_import_task(user_id, progress_id, move || async move {
        run_apple_calendar_import(calendar, limit, pid, tracker, node_arc, service).await;
    })
}

#[cfg(target_os = "macos")]
async fn run_apple_calendar_import(
    calendar: Option<String>,
    limit: Option<usize>,
    progress_id: String,
    tracker: ProgressTracker,
    node_arc: std::sync::Arc<crate::fold_node::FoldNode>,
    service: std::sync::Arc<crate::ingestion::ingestion_service::IngestionService>,
) {
    use crate::ingestion::apple_import::calendar as cal;
    run_record_batch_import(
        progress_id,
        tracker,
        node_arc,
        service,
        &APPLE_CALENDAR_IMPORT_CFG,
        move || cal::extract(calendar.as_deref()).map(|v| truncate_to_limit(v, limit)),
        cal::to_json_records,
    )
    .await;
}

#[cfg(not(target_os = "macos"))]
async fn run_apple_calendar_import(
    _calendar: Option<String>,
    _limit: Option<usize>,
    progress_id: String,
    tracker: ProgressTracker,
    _node_arc: std::sync::Arc<crate::fold_node::FoldNode>,
    _service: std::sync::Arc<crate::ingestion::ingestion_service::IngestionService>,
) {
    mark_apple_import_unavailable_on_non_macos(progress_id, tracker, "apple-calendar").await;
}

/// Request body for `POST /api/ingestion/apple-import/contacts`.
///
/// `limit` caps how many contacts are ingested. When `None`, imports
/// the whole library. Set a low limit (e.g. 5) for a smoke test before
/// committing the full library.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AppleContactsRequest {
    pub limit: Option<usize>,
}

/// POST /api/ingestion/apple-import/contacts
///
/// Body is optional — see [`apple_import_notes`] for the rationale.
pub async fn apple_import_contacts(
    body: web::Bytes,
    state: web::Data<AppState>,
    ingestion_service: web::Data<IngestionServiceState>,
    progress_tracker: web::Data<ProgressTracker>,
) -> impl Responder {
    let request: AppleContactsRequest = match parse_apple_request_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
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

    let limit = request.limit;
    let pid = progress_id.clone();
    spawn_apple_import_task(user_id, progress_id, move || async move {
        run_apple_contacts_import(limit, pid, tracker, node_arc, service).await;
    })
}

#[cfg(target_os = "macos")]
async fn run_apple_contacts_import(
    limit: Option<usize>,
    progress_id: String,
    tracker: ProgressTracker,
    node_arc: std::sync::Arc<crate::fold_node::FoldNode>,
    service: std::sync::Arc<crate::ingestion::ingestion_service::IngestionService>,
) {
    use crate::ingestion::apple_import::contacts as ctc;
    run_record_batch_import(
        progress_id,
        tracker,
        node_arc,
        service,
        &APPLE_CONTACTS_IMPORT_CFG,
        move || ctc::extract().map(|v| truncate_to_limit(v, limit)),
        ctc::to_json_records,
    )
    .await;
}

#[cfg(not(target_os = "macos"))]
async fn run_apple_contacts_import(
    _limit: Option<usize>,
    progress_id: String,
    tracker: ProgressTracker,
    _node_arc: std::sync::Arc<crate::fold_node::FoldNode>,
    _service: std::sync::Arc<crate::ingestion::ingestion_service::IngestionService>,
) {
    mark_apple_import_unavailable_on_non_macos(progress_id, tracker, "apple-contacts").await;
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
        assert_eq!(result["source"], "apple-reminders");
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
        assert_eq!(result["source"], "apple-reminders");
        assert_eq!(result["total"], 42);
        assert_eq!(result["ingested"], 0);
    }

    #[test]
    fn results_flow_through_to_ingestion_progress_payload() {
        // Pins the bug fix: every apple-import handler writes a structured
        // `{source, total, ingested}` JSON to `Job.result` on success, and the
        // `IngestionProgress::From<Job>` mapping must surface it on the wire
        // so the React UI doesn't fall back to parsing `status_message`. The
        // pre-fix mapping tried to deserialize `Job.result` into the typed
        // `IngestionResults` struct and silently nulled the field whenever
        // the shape didn't match the file-ingest case.
        use crate::ingestion::progress::IngestionProgress;
        let job = build_reminders_final_job("p4".into(), 7, 7, None);
        let progress: IngestionProgress = job.into();
        assert!(progress.is_complete);
        assert!(!progress.is_failed);
        let results = progress
            .results
            .expect("apple-import progress.results must be non-null on success");
        assert_eq!(results["source"], "apple-reminders");
        assert_eq!(results["total"], 7);
        assert_eq!(results["ingested"], 7);
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
mod with_extraction_heartbeat_tests {
    use super::with_extraction_heartbeat;
    use async_trait::async_trait;
    use fold_db::progress::{Job, ProgressStore, ProgressTracker};
    use std::sync::{Arc, Mutex};

    struct RecordingStore {
        saves: Mutex<Vec<Job>>,
    }

    #[async_trait]
    impl ProgressStore for RecordingStore {
        async fn save(&self, job: &Job) -> Result<(), String> {
            self.saves.lock().unwrap().push(job.clone());
            Ok(())
        }
        async fn load(&self, _id: &str) -> Result<Option<Job>, String> {
            Ok(None)
        }
        async fn list_by_user(&self, _user_id: &str) -> Result<Vec<Job>, String> {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn ticks_emit_distinct_elapsed_messages_during_long_extraction() {
        let store = Arc::new(RecordingStore {
            saves: Mutex::new(Vec::new()),
        });
        let tracker: ProgressTracker = store.clone();

        // Simulate a 5-second extraction. With a 2s tick (and the immediate
        // first fire skipped), heartbeat saves land at +2s and +4s.
        with_extraction_heartbeat(
            &tracker,
            "test-pid",
            "apple-notes",
            "Extracting notes from Apple Notes...",
            tokio::time::sleep(std::time::Duration::from_millis(5000)),
        )
        .await;

        let saves = store.saves.lock().unwrap();
        let messages: Vec<String> = saves.iter().map(|j| j.message.clone()).collect();
        let distinct: std::collections::HashSet<&String> = messages.iter().collect();
        assert!(
            distinct.len() >= 2,
            "expected >=2 distinct heartbeat messages during a 5s extraction, got: {:?}",
            messages,
        );
        for m in &messages {
            assert!(
                m.starts_with("Extracting notes from Apple Notes... ("),
                "heartbeat message should include base + elapsed marker, got: {}",
                m,
            );
            assert!(
                m.ends_with("s)"),
                "elapsed marker should end with 's)', got: {}",
                m
            );
        }
        // The whole point: percentage stays at 5 throughout — the heartbeat
        // doesn't advance progress, only refreshes the message + updated_at
        // so polling clients see the import is alive.
        for j in saves.iter() {
            assert_eq!(
                j.progress_percentage, 5,
                "heartbeat must not change progress_percentage from 5",
            );
            assert!(
                matches!(j.job_type, fold_db::progress::JobType::Other(ref s) if s == "apple-notes"),
                "heartbeat must preserve the original job_kind",
            );
        }
    }

    #[tokio::test]
    async fn no_ticks_for_subsecond_work_and_helper_returns_value() {
        // Work returns in 100ms — well under the 2s tick interval. We expect
        // zero heartbeat saves, the helper to return the work's value, and
        // no leaked task ticking afterward.
        let store = Arc::new(RecordingStore {
            saves: Mutex::new(Vec::new()),
        });
        let tracker: ProgressTracker = store.clone();

        let result = with_extraction_heartbeat(
            &tracker,
            "fast-pid",
            "apple-reminders",
            "Extracting reminders...",
            async {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                42_u32
            },
        )
        .await;
        assert_eq!(result, 42);

        // Wait past two tick windows. If the heartbeat task hadn't been
        // aborted, we'd see saves accumulating here.
        tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
        let saves_count = store.saves.lock().unwrap().len();
        assert_eq!(
            saves_count, 0,
            "expected no heartbeat saves for sub-tick work; helper must abort cleanly, got {} saves",
            saves_count,
        );
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
    use super::{
        parse_apple_request_body, AppleCalendarRequest, AppleContactsRequest, AppleNotesRequest,
        ApplePhotosRequest, AppleRemindersRequest,
    };
    use actix_web::{test, web, App, HttpResponse, Responder};
    use serde_json::json;

    async fn contacts_stub(body: web::Bytes) -> impl Responder {
        let req: AppleContactsRequest = match parse_apple_request_body(&body) {
            Ok(r) => r,
            Err(resp) => return resp,
        };
        HttpResponse::Accepted().json(json!({
            "success": true,
            "progress_id": "test-progress-id",
            "limit": req.limit,
        }))
    }

    async fn photos_stub(body: web::Bytes) -> impl Responder {
        let req: ApplePhotosRequest = match parse_apple_request_body(&body) {
            Ok(r) => r,
            Err(resp) => return resp,
        };
        HttpResponse::Ok().json(json!({
            "album": req.album,
            "limit": req.limit,
        }))
    }

    async fn notes_stub(body: web::Bytes) -> impl Responder {
        let req: AppleNotesRequest = match parse_apple_request_body(&body) {
            Ok(r) => r,
            Err(resp) => return resp,
        };
        HttpResponse::Ok().json(json!({
            "folder": req.folder,
            "limit": req.limit,
        }))
    }

    async fn reminders_stub(body: web::Bytes) -> impl Responder {
        let req: AppleRemindersRequest = match parse_apple_request_body(&body) {
            Ok(r) => r,
            Err(resp) => return resp,
        };
        HttpResponse::Ok().json(json!({
            "list": req.list,
            "limit": req.limit,
        }))
    }

    async fn calendar_stub(body: web::Bytes) -> impl Responder {
        let req: AppleCalendarRequest = match parse_apple_request_body(&body) {
            Ok(r) => r,
            Err(resp) => return resp,
        };
        HttpResponse::Ok().json(json!({
            "calendar": req.calendar,
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

    /// Dogfood 2026-05-11 regression: `{"limit": 5}` against
    /// `/apple-import/notes` was silently ignored and the full library got
    /// imported. Every Apple request struct must accept `limit` so a smoke
    /// test stays a smoke test.
    #[actix_web::test]
    async fn every_apple_request_accepts_limit_field() {
        let app = test::init_service(
            App::new()
                .route("/notes", web::post().to(notes_stub))
                .route("/reminders", web::post().to(reminders_stub))
                .route("/calendar", web::post().to(calendar_stub))
                .route("/contacts", web::post().to(contacts_stub))
                .route("/photos", web::post().to(photos_stub)),
        )
        .await;

        for path in ["/notes", "/reminders", "/calendar", "/contacts", "/photos"] {
            let req = test::TestRequest::post()
                .uri(path)
                .insert_header(("content-type", "application/json"))
                .set_payload(r#"{"limit": 5}"#)
                .to_request();
            let resp = test::call_service(&app, req).await;
            assert!(
                resp.status().is_success(),
                "{} should accept `limit`, got {}",
                path,
                resp.status(),
            );
            let body: serde_json::Value = test::read_body_json(resp).await;
            assert_eq!(
                body["limit"], 5,
                "{} echoed limit incorrectly: {body}",
                path,
            );
        }
    }

    /// `deny_unknown_fields` means a typo (`{"banana": 42}`) 400s with a
    /// clear "unknown field" message instead of silently no-op'ing — the
    /// other half of the dogfood 2026-05-11 fix.
    #[actix_web::test]
    async fn every_apple_request_rejects_unknown_fields() {
        let app = test::init_service(
            App::new()
                .route("/notes", web::post().to(notes_stub))
                .route("/reminders", web::post().to(reminders_stub))
                .route("/calendar", web::post().to(calendar_stub))
                .route("/contacts", web::post().to(contacts_stub))
                .route("/photos", web::post().to(photos_stub)),
        )
        .await;

        for path in ["/notes", "/reminders", "/calendar", "/contacts", "/photos"] {
            let req = test::TestRequest::post()
                .uri(path)
                .insert_header(("content-type", "application/json"))
                .set_payload(r#"{"banana": 42}"#)
                .to_request();
            let resp = test::call_service(&app, req).await;
            assert_eq!(
                resp.status(),
                400,
                "{} should 400 on unknown field, got {}",
                path,
                resp.status(),
            );
        }
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

/// Pin the per-source configs feeding [`run_record_batch_import`].
///
/// These tests don't exercise the helper end-to-end (that needs a real
/// `ProgressTracker` / `FoldNode` / `IngestionService`); per-source
/// behavior is covered by the `apple_import` integration tests via the
/// HTTP route handlers. What we DO want pinned here is that no future
/// edit silently drifts a label, schema name, or error policy — those
/// values are observable through the user-facing job message stream.
#[cfg(all(test, target_os = "macos"))]
mod batch_import_config_tests {
    use super::{
        BatchErrorPolicy, APPLE_CALENDAR_IMPORT_CFG, APPLE_CONTACTS_IMPORT_CFG,
        APPLE_NOTES_IMPORT_CFG, APPLE_REMINDERS_IMPORT_CFG,
    };

    #[test]
    fn calendar_uses_distinct_progress_and_terminal_labels() {
        // "Imported N calendar events" / "No calendar events found" but
        // "Extracted N events, ingesting..." — preserve the asymmetry.
        assert_eq!(APPLE_CALENDAR_IMPORT_CFG.progress_label, "events");
        assert_eq!(APPLE_CALENDAR_IMPORT_CFG.terminal_label, "calendar events");
    }

    #[test]
    fn other_sources_share_progress_and_terminal_labels() {
        for cfg in [
            &APPLE_NOTES_IMPORT_CFG,
            &APPLE_REMINDERS_IMPORT_CFG,
            &APPLE_CONTACTS_IMPORT_CFG,
        ] {
            assert_eq!(
                cfg.progress_label, cfg.terminal_label,
                "{}: progress and terminal labels should match",
                cfg.job_kind,
            );
        }
    }

    #[test]
    fn only_reminders_captures_first_ingest_error() {
        assert!(matches!(
            APPLE_REMINDERS_IMPORT_CFG.error_policy,
            BatchErrorPolicy::LogAndCaptureFirstError
        ));
        for cfg in [
            &APPLE_NOTES_IMPORT_CFG,
            &APPLE_CALENDAR_IMPORT_CFG,
            &APPLE_CONTACTS_IMPORT_CFG,
        ] {
            assert!(
                matches!(cfg.error_policy, BatchErrorPolicy::LogAndContinue),
                "{}: should use LogAndContinue policy",
                cfg.job_kind,
            );
        }
    }

    #[test]
    fn app_name_drives_forced_schema_and_warn_label() {
        // The string in `app_name` is observable in two places: as the
        // value of `forced_schema_descriptive_name` (visible in the
        // ingestion DB) and as the prefix in the `tracing::warn!` output
        // ("Apple Notes batch 3 failed: ..."). Lock both shapes.
        let pairs = [
            (&APPLE_NOTES_IMPORT_CFG, "Apple Notes"),
            (&APPLE_REMINDERS_IMPORT_CFG, "Apple Reminders"),
            (&APPLE_CALENDAR_IMPORT_CFG, "Apple Calendar"),
            (&APPLE_CONTACTS_IMPORT_CFG, "Apple Contacts"),
        ];
        for (cfg, expected) in pairs {
            assert_eq!(cfg.app_name, expected, "{}: app_name", cfg.job_kind);
        }
    }

    #[test]
    fn job_kinds_match_route_handler_strings() {
        // Drift here flips JobType::Other on the progress stream and
        // breaks any UI / poller filtering by job kind.
        assert_eq!(APPLE_NOTES_IMPORT_CFG.job_kind, "apple-notes");
        assert_eq!(APPLE_REMINDERS_IMPORT_CFG.job_kind, "apple-reminders");
        assert_eq!(APPLE_CALENDAR_IMPORT_CFG.job_kind, "apple-calendar");
        assert_eq!(APPLE_CONTACTS_IMPORT_CFG.job_kind, "apple-contacts");
    }
}

/// Regression tests for the 2026-05-11 dogfood bug: every Apple import
/// showed `current_step: ValidatingConfig` at `progress_percentage: 5`
/// for the entire 30–90s extraction window. Root cause: every save site
/// created a `Job::new(...)` without step metadata, so
/// `IngestionProgress::From<Job>` always fell back to ValidatingConfig.
#[cfg(test)]
mod step_metadata_tests {
    use super::{build_reminders_final_job, ingestion_progress_pct, set_step};
    use crate::ingestion::progress::{IngestionProgress, IngestionStep};
    use fold_db::progress::{Job, JobStatus, JobType};

    // APPLE_NOTES_IMPORT_CFG is macOS-gated, but the behavior under test
    // (step metadata round-tripping) is platform-agnostic. Hardcode the
    // job_kind string so this module compiles on Linux CI runners too.
    const NOTES_JOB_KIND: &str = "apple-notes";

    fn job_step(job: Job) -> IngestionStep {
        let progress: IngestionProgress = job.into();
        progress.current_step
    }

    fn make_job(kind: &str) -> Job {
        Job::new("p1".into(), JobType::Other(kind.into()))
    }

    #[test]
    fn set_step_round_trips_through_ingestion_progress() {
        // The core fix: `set_step` writes metadata such that
        // `IngestionProgress::From<Job>` picks it up. Without this, every
        // Apple-import save left `current_step` defaulted to ValidatingConfig.
        let mut job = make_job("apple-notes");
        set_step(&mut job, IngestionStep::ExecutingMutations);
        assert_eq!(job_step(job), IngestionStep::ExecutingMutations);
    }

    #[test]
    fn set_step_writes_every_variant_we_use_in_apple_imports() {
        // Lock the variants the apple-import save sites actually emit.
        // If any of these stop round-tripping, the From<Job> impl in
        // progress.rs has drifted and the UI would silently regress.
        for step in [
            IngestionStep::ValidatingConfig,
            IngestionStep::FlatteningData,
            IngestionStep::GettingAIRecommendation,
            IngestionStep::ExecutingMutations,
            IngestionStep::Completed,
            IngestionStep::Failed,
        ] {
            let mut job = make_job("apple-notes");
            set_step(&mut job, step.clone());
            assert_eq!(
                job_step(job),
                step,
                "step variant did not round-trip through metadata",
            );
        }
    }

    #[test]
    fn ladder_for_132_note_import_never_freezes_at_validating_config() {
        // The dogfood scenario: 132 Apple Notes, batch_size 10 → 14
        // batches. Replays the sequence of progress writes the runtime
        // would emit and asserts that — after the initial preflight save —
        // every subsequent save advances either `step` off ValidatingConfig
        // or `progress_percentage` above 5%. Before the fix, every save
        // looked frozen at (ValidatingConfig, 5%) until the very end.
        let total = 132usize;
        let batch_size = 10usize;

        let mut ladder: Vec<(IngestionStep, u8)> = Vec::new();

        // 1. init_apple_import_job
        let mut j = make_job(NOTES_JOB_KIND);
        j.status = JobStatus::Running;
        j.progress_percentage = 5;
        set_step(&mut j, IngestionStep::ValidatingConfig);
        ladder.push((job_step(j.clone()), j.progress_percentage));

        // 2. heartbeat ticks during AppleScript. Percentage stays at 5
        //    (that's the documented contract — the AppleScript export is
        //    one opaque call) but step moves to FlatteningData so the API
        //    doesn't look frozen on ValidatingConfig.
        for _ in 0..3 {
            let mut j = make_job(NOTES_JOB_KIND);
            j.status = JobStatus::Running;
            j.progress_percentage = 5;
            set_step(&mut j, IngestionStep::FlatteningData);
            ladder.push((job_step(j.clone()), j.progress_percentage));
        }

        // 3. post-extract, pre-batch
        let mut j = make_job(NOTES_JOB_KIND);
        j.status = JobStatus::Running;
        j.progress_percentage = 10;
        set_step(&mut j, IngestionStep::GettingAIRecommendation);
        ladder.push((job_step(j.clone()), j.progress_percentage));

        // 4. per-batch loop
        let total_batches = total.div_ceil(batch_size);
        for i in 0..total_batches {
            let ingested = ((i + 1) * batch_size).min(total);
            let mut j = make_job(NOTES_JOB_KIND);
            j.status = JobStatus::Running;
            j.progress_percentage = ingestion_progress_pct(ingested, total);
            set_step(&mut j, IngestionStep::ExecutingMutations);
            ladder.push((job_step(j.clone()), j.progress_percentage));
        }

        // 5. terminal Completed
        let mut j = make_job(NOTES_JOB_KIND);
        j.status = JobStatus::Completed;
        j.progress_percentage = 100;
        set_step(&mut j, IngestionStep::Completed);
        ladder.push((job_step(j.clone()), j.progress_percentage));

        // Sanity: after the first save, no entry is allowed to look
        // frozen at (ValidatingConfig, 5%). That's the exact dogfood
        // symptom and the regression this whole module guards against.
        for (idx, (step, pct)) in ladder.iter().enumerate().skip(1) {
            assert!(
                *step != IngestionStep::ValidatingConfig || *pct > 5,
                "ladder[{}] = ({:?}, {}%) — looks frozen at (ValidatingConfig, 5%)",
                idx,
                step,
                pct,
            );
        }

        // At least one mid-flight save must have a percentage strictly
        // greater than 5 — that's the bar moving past the "is the job
        // hung?" threshold the user was staring at.
        let max_mid_pct = ladder.iter().skip(1).map(|(_, p)| *p).max().unwrap_or(0);
        assert!(
            max_mid_pct > 5,
            "no save in the 132-note ladder advanced past 5%: {:?}",
            ladder,
        );

        // Percentage is non-decreasing through the whole ladder.
        for w in ladder.windows(2) {
            assert!(
                w[0].1 <= w[1].1,
                "non-monotonic progress: {:?} -> {:?}",
                w[0],
                w[1],
            );
        }

        // Terminal save is Completed at 100%.
        assert_eq!(*ladder.last().unwrap(), (IngestionStep::Completed, 100));
    }

    #[test]
    fn reminders_final_job_failure_carries_failed_step_and_error_hint() {
        // The Contacts-timeout dogfood symptom touched the failure path
        // too: even with a helpful error message, `current_step` stayed
        // on ValidatingConfig. build_reminders_final_job (and the parallel
        // mark_failed sites in other handlers) must now stamp Failed.
        let job = build_reminders_final_job(
            "p1".into(),
            42,
            0,
            Some("schema service unreachable".into()),
        );
        let progress: IngestionProgress = job.into();
        assert_eq!(progress.current_step, IngestionStep::Failed);
        assert!(progress.is_failed);
        assert!(
            progress
                .status_message
                .contains("schema service unreachable"),
            "error hint must reach status_message — got: {}",
            progress.status_message,
        );
    }

    #[test]
    fn reminders_final_job_success_carries_completed_step() {
        let job = build_reminders_final_job("p2".into(), 10, 10, None);
        let progress: IngestionProgress = job.into();
        assert_eq!(progress.current_step, IngestionStep::Completed);
        assert_eq!(progress.progress_percentage, 100);
    }
}
