//! Tests for Apple auto-sync config serialization, schedule computation, and persistence.

use fold_db_node::ingestion::apple_import::sync_config::{
    AppleSyncConfig, EnabledSources, SyncSchedule,
};

#[test]
fn test_default_config_is_disabled() {
    let cfg = AppleSyncConfig::default();
    assert!(!cfg.enabled);
    assert!(cfg.last_sync.is_none());
    assert!(cfg.next_sync.is_none());
}

#[test]
fn test_daily_schedule_computes_24h() {
    let mut cfg = AppleSyncConfig {
        enabled: true,
        schedule: SyncSchedule::Daily,
        sources: EnabledSources::default(),
        photos_limit: 50,
        last_sync: Some(chrono::Utc::now()),
        next_sync: None,
    };
    cfg.recompute_next_sync();
    let next = cfg.next_sync.unwrap();
    let expected = cfg.last_sync.unwrap() + chrono::Duration::hours(24);
    assert!((next - expected).num_seconds().abs() < 2);
}

#[test]
fn test_weekly_schedule_computes_168h() {
    let mut cfg = AppleSyncConfig {
        enabled: true,
        schedule: SyncSchedule::Weekly,
        sources: EnabledSources::default(),
        photos_limit: 50,
        last_sync: Some(chrono::Utc::now()),
        next_sync: None,
    };
    cfg.recompute_next_sync();
    let next = cfg.next_sync.unwrap();
    let expected = cfg.last_sync.unwrap() + chrono::Duration::hours(168);
    assert!((next - expected).num_seconds().abs() < 2);
}

#[test]
fn test_custom_schedule() {
    let mut cfg = AppleSyncConfig {
        enabled: true,
        schedule: SyncSchedule::Custom { hours: 6 },
        sources: EnabledSources::default(),
        photos_limit: 50,
        last_sync: Some(chrono::Utc::now()),
        next_sync: None,
    };
    cfg.recompute_next_sync();
    let next = cfg.next_sync.unwrap();
    let expected = cfg.last_sync.unwrap() + chrono::Duration::hours(6);
    assert!((next - expected).num_seconds().abs() < 2);
}

#[test]
fn test_disabled_clears_next_sync() {
    let mut cfg = AppleSyncConfig {
        enabled: false,
        schedule: SyncSchedule::Daily,
        sources: EnabledSources::default(),
        photos_limit: 50,
        last_sync: Some(chrono::Utc::now()),
        next_sync: Some(chrono::Utc::now()),
    };
    cfg.recompute_next_sync();
    assert!(cfg.next_sync.is_none());
}

#[test]
fn test_mark_sync_complete_updates_times() {
    let mut cfg = AppleSyncConfig {
        enabled: true,
        schedule: SyncSchedule::Daily,
        sources: EnabledSources::default(),
        photos_limit: 50,
        last_sync: None,
        next_sync: None,
    };
    let now = chrono::Utc::now();
    cfg.mark_sync_complete(now);
    assert_eq!(cfg.last_sync, Some(now));
    assert!(cfg.next_sync.is_some());
}

#[test]
fn test_serialization_roundtrip_daily() {
    let cfg = AppleSyncConfig {
        enabled: true,
        schedule: SyncSchedule::Daily,
        sources: EnabledSources {
            notes: true,
            reminders: false,
            photos: true,
        },
        photos_limit: 100,
        last_sync: Some(chrono::Utc::now()),
        next_sync: Some(chrono::Utc::now() + chrono::Duration::hours(24)),
    };
    let json = serde_json::to_string(&cfg).unwrap();
    let deserialized: AppleSyncConfig = serde_json::from_str(&json).unwrap();
    assert!(deserialized.enabled);
    assert_eq!(deserialized.schedule, SyncSchedule::Daily);
    assert!(deserialized.sources.notes);
    assert!(!deserialized.sources.reminders);
    assert!(deserialized.sources.photos);
    assert_eq!(deserialized.photos_limit, 100);
}

#[test]
fn test_serialization_roundtrip_custom() {
    let cfg = AppleSyncConfig {
        enabled: true,
        schedule: SyncSchedule::Custom { hours: 12 },
        sources: EnabledSources::default(),
        photos_limit: 50,
        last_sync: None,
        next_sync: None,
    };
    let json = serde_json::to_string(&cfg).unwrap();
    let deserialized: AppleSyncConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.schedule, SyncSchedule::Custom { hours: 12 });
}

#[test]
fn test_no_last_sync_schedules_from_now() {
    let before = chrono::Utc::now();
    let mut cfg = AppleSyncConfig {
        enabled: true,
        schedule: SyncSchedule::Daily,
        sources: EnabledSources::default(),
        photos_limit: 50,
        last_sync: None,
        next_sync: None,
    };
    cfg.recompute_next_sync();
    let after = chrono::Utc::now();
    let next = cfg.next_sync.unwrap();
    // next should be ~24h from now (between before+24h and after+24h)
    assert!(next >= before + chrono::Duration::hours(24));
    assert!(next <= after + chrono::Duration::hours(24));
}

#[test]
fn test_enabled_sources_default() {
    let sources = EnabledSources::default();
    assert!(sources.notes);
    assert!(sources.reminders);
    assert!(!sources.photos); // photos off by default (expensive)
}
