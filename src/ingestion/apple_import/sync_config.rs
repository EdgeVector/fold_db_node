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
}

fn default_calendar_enabled() -> bool {
    true
}

impl Default for EnabledSources {
    fn default() -> Self {
        Self {
            notes: true,
            reminders: true,
            photos: false, // photos are expensive, off by default
            calendar: true,
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
    /// Human-readable error from the most recent sync attempt (cleared on success).
    /// Surfaced to the UI so a silent background failure stays visible.
    #[serde(default)]
    pub last_error: Option<String>,
    /// Timestamp the last error was recorded.
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

    /// Mark a sync as completed at the given time and recompute next_sync.
    pub fn mark_sync_complete(&mut self, at: DateTime<Utc>) {
        self.last_sync = Some(at);
        self.recompute_next_sync();
    }

    /// Pure helper: whether a scheduled sync is due as of `now`.
    /// Does not consider re-entry — use the scheduler's `decide_tick` for the
    /// full scheduling decision.
    pub fn is_due(&self, now: DateTime<Utc>) -> bool {
        self.enabled && self.next_sync.is_some_and(|next| now >= next)
    }

    /// Record an error from the most recent sync attempt.
    pub fn record_error(&mut self, message: impl Into<String>, at: DateTime<Utc>) {
        self.last_error = Some(message.into());
        self.last_error_at = Some(at);
    }

    /// Clear any previously-recorded error after a successful sync.
    pub fn clear_error(&mut self) {
        self.last_error = None;
        self.last_error_at = None;
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
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config dir: {e}"))?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize sync config: {e}"))?;
        std::fs::write(&path, json).map_err(|e| format!("Failed to write sync config: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = AppleSyncConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.schedule, SyncSchedule::Daily);
        assert!(cfg.sources.notes);
        assert!(cfg.sources.reminders);
        assert!(!cfg.sources.photos);
        assert!(cfg.sources.calendar);
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

    #[test]
    fn is_due_false_when_disabled() {
        let cfg = AppleSyncConfig {
            enabled: false,
            next_sync: Some(Utc::now() - Duration::hours(1)),
            ..AppleSyncConfig::default()
        };
        assert!(!cfg.is_due(Utc::now()));
    }

    #[test]
    fn is_due_false_before_next_sync() {
        let now = Utc::now();
        let cfg = AppleSyncConfig {
            enabled: true,
            next_sync: Some(now + Duration::hours(1)),
            ..AppleSyncConfig::default()
        };
        assert!(!cfg.is_due(now));
    }

    #[test]
    fn is_due_true_at_or_past_next_sync() {
        let now = Utc::now();
        let cfg = AppleSyncConfig {
            enabled: true,
            next_sync: Some(now - Duration::seconds(1)),
            ..AppleSyncConfig::default()
        };
        assert!(cfg.is_due(now));
    }

    #[test]
    fn record_and_clear_error() {
        let mut cfg = AppleSyncConfig::default();
        let at = Utc::now();
        cfg.record_error("boom", at);
        assert_eq!(cfg.last_error.as_deref(), Some("boom"));
        assert_eq!(cfg.last_error_at, Some(at));
        cfg.clear_error();
        assert!(cfg.last_error.is_none());
        assert!(cfg.last_error_at.is_none());
    }

    #[test]
    fn test_legacy_config_without_error_fields_deserializes() {
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
}
