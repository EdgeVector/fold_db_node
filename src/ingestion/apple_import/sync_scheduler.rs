//! Background scheduler that re-imports enabled Apple sources on a timer.
//!
//! The scheduler runs as a `tokio::spawn`-ed task. It checks once per minute
//! whether the next sync time has been reached and, if so, triggers imports
//! for all enabled sources. Content-hash dedup in the ingestion pipeline
//! ensures unchanged items are skipped.

use chrono::{DateTime, Utc};
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

/// Result of one run of [`run_sync`].
///
/// Each per-source failure is captured as a `"source: message"` string so the
/// UI and persisted config can surface them verbatim instead of burying the
/// error in a log line no one reads.
#[derive(Debug, Default, Clone)]
pub struct SyncOutcome {
    pub errors: Vec<String>,
}

impl SyncOutcome {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// Join per-source errors into a single user-facing summary.
    pub fn summary(&self) -> Option<String> {
        if self.errors.is_empty() {
            None
        } else {
            Some(self.errors.join("; "))
        }
    }

    fn push(&mut self, source: &str, err: impl std::fmt::Display) {
        self.errors.push(format!("{source}: {err}"));
    }
}

/// Decision for one scheduler tick.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TickDecision {
    /// Sync is disabled or not yet due — do nothing.
    Skip,
    /// Due and no concurrent sync in progress — run now.
    Run,
    /// Due but a previous sync is still running — skip this tick to prevent
    /// re-entry. Guards against double-ingest if a single sync outruns the
    /// tick interval (photos + LLM can easily do that).
    AlreadyRunning,
}

/// Decide what the scheduler should do on a single tick.
///
/// Pure function so the tick loop can be driven deterministically in tests
/// (simulating real clocks, macOS sleep/wake gaps, long-running syncs).
pub fn decide_tick(config: &AppleSyncConfig, now: DateTime<Utc>, is_running: bool) -> TickDecision {
    if !config.is_due(now) {
        return TickDecision::Skip;
    }
    if is_running {
        return TickDecision::AlreadyRunning;
    }
    TickDecision::Run
}

/// Execute all enabled Apple-source imports once.
///
/// Framework-agnostic — the HTTP layer's scheduler loop calls this after
/// resolving `node_arc` / `service` / `tracker` for the current user.
///
/// Returns a [`SyncOutcome`] aggregating per-source errors so the caller can
/// persist a visible error state.
pub async fn run_sync(
    sources: &super::sync_config::EnabledSources,
    photos_limit: usize,
    user_id: &str,
    node_arc: Arc<crate::fold_node::FoldNode>,
    service: Arc<IngestionService>,
    tracker: ProgressTracker,
) -> SyncOutcome {
    let mut outcome = SyncOutcome::default();

    if sources.notes {
        log_feature!(
            LogFeature::Ingestion,
            info,
            "Apple auto-sync: importing notes"
        );
        if let Err(e) =
            sync_notes(user_id, node_arc.clone(), service.clone(), tracker.clone()).await
        {
            outcome.push("notes", e);
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
            outcome.push("reminders", e);
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
        )
        .await
        {
            outcome.push("photos", e);
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
            outcome.push("calendar", e);
        }
    }

    outcome
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
                warn,
                "Auto-sync notes extract failed: {}",
                e
            );
            return Err(format!("extract failed: {e}"));
        }
        Err(e) => {
            log_feature!(
                LogFeature::Ingestion,
                warn,
                "Auto-sync notes task panicked: {}",
                e
            );
            return Err(format!("extract task panicked: {e}"));
        }
    };

    if notes.is_empty() {
        return Ok(());
    }

    let records = notes::to_json_records(&notes);
    let node = node_arc.as_ref();
    let uid = user_id.to_string();
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
                warn,
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
            "{} batch error(s); first: {}",
            batch_errors.len(),
            batch_errors[0]
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
                warn,
                "Auto-sync reminders extract failed: {}",
                e
            );
            return Err(format!("extract failed: {e}"));
        }
        Err(e) => {
            log_feature!(
                LogFeature::Ingestion,
                warn,
                "Auto-sync reminders task panicked: {}",
                e
            );
            return Err(format!("extract task panicked: {e}"));
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

    match crate::handlers::ingestion::process_json(request, user_id, &tracker, node, service).await
    {
        Ok(_) => Ok(()),
        Err(e) => {
            log_feature!(
                LogFeature::Ingestion,
                warn,
                "Auto-sync reminders error: {}",
                e
            );
            Err(e.to_string())
        }
    }
}

#[cfg(target_os = "macos")]
async fn sync_photos(
    user_id: &str,
    node_arc: Arc<crate::fold_node::FoldNode>,
    service: Arc<IngestionService>,
    tracker: ProgressTracker,
    limit: usize,
) -> Result<(), String> {
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
            return Err(format!("export failed: {e}"));
        }
        Err(e) => {
            log_feature!(
                LogFeature::Ingestion,
                warn,
                "Auto-sync photos task panicked: {}",
                e
            );
            return Err(format!("export task panicked: {e}"));
        }
    };

    if paths.is_empty() {
        return Ok(());
    }

    let node = node_arc.as_ref();
    let mut batch_errors: Vec<String> = Vec::new();

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
                    node,
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
                    batch_errors.push(format!("{file_name}: {e}"));
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
                batch_errors.push(format!("convert {}: {e}", path.display()));
            }
        }
    }

    if batch_errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} photo error(s); first: {}",
            batch_errors.len(),
            batch_errors[0]
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
                warn,
                "Auto-sync calendar extract failed: {}",
                e
            );
            return Err(format!("extract failed: {e}"));
        }
        Err(e) => {
            log_feature!(
                LogFeature::Ingestion,
                warn,
                "Auto-sync calendar task panicked: {}",
                e
            );
            return Err(format!("extract task panicked: {e}"));
        }
    };

    if events.is_empty() {
        return Ok(());
    }

    let records = cal::to_json_records(&events);
    let node = node_arc.as_ref();
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
                warn,
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
            "{} batch error(s); first: {}",
            batch_errors.len(),
            batch_errors[0]
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
    Err("not available on this platform".to_string())
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
    Err("not available on this platform".to_string())
}

#[cfg(not(target_os = "macos"))]
async fn sync_photos(
    _user_id: &str,
    _node_arc: Arc<crate::fold_node::FoldNode>,
    _service: Arc<IngestionService>,
    _tracker: ProgressTracker,
    _limit: usize,
) -> Result<(), String> {
    log_feature!(
        LogFeature::Ingestion,
        warn,
        "Auto-sync photos: not available on this platform"
    );
    Err("not available on this platform".to_string())
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
    Err("not available on this platform".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingestion::apple_import::sync_config::{
        AppleSyncConfig, EnabledSources, SyncSchedule,
    };
    use chrono::Duration;

    fn enabled_cfg_with_next(next: DateTime<Utc>) -> AppleSyncConfig {
        AppleSyncConfig {
            enabled: true,
            schedule: SyncSchedule::Custom { hours: 6 },
            next_sync: Some(next),
            sources: EnabledSources {
                notes: true,
                reminders: false,
                photos: false,
                calendar: false,
            },
            ..AppleSyncConfig::default()
        }
    }

    #[test]
    fn decide_tick_skips_when_disabled() {
        let now = Utc::now();
        let cfg = AppleSyncConfig {
            enabled: false,
            next_sync: Some(now - Duration::hours(1)),
            ..AppleSyncConfig::default()
        };
        assert_eq!(decide_tick(&cfg, now, false), TickDecision::Skip);
    }

    #[test]
    fn decide_tick_skips_before_due() {
        let now = Utc::now();
        let cfg = enabled_cfg_with_next(now + Duration::minutes(30));
        assert_eq!(decide_tick(&cfg, now, false), TickDecision::Skip);
    }

    #[test]
    fn decide_tick_runs_when_due_and_idle() {
        let now = Utc::now();
        let cfg = enabled_cfg_with_next(now - Duration::seconds(1));
        assert_eq!(decide_tick(&cfg, now, false), TickDecision::Run);
    }

    #[test]
    fn decide_tick_already_running_prevents_double_ingest() {
        let now = Utc::now();
        let cfg = enabled_cfg_with_next(now - Duration::seconds(1));
        assert_eq!(decide_tick(&cfg, now, true), TickDecision::AlreadyRunning);
    }

    /// Deterministic simulation of the scheduler tick loop over 24 hours on a
    /// 6-hour cadence, including a macOS sleep/wake gap and a long-running
    /// sync that outruns the 60-second tick interval.
    ///
    /// Asserts:
    ///   - Exactly one `Run` decision per scheduled window (no double-runs).
    ///   - After a sleep gap that spans multiple windows, only ONE catch-up
    ///     run fires on wake (mark_sync_complete advances next_sync past the
    ///     remaining ticks).
    ///   - While a sync is in flight, subsequent ticks return `AlreadyRunning`
    ///     rather than spawning concurrent runs.
    #[test]
    fn simulates_24h_with_sleep_wake_and_long_sync() {
        let t0: DateTime<Utc> = "2026-04-19T00:00:00Z".parse().unwrap();
        let mut cfg = AppleSyncConfig {
            enabled: true,
            schedule: SyncSchedule::Custom { hours: 6 },
            ..AppleSyncConfig::default()
        };
        cfg.mark_sync_complete(t0);
        // next_sync is now t0 + 6h = 06:00

        // Simulated sync duration (3 minutes) — exceeds the 60s tick interval
        // so subsequent ticks must return `AlreadyRunning`.
        let sync_duration = Duration::minutes(3);

        // Simulate a macOS sleep window: 09:00–17:00. No ticks fire during sleep.
        let sleep_start = t0 + Duration::hours(9);
        let sleep_end = t0 + Duration::hours(17);

        let mut is_running = false;
        let mut running_until: Option<DateTime<Utc>> = None;
        let mut run_count = 0usize;
        let mut already_running_count = 0usize;
        let mut run_times: Vec<DateTime<Utc>> = Vec::new();

        // 24 hours of 60-second ticks.
        for minute in 0..(24 * 60) {
            let now = t0 + Duration::minutes(minute as i64);

            // macOS is asleep — tokio task does not fire ticks during sleep.
            if now >= sleep_start && now < sleep_end {
                continue;
            }

            // If a running sync has finished, clear the guard and mark complete.
            if let Some(done_at) = running_until {
                if now >= done_at {
                    is_running = false;
                    running_until = None;
                    cfg.mark_sync_complete(done_at);
                }
            }

            match decide_tick(&cfg, now, is_running) {
                TickDecision::Skip => {}
                TickDecision::Run => {
                    run_count += 1;
                    run_times.push(now);
                    is_running = true;
                    running_until = Some(now + sync_duration);
                }
                TickDecision::AlreadyRunning => {
                    already_running_count += 1;
                }
            }
        }

        // On a 6h cadence without sleep, 4 runs would fire in 24h (at 6,12,18,24).
        // With the 09:00–17:00 sleep window, the 12:00 run is missed and the
        // 18:00 run fires as a catch-up on wake. Expected runs: ~3–4.
        assert!(
            (3..=4).contains(&run_count),
            "expected 3 or 4 runs in 24h with sleep gap, got {run_count} at {run_times:?}"
        );

        // Each `Run` triggers a 3-minute in-flight window during which the
        // next two 60-second ticks should return `AlreadyRunning`. With 3–4
        // runs that's 6–8 expected `AlreadyRunning` observations, which
        // proves the re-entry guard actually fires.
        assert!(
            already_running_count >= 2 * run_count,
            "expected at least {} AlreadyRunning observations, got {already_running_count}",
            2 * run_count
        );

        // Catch-up after wake: the first post-sleep run must fire at or after
        // sleep_end, never during sleep.
        let post_sleep_runs: Vec<_> = run_times.iter().filter(|t| **t >= sleep_end).collect();
        assert!(
            !post_sleep_runs.is_empty(),
            "sleep/wake catch-up never fired; runs: {run_times:?}"
        );

        // Back-to-back runs must be separated by at least the schedule interval
        // minus the sync duration (to prove mark_sync_complete pushed next_sync
        // forward and prevented a 60s double-run).
        let min_gap = Duration::hours(6) - Duration::minutes(5);
        for w in run_times.windows(2) {
            let gap = w[1] - w[0];
            assert!(
                gap >= min_gap,
                "runs too close together: {} and {} (gap {gap})",
                w[0],
                w[1],
            );
        }
    }

    /// Regression guard: if a sync is still running when its own next_sync
    /// elapses (pathological case — sync takes > 6h), the scheduler must not
    /// spawn a parallel run.
    #[test]
    fn never_double_runs_even_if_sync_outruns_next_interval() {
        let t0: DateTime<Utc> = "2026-04-19T00:00:00Z".parse().unwrap();
        let cfg = AppleSyncConfig {
            enabled: true,
            schedule: SyncSchedule::Custom { hours: 6 },
            next_sync: Some(t0),
            ..AppleSyncConfig::default()
        };
        // Sync started at t0, still running 10 hours later — well past next_sync.
        let now_10h_later = t0 + Duration::hours(10);
        let d = decide_tick(&cfg, now_10h_later, /* is_running = */ true);
        assert_eq!(d, TickDecision::AlreadyRunning);
    }

    #[test]
    fn sync_outcome_aggregates_errors() {
        let mut outcome = SyncOutcome::default();
        assert!(outcome.is_ok());
        assert_eq!(outcome.summary(), None);

        outcome.push("notes", "extract failed: foo");
        outcome.push("calendar", "timeout");
        assert!(!outcome.is_ok());
        let summary = outcome.summary().unwrap();
        assert!(summary.contains("notes: extract failed: foo"));
        assert!(summary.contains("calendar: timeout"));
        assert!(summary.contains(";"));
    }
}
