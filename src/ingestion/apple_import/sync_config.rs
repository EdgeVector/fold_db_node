//! Configuration for automatic Apple data re-import scheduling.
//!
//! Stores schedule preferences (daily/weekly/custom interval), which sources
//! are enabled, and last/next sync timestamps. Persisted as JSON alongside
//! the node config.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// How often auto-sync should run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SyncSchedule {
    Daily,
    Weekly,
    /// Custom interval in hours.
    Custom {
        hours: u32,
    },
}

impl SyncSchedule {
    /// Return the interval as a `chrono::Duration`.
    pub fn as_duration(&self) -> Duration {
        match self {
            SyncSchedule::Daily => Duration::hours(24),
            SyncSchedule::Weekly => Duration::hours(168),
            SyncSchedule::Custom { hours } => Duration::hours(i64::from(*hours)),
        }
    }
}

/// Which Apple sources are enabled for auto-sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnabledSources {
    pub notes: bool,
    pub reminders: bool,
    pub photos: bool,
    // Accepts configs persisted before calendar auto-sync existed.
    #[serde(default = "default_calendar_enabled")]
    pub calendar: bool,
    // Accepts configs persisted before contacts auto-sync existed.
    #[serde(default = "default_contacts_enabled")]
    pub contacts: bool,
}

fn default_calendar_enabled() -> bool {
    true
}

fn default_contacts_enabled() -> bool {
    true
}

impl Default for EnabledSources {
    fn default() -> Self {
        Self {
            notes: true,
            reminders: true,
            photos: false, // photos are expensive, off by default
            calendar: true,
            contacts: true,
        }
    }
}

/// Top-level auto-sync configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppleSyncConfig {
    /// Whether auto-sync is enabled.
    pub enabled: bool,
    /// The schedule for re-imports.
    pub schedule: SyncSchedule,
    /// Which Apple sources to include.
    pub sources: EnabledSources,
    /// Photo limit per sync (only applies when photos are enabled).
    pub photos_limit: usize,
    /// Timestamp of the last completed sync (None if never synced).
    pub last_sync: Option<DateTime<Utc>>,
    /// Computed next sync time (None if disabled or never scheduled).
    pub next_sync: Option<DateTime<Utc>>,
    /// Aggregated error message from the most recent sync attempt.
    /// Cleared on any successful sync. `None` means the last attempt
    /// completed without errors (or no attempt has run yet).
    #[serde(default)]
    pub last_error: Option<String>,
    /// Timestamp of the most recent failed sync attempt.
    #[serde(default)]
    pub last_error_at: Option<DateTime<Utc>>,
}

impl Default for AppleSyncConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            schedule: SyncSchedule::Daily,
            sources: EnabledSources::default(),
            photos_limit: 50,
            last_sync: None,
            next_sync: None,
            last_error: None,
            last_error_at: None,
        }
    }
}

impl AppleSyncConfig {
    /// Recompute `next_sync` based on `last_sync` and `schedule`.
    /// If there has never been a sync, the next sync is scheduled from now.
    pub fn recompute_next_sync(&mut self) {
        if !self.enabled {
            self.next_sync = None;
            return;
        }
        let base = self.last_sync.unwrap_or_else(Utc::now);
        self.next_sync = Some(base + self.schedule.as_duration());
    }

    /// Mark a sync as completed at the given time, clear any prior error,
    /// and recompute `next_sync`.
    pub fn mark_sync_complete(&mut self, at: DateTime<Utc>) {
        self.last_sync = Some(at);
        self.last_error = None;
        self.last_error_at = None;
        self.recompute_next_sync();
    }

    /// Record a sync failure. Does NOT update `last_sync` (so a failed
    /// attempt does not mask the "last successful sync" timestamp in the
    /// UI), but DOES push `next_sync` forward by the schedule interval
    /// from the failure time so we don't hot-loop retrying a broken
    /// extractor every minute.
    pub fn mark_sync_error(&mut self, at: DateTime<Utc>, message: String) {
        self.last_error = Some(message);
        self.last_error_at = Some(at);
        if self.enabled {
            self.next_sync = Some(at + self.schedule.as_duration());
        } else {
            self.next_sync = None;
        }
    }

    /// Resolve the config file path (alongside node_config.json).
    fn config_path() -> PathBuf {
        let config_dir = std::env::var("NODE_CONFIG")
            .ok()
            .and_then(|p| Path::new(&p).parent().map(|d| d.to_path_buf()))
            .or_else(|| {
                crate::utils::paths::folddb_home()
                    .ok()
                    .map(|h| h.join("config"))
            })
            .unwrap_or_else(|| PathBuf::from("config"));
        config_dir.join("apple_sync_config.json")
    }

    /// Load from disk. Returns default config if file does not exist.
    pub fn load() -> Self {
        let path = Self::config_path();
        match std::fs::read_to_string(&path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Persist to disk.
    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_path();
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize sync config: {e}"))?;
        crate::sensitive_io::write_sensitive(&path, json.as_bytes())
            .map_err(|e| format!("Failed to write sync config: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `NODE_CONFIG` is process-wide; the atomic-write tests below mutate
    /// it to redirect `AppleSyncConfig::config_path()` at a tempdir, so
    /// they serialize on this lock to keep parallel test runs from
    /// clobbering each other.
    static NODE_CONFIG_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_default_config() {
        let cfg = AppleSyncConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.schedule, SyncSchedule::Daily);
        assert!(cfg.sources.notes);
        assert!(cfg.sources.reminders);
        assert!(!cfg.sources.photos);
        assert!(cfg.sources.calendar);
        assert!(cfg.sources.contacts);
        assert!(cfg.last_sync.is_none());
        assert!(cfg.next_sync.is_none());
    }

    #[test]
    fn test_schedule_durations() {
        assert_eq!(SyncSchedule::Daily.as_duration(), Duration::hours(24));
        assert_eq!(SyncSchedule::Weekly.as_duration(), Duration::hours(168));
        assert_eq!(
            SyncSchedule::Custom { hours: 6 }.as_duration(),
            Duration::hours(6)
        );
    }

    #[test]
    fn test_recompute_next_sync_disabled() {
        let mut cfg = AppleSyncConfig::default();
        // default is already disabled, just verify
        cfg.recompute_next_sync();
        assert!(cfg.next_sync.is_none());
    }

    #[test]
    fn test_recompute_next_sync_enabled() {
        let now = Utc::now();
        let mut cfg = AppleSyncConfig {
            enabled: true,
            schedule: SyncSchedule::Daily,
            last_sync: Some(now),
            ..AppleSyncConfig::default()
        };
        cfg.recompute_next_sync();
        let expected = now + Duration::hours(24);
        let diff = (cfg.next_sync.unwrap() - expected).num_seconds().abs();
        assert!(diff < 2, "next_sync should be ~24h after last_sync");
    }

    #[test]
    fn test_mark_sync_complete() {
        let mut cfg = AppleSyncConfig {
            enabled: true,
            schedule: SyncSchedule::Weekly,
            ..AppleSyncConfig::default()
        };
        let now = Utc::now();
        cfg.mark_sync_complete(now);
        assert_eq!(cfg.last_sync, Some(now));
        assert!(cfg.next_sync.is_some());
        let diff = (cfg.next_sync.unwrap() - (now + Duration::hours(168)))
            .num_seconds()
            .abs();
        assert!(diff < 2);
    }

    #[test]
    fn test_serialization_roundtrip() {
        let mut cfg = AppleSyncConfig {
            enabled: true,
            schedule: SyncSchedule::Custom { hours: 12 },
            sources: EnabledSources {
                photos: true,
                ..EnabledSources::default()
            },
            photos_limit: 100,
            last_sync: Some(Utc::now()),
            ..AppleSyncConfig::default()
        };
        cfg.recompute_next_sync();

        let json = serde_json::to_string(&cfg).unwrap();
        let deserialized: AppleSyncConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.enabled, cfg.enabled);
        assert_eq!(deserialized.schedule, cfg.schedule);
        assert_eq!(deserialized.sources.photos, cfg.sources.photos);
        assert_eq!(deserialized.sources.calendar, cfg.sources.calendar);
        assert_eq!(deserialized.photos_limit, cfg.photos_limit);
    }

    /// Exercise schedule-firing arithmetic for every variant: given a fixed
    /// `last_sync`, the computed `next_sync` must equal `last_sync +
    /// schedule.as_duration()`. This is the core invariant the scheduler
    /// loop relies on to decide when to fire.
    #[test]
    fn test_next_sync_equals_last_plus_interval_for_all_variants() {
        let base = Utc::now() - Duration::hours(2);
        let cases = [
            (SyncSchedule::Daily, Duration::hours(24)),
            (SyncSchedule::Weekly, Duration::hours(168)),
            (SyncSchedule::Custom { hours: 1 }, Duration::hours(1)),
            (SyncSchedule::Custom { hours: 72 }, Duration::hours(72)),
        ];

        for (schedule, interval) in cases {
            let mut cfg = AppleSyncConfig {
                enabled: true,
                schedule: schedule.clone(),
                last_sync: Some(base),
                ..AppleSyncConfig::default()
            };
            cfg.recompute_next_sync();
            assert_eq!(
                cfg.next_sync,
                Some(base + interval),
                "next_sync must equal last_sync + {:?}",
                schedule
            );
        }
    }

    /// Simulates a macOS sleep/wake scenario: the scheduler loop was
    /// paused for 5 minutes while the Mac was asleep, with `next_sync`
    /// set to a point inside that gap. After wake, `now >= next_sync`
    /// must be true — i.e. the wall-clock check catches up rather than
    /// relying on tokio timer drift. This is the regression guard for
    /// the sleep/wake requirement in M5.
    #[test]
    fn test_next_sync_fires_after_clock_skip() {
        let sleep_duration = Duration::minutes(5);
        let pre_sleep = Utc::now() - sleep_duration - Duration::minutes(1);
        let mut cfg = AppleSyncConfig {
            enabled: true,
            schedule: SyncSchedule::Custom { hours: 1 },
            last_sync: Some(pre_sleep - Duration::hours(1)),
            ..AppleSyncConfig::default()
        };
        cfg.recompute_next_sync();
        // next_sync = (pre_sleep - 1h) + 1h = pre_sleep, which is ~6min
        // in the past by "now" — a scheduler checking `Utc::now() >=
        // next_sync` must see this as fire-ready.
        let next = cfg.next_sync.expect("next_sync should be set");
        assert!(
            Utc::now() >= next,
            "post-wake wall-clock check must see next_sync as past"
        );
    }

    /// Idempotency at the schedule level: after a sync completes, the
    /// scheduler should not re-fire on the very next tick. This protects
    /// against a bug where rapid `mark_sync_complete` calls would still
    /// leave `next_sync` in the past and drive a double-ingest.
    #[test]
    fn test_mark_sync_complete_prevents_immediate_refire() {
        let mut cfg = AppleSyncConfig {
            enabled: true,
            schedule: SyncSchedule::Custom { hours: 1 },
            ..AppleSyncConfig::default()
        };
        let t0 = Utc::now();
        cfg.mark_sync_complete(t0);
        let next = cfg.next_sync.expect("next_sync set after mark_complete");

        // Second "tick" happens 1 second later — scheduler predicate is
        // `now >= next_sync`, must be false so we do not re-fire.
        let tick = t0 + Duration::seconds(1);
        assert!(
            tick < next,
            "next_sync must sit in the future right after completion"
        );
    }

    /// Error path: a failed sync must populate `last_error` /
    /// `last_error_at` AND push `next_sync` forward so the scheduler
    /// does not hot-loop on a broken extractor. `last_sync` is
    /// deliberately untouched so the UI can show "last success" vs.
    /// "last error" separately.
    #[test]
    fn test_mark_sync_error_pushes_next_and_preserves_last_sync() {
        let prior_success = Utc::now() - Duration::hours(2);
        let mut cfg = AppleSyncConfig {
            enabled: true,
            schedule: SyncSchedule::Custom { hours: 1 },
            last_sync: Some(prior_success),
            ..AppleSyncConfig::default()
        };
        cfg.recompute_next_sync();

        let fail_at = Utc::now();
        cfg.mark_sync_error(fail_at, "notes extractor permission denied".into());

        assert_eq!(cfg.last_sync, Some(prior_success));
        assert_eq!(
            cfg.last_error.as_deref(),
            Some("notes extractor permission denied")
        );
        assert_eq!(cfg.last_error_at, Some(fail_at));
        let next = cfg.next_sync.expect("next_sync set after mark_error");
        let expected = fail_at + Duration::hours(1);
        assert!((next - expected).num_seconds().abs() < 2);
    }

    /// A successful sync after a failure must clear `last_error` so
    /// the UI indicator goes green. Regression test for the
    /// no-stale-error invariant.
    #[test]
    fn test_mark_sync_complete_clears_last_error() {
        let mut cfg = AppleSyncConfig {
            enabled: true,
            schedule: SyncSchedule::Daily,
            ..AppleSyncConfig::default()
        };
        cfg.mark_sync_error(Utc::now(), "transient failure".into());
        assert!(cfg.last_error.is_some());

        cfg.mark_sync_complete(Utc::now());
        assert!(cfg.last_error.is_none());
        assert!(cfg.last_error_at.is_none());
    }

    /// Disabled auto-sync must not arm `next_sync`, even in the error
    /// path — otherwise a disabled config would still tick the
    /// scheduler the moment it's re-enabled.
    #[test]
    fn test_mark_sync_error_respects_disabled_state() {
        let mut cfg = AppleSyncConfig {
            enabled: false,
            ..AppleSyncConfig::default()
        };
        cfg.mark_sync_error(Utc::now(), "boom".into());
        assert!(cfg.next_sync.is_none());
        assert!(cfg.last_error.is_some());
    }

    #[test]
    fn test_legacy_config_without_calendar_field_defaults_to_true() {
        // Users with a config persisted before calendar auto-sync existed
        // should pick up the new source enabled by default, not disabled.
        let legacy = r#"{
            "enabled": true,
            "schedule": "daily",
            "sources": { "notes": true, "reminders": true, "photos": false },
            "photos_limit": 50,
            "last_sync": null,
            "next_sync": null
        }"#;
        let cfg: AppleSyncConfig = serde_json::from_str(legacy).unwrap();
        assert!(cfg.sources.calendar);
    }

    #[test]
    fn test_legacy_config_without_contacts_field_defaults_to_true() {
        // Configs persisted before contacts auto-sync existed should pick up
        // the new source enabled by default — same precedent as calendar.
        let legacy = r#"{
            "enabled": true,
            "schedule": "daily",
            "sources": { "notes": true, "reminders": true, "photos": false, "calendar": true },
            "photos_limit": 50,
            "last_sync": null,
            "next_sync": null
        }"#;
        let cfg: AppleSyncConfig = serde_json::from_str(legacy).unwrap();
        assert!(cfg.sources.contacts);
    }

    /// Configs persisted before `last_error` existed must deserialize
    /// cleanly with `None` rather than erroring out on missing fields.
    /// `save()` must route through the atomic-write helper: after success
    /// there must be no `<path>.tmp` sibling left on disk, and the file
    /// must parse cleanly via `load()`. Regression guard for the "plain
    /// fs::write leaves half-written JSON on power loss / OOM-kill"
    /// failure mode that the helper closes.
    #[test]
    fn save_uses_atomic_write_no_stale_tmpfile() {
        let _guard = NODE_CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("NODE_CONFIG").ok();

        let dir = tempfile::tempdir().expect("tempdir");
        let node_config = dir.path().join("node_config.json");
        std::env::set_var("NODE_CONFIG", &node_config);

        let cfg = AppleSyncConfig {
            enabled: true,
            schedule: SyncSchedule::Custom { hours: 6 },
            ..AppleSyncConfig::default()
        };
        cfg.save().expect("save");

        let path = dir.path().join("apple_sync_config.json");
        let tmp_sibling = path.with_file_name("apple_sync_config.json.tmp");
        assert!(path.exists(), "final config must exist at {path:?}");
        assert!(
            !tmp_sibling.exists(),
            "atomic write must not leave a tmpfile at {tmp_sibling:?}",
        );

        let reread = AppleSyncConfig::load();
        assert!(reread.enabled);
        assert_eq!(reread.schedule, SyncSchedule::Custom { hours: 6 });

        match prev {
            Some(v) => std::env::set_var("NODE_CONFIG", v),
            None => std::env::remove_var("NODE_CONFIG"),
        }
    }

    /// A leftover `<path>.tmp` from a previous crash must not block the
    /// next `save()` — the helper opens the tmpfile with `truncate(true)`,
    /// so a stale staged file from an OOM-kill mid-write is silently
    /// overwritten on retry.
    #[test]
    fn save_recovers_from_stale_tmpfile() {
        let _guard = NODE_CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("NODE_CONFIG").ok();

        let dir = tempfile::tempdir().expect("tempdir");
        let node_config = dir.path().join("node_config.json");
        std::env::set_var("NODE_CONFIG", &node_config);

        let final_path = dir.path().join("apple_sync_config.json");
        let tmp_sibling = final_path.with_file_name("apple_sync_config.json.tmp");
        std::fs::write(&tmp_sibling, b"garbage from a prior crash").expect("seed stale tmp");

        AppleSyncConfig::default().save().expect("save");

        assert!(final_path.exists(), "final config must exist");
        assert!(
            !tmp_sibling.exists(),
            "stale tmpfile must be replaced by the rename",
        );
        // Sanity: contents are real JSON, not the "garbage" bytes from the stale tmp.
        let on_disk = std::fs::read_to_string(&final_path).expect("read");
        let _: AppleSyncConfig = serde_json::from_str(&on_disk).expect("parse");

        match prev {
            Some(v) => std::env::set_var("NODE_CONFIG", v),
            None => std::env::remove_var("NODE_CONFIG"),
        }
    }

    /// `save()` routes through `sensitive_io::write_atomic_0600`, so the
    /// persisted `apple_sync_config.json` must end up owner-only on Unix.
    /// Guards against the callsite drifting back to a umask-default helper
    /// that would leave the user's sync settings group/world-readable.
    #[cfg(unix)]
    #[test]
    fn save_writes_sync_config_with_0600_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = NODE_CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("NODE_CONFIG").ok();

        let dir = tempfile::tempdir().expect("tempdir");
        let node_config = dir.path().join("node_config.json");
        std::env::set_var("NODE_CONFIG", &node_config);

        AppleSyncConfig::default().save().expect("save");

        let path = dir.path().join("apple_sync_config.json");
        let mode = std::fs::metadata(&path)
            .expect("stat apple_sync_config")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "expected 0o600, got {mode:o}");

        match prev {
            Some(v) => std::env::set_var("NODE_CONFIG", v),
            None => std::env::remove_var("NODE_CONFIG"),
        }
    }

    #[test]
    fn test_legacy_config_without_last_error_fields() {
        let legacy = r#"{
            "enabled": true,
            "schedule": "daily",
            "sources": { "notes": true, "reminders": true, "photos": false, "calendar": true },
            "photos_limit": 50,
            "last_sync": null,
            "next_sync": null
        }"#;
        let cfg: AppleSyncConfig = serde_json::from_str(legacy).unwrap();
        assert!(cfg.last_error.is_none());
        assert!(cfg.last_error_at.is_none());
    }
}
