//! Adapter between `schema_service_core::types::Trigger` (the canonical
//! trigger shape stored + validated by the schema service) and
//! `fold_db::triggers::types::Trigger` (the interval-based shape the
//! execution-layer `TriggerRunner` actually understands).
//!
//! ## Why this lives here
//!
//! `fold_db` is a public repo; `schema_service` is private. `fold_db`
//! cannot depend on `schema_service_core` without breaking its CI clone
//! path. `fold_db_node` already depends on both, so it is the right
//! place to bridge the two types. See preferences/fold_db_vs_fold_db_node_boundary.
//!
//! ## Lossy translation
//!
//! This adapter is **LOSSY**. Canonical triggers declared against a cron
//! schedule with timezone + DST handling are approximated as fixed-interval
//! execution. For the initial release, this means:
//! - DST transitions: the actual fire cadence may drift ±1 hour twice a year.
//! - Cron expressions with irregular intervals (e.g. `"0 9,17 * * *"`) are
//!   approximated by the mean interval; actual fires happen at cron-correct
//!   times only when fold_db's Trigger type supports cron natively.
//! - `window` and `skip_if_idle` canonical fields are currently dropped —
//!   fold_db's Trigger has no equivalent shape. `ScheduledIfDirty` is the
//!   closest analogue for `skip_if_idle=true` semantics at the canonical→exec
//!   seam.
//!
//! Follow-up work: enrich `fold_db::triggers::types::Trigger` with native
//! cron support (tracked as `projects/fold-db-native-cron-support` in gbrain)
//! so this adapter becomes full-fidelity.

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use croner::Cron;
use fold_db::triggers::types::Trigger as ExecTrigger;
use schema_service_core::types::Trigger as CanonicalTrigger;
use thiserror::Error;

/// Errors returned when a canonical trigger cannot be adapted to an exec
/// trigger. `InvalidCron` and `InvalidTimezone` should never escape into
/// production: `schema_service`'s `add_view` rejects malformed cron /
/// timezone at write-time. They remain here as defense-in-depth for
/// StoredViews loaded from older stores or fuzzed inputs.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AdapterError {
    #[error("invalid cron expression: {0}")]
    InvalidCron(String),
    #[error("invalid IANA timezone: {0}")]
    InvalidTimezone(String),
    #[error("unsupported canonical trigger shape: {0}")]
    UnsupportedTriggerShape(String),
}

/// Convert a canonical `Trigger` (as persisted by `schema_service`) into
/// fold_db's exec `Trigger` plus the list of source schemas the canonical
/// shape declared.
///
/// fold_db's `Trigger` has no `schemas` field — the mutation→view index
/// is keyed off the view's `input_queries` instead. The caller usually
/// does not need the returned `Vec<String>`, but it is exposed so that
/// callers integrating with the future cron-native Trigger (or validators
/// comparing the canonical `schemas` list against `input_queries`) can
/// use it.
///
/// `now_ms` is the wall-clock anchor used to compute the mean interval
/// for cron-based triggers. Passed in (rather than read from `Utc::now()`
/// inside the adapter) so unit tests are deterministic.
pub fn canonical_to_exec(
    t: &CanonicalTrigger,
    now_ms: i64,
) -> Result<(ExecTrigger, Vec<String>), AdapterError> {
    match t {
        CanonicalTrigger::OnWrite { schemas } => Ok((ExecTrigger::OnWrite, schemas.clone())),
        CanonicalTrigger::OnWriteCoalesced {
            schemas,
            min_batch,
            debounce_ms,
            max_wait_ms,
        } => Ok((
            ExecTrigger::OnWriteCoalesced {
                min_batch: *min_batch,
                debounce_ms: *debounce_ms,
                max_wait_ms: *max_wait_ms,
            },
            schemas.clone(),
        )),
        CanonicalTrigger::Scheduled {
            cron,
            timezone,
            schemas,
            // `window` and `skip_if_idle` are dropped; fold_db's Trigger
            // has no matching fields. See module docs.
            window: _,
            skip_if_idle: _,
        } => {
            let interval_ms = compute_cron_interval_ms(cron, timezone, now_ms)?;
            Ok((ExecTrigger::Scheduled { interval_ms }, schemas.clone()))
        }
        CanonicalTrigger::ScheduledIfDirty {
            cron,
            timezone,
            schemas,
            window: _,
        } => {
            let interval_ms = compute_cron_interval_ms(cron, timezone, now_ms)?;
            Ok((
                ExecTrigger::ScheduledIfDirty { interval_ms },
                schemas.clone(),
            ))
        }
        CanonicalTrigger::Manual => Ok((ExecTrigger::Manual, Vec::new())),
    }
}

/// Compute a lossy mean interval (in ms) from a cron expression by taking
/// the arithmetic mean of the next three fire-to-fire deltas starting at
/// `now_ms`. This is the single point where the cron→interval lossiness
/// happens.
fn compute_cron_interval_ms(cron: &str, timezone: &str, now_ms: i64) -> Result<u64, AdapterError> {
    let parsed = Cron::new(cron)
        .parse()
        .map_err(|e| AdapterError::InvalidCron(format!("'{}' failed to parse: {}", cron, e)))?;
    let tz: Tz = timezone.parse().map_err(|e: chrono_tz::ParseError| {
        AdapterError::InvalidTimezone(format!("'{}': {}", timezone, e))
    })?;

    let utc_anchor: DateTime<Utc> =
        DateTime::<Utc>::from_timestamp_millis(now_ms).ok_or_else(|| {
            AdapterError::UnsupportedTriggerShape(format!(
                "now_ms {} out of chrono::DateTime range",
                now_ms
            ))
        })?;

    // Anchor on the FIRST upcoming fire, then measure 3 fire-to-fire
    // intervals. If we anchored on `now_ms` directly, the first "interval"
    // would be the partial span from now → next fire, which skews the mean
    // low (e.g. a daily cron anchored 2h before the daily fire would yield
    // a mean of ~17h instead of ~24h).
    let seed: DateTime<Tz> = utc_anchor.with_timezone(&tz);
    let mut prev = parsed.find_next_occurrence(&seed, true).map_err(|e| {
        AdapterError::InvalidCron(format!("find_next_occurrence('{}'): {}", cron, e))
    })?;

    const SAMPLE_COUNT: u32 = 3;
    let mut total_ms: i128 = 0;
    for _ in 0..SAMPLE_COUNT {
        let next = parsed.find_next_occurrence(&prev, false).map_err(|e| {
            AdapterError::InvalidCron(format!("find_next_occurrence('{}'): {}", cron, e))
        })?;
        let delta = next.signed_duration_since(prev).num_milliseconds();
        if delta <= 0 {
            return Err(AdapterError::UnsupportedTriggerShape(format!(
                "non-positive cron interval {} ms (cron='{}', tz='{}')",
                delta, cron, timezone
            )));
        }
        total_ms += delta as i128;
        prev = next;
    }

    let mean_ms = total_ms / i128::from(SAMPLE_COUNT);
    if mean_ms <= 0 || mean_ms > i128::from(i64::MAX) {
        return Err(AdapterError::UnsupportedTriggerShape(format!(
            "mean cron interval {} ms outside representable range (cron='{}')",
            mean_ms, cron
        )));
    }
    Ok(mean_ms as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wall-clock anchor used for deterministic cron tests: 2026-01-02T00:00:00Z,
    /// chosen to sit well clear of a DST boundary so the mean-interval
    /// approximations are stable.
    const DEFAULT_ANCHOR_MS: i64 = 1_767_312_000_000;

    #[test]
    fn manual_maps_to_manual_with_empty_schemas() {
        let (exec, schemas) = canonical_to_exec(&CanonicalTrigger::Manual, 0).unwrap();
        assert_eq!(exec, ExecTrigger::Manual);
        assert!(schemas.is_empty());
    }

    #[test]
    fn on_write_preserves_schemas() {
        let canonical = CanonicalTrigger::OnWrite {
            schemas: vec!["SchemaA".into(), "SchemaB".into()],
        };
        let (exec, schemas) = canonical_to_exec(&canonical, 0).unwrap();
        assert_eq!(exec, ExecTrigger::OnWrite);
        assert_eq!(schemas, vec!["SchemaA".to_string(), "SchemaB".into()]);
    }

    #[test]
    fn on_write_coalesced_field_for_field() {
        let canonical = CanonicalTrigger::OnWriteCoalesced {
            schemas: vec!["Src".into()],
            min_batch: 10,
            debounce_ms: 250,
            max_wait_ms: 5_000,
        };
        let (exec, schemas) = canonical_to_exec(&canonical, 0).unwrap();
        assert_eq!(
            exec,
            ExecTrigger::OnWriteCoalesced {
                min_batch: 10,
                debounce_ms: 250,
                max_wait_ms: 5_000,
            }
        );
        assert_eq!(schemas, vec!["Src".to_string()]);
    }

    #[test]
    fn scheduled_every_five_minutes_utc_is_five_minutes() {
        let canonical = CanonicalTrigger::Scheduled {
            cron: "*/5 * * * *".into(),
            timezone: "UTC".into(),
            window: None,
            skip_if_idle: false,
            schemas: vec!["Src".into()],
        };
        let (exec, schemas) = canonical_to_exec(&canonical, DEFAULT_ANCHOR_MS).unwrap();
        let interval_ms = match exec {
            ExecTrigger::Scheduled { interval_ms } => interval_ms,
            other => panic!("expected Scheduled, got {:?}", other),
        };
        assert_eq!(
            interval_ms, 300_000,
            "5-minute cron should approximate to 300_000 ms, got {}",
            interval_ms
        );
        assert_eq!(schemas, vec!["Src".to_string()]);
    }

    #[test]
    fn scheduled_if_dirty_daily_at_0200_is_one_day() {
        // NOTE on DST: `"0 2 * * *"` fires daily at 02:00 local. In a
        // fixed-offset zone (here UTC) the 3-sample mean is exactly
        // 86_400_000 ms. In a DST-observing zone the mean over a DST
        // transition can drift by up to 3_600_000 ms (one hour across 3
        // samples = ~1.2M ms). We test UTC to keep the assertion crisp;
        // the drift is the documented lossiness in the module docs.
        let canonical = CanonicalTrigger::ScheduledIfDirty {
            cron: "0 2 * * *".into(),
            timezone: "UTC".into(),
            window: Some("24h".into()),
            schemas: vec!["Src".into()],
        };
        let (exec, _schemas) = canonical_to_exec(&canonical, DEFAULT_ANCHOR_MS).unwrap();
        let interval_ms = match exec {
            ExecTrigger::ScheduledIfDirty { interval_ms } => interval_ms,
            other => panic!("expected ScheduledIfDirty, got {:?}", other),
        };
        assert_eq!(
            interval_ms, 86_400_000,
            "daily cron should approximate to 86_400_000 ms, got {}",
            interval_ms
        );
    }

    #[test]
    fn scheduled_dst_zone_documented_drift() {
        // Anchor at 2026-03-07T00:00:00Z — a few days before US DST
        // spring-forward (2026-03-08 02:00 local in America/Los_Angeles).
        // The 3-sample mean spans the transition and so drifts below
        // 86_400_000 by ~one hour across 3 samples. We don't assert an
        // exact value: the test exists to document that this drift is
        // expected and to fail loudly if croner + chrono-tz semantics
        // change in an unexpected direction.
        let anchor_ms = 1_772_409_600_000; // 2026-03-07T00:00:00Z
        let canonical = CanonicalTrigger::Scheduled {
            cron: "0 2 * * *".into(),
            timezone: "America/Los_Angeles".into(),
            window: None,
            skip_if_idle: false,
            schemas: vec![],
        };
        let (exec, _) = canonical_to_exec(&canonical, anchor_ms).unwrap();
        let interval_ms = match exec {
            ExecTrigger::Scheduled { interval_ms } => interval_ms,
            other => panic!("expected Scheduled, got {:?}", other),
        };
        // Sanity: within ±1 hour of nominal daily interval. That is the
        // lossiness ceiling for a once-daily cron across DST.
        let delta = (interval_ms as i128 - 86_400_000i128).unsigned_abs();
        assert!(
            delta <= 3_600_000,
            "DST drift should be within ±1h of one day, got {} ms (delta {} ms)",
            interval_ms,
            delta
        );
    }

    #[test]
    fn invalid_cron_returns_invalid_cron() {
        let canonical = CanonicalTrigger::Scheduled {
            cron: "not a cron".into(),
            timezone: "UTC".into(),
            window: None,
            skip_if_idle: false,
            schemas: vec![],
        };
        let err = canonical_to_exec(&canonical, DEFAULT_ANCHOR_MS).unwrap_err();
        assert!(
            matches!(err, AdapterError::InvalidCron(_)),
            "expected InvalidCron, got {:?}",
            err
        );
    }

    #[test]
    fn invalid_timezone_returns_invalid_timezone() {
        let canonical = CanonicalTrigger::ScheduledIfDirty {
            cron: "*/5 * * * *".into(),
            timezone: "Not/A_Zone".into(),
            window: None,
            schemas: vec![],
        };
        let err = canonical_to_exec(&canonical, DEFAULT_ANCHOR_MS).unwrap_err();
        assert!(
            matches!(err, AdapterError::InvalidTimezone(_)),
            "expected InvalidTimezone, got {:?}",
            err
        );
    }

    #[test]
    fn now_ms_out_of_range_returns_unsupported_shape() {
        // chrono::DateTime::<Utc>::from_timestamp_millis rejects i64::MIN.
        let canonical = CanonicalTrigger::Scheduled {
            cron: "*/5 * * * *".into(),
            timezone: "UTC".into(),
            window: None,
            skip_if_idle: false,
            schemas: vec![],
        };
        let err = canonical_to_exec(&canonical, i64::MIN).unwrap_err();
        assert!(
            matches!(err, AdapterError::UnsupportedTriggerShape(_)),
            "expected UnsupportedTriggerShape, got {:?}",
            err
        );
    }
}
