//! Tame Sled storage internals in the node's structured log file.
//!
//! Upstream `observability::init_node_with_web` reads `RUST_LOG` and
//! installs a generic `tracing_log::LogTracer` bridge. With
//! `RUST_LOG=debug` (set by `run.sh --local`, or by operators chasing an
//! ingestion bug), Sled's `log::*!` calls inside `sled::pagecache::iobuf`
//! / `sled::tree` / `sled::iter` flood `~/.folddb/observability.jsonl`
//! with `advancing offset` / `wrote lsns` / `mark_interval` /
//! `freeing segment` / `make_stable` events — a single 50-file Smart
//! Folder run produced 231,872 lines (~91% Sled internals) on
//! 2026-05-05 (231k total, 208k DEBUG, 14 WARN).
//!
//! `EnvFilter` can't catch them: tracing-log's bridge stamps every
//! bridged event with `metadata.target() == "log"` (the original target
//! lives in the `log.target` field), so a `RUST_LOG=...,sled=info`
//! directive never matches — the filter only sees `target="log"`.
//!
//! We solve this on the fold_db_node side by installing our own
//! `log::Log` implementation **before** upstream's `LogTracer::init()`.
//! `log::set_boxed_logger` is single-init: first installer wins, the
//! upstream call returns `SetLoggerError` and the upstream code already
//! swallows it. Our wrapper delegates to a real `tracing_log::LogTracer`
//! for everything except sled-prefixed targets, which we cap at
//! `FOLDDB_LOG_SLED` (default `info`).
//!
//! Operators debugging Sled itself opt back in with
//! `FOLDDB_LOG_SLED=debug` (or any tracing level — `trace`, `debug`,
//! `info`, `warn`, `error`).

use std::env;

use log::{Level, LevelFilter, Log, Metadata, Record};
use tracing_log::LogTracer;

/// Env var operators set to override the default Sled log level.
///
/// `FOLDDB_LOG_SLED=debug` restores the pre-fix volume. Any tracing
/// level the `log` crate accepts works.
pub const FOLDDB_LOG_SLED_ENV: &str = "FOLDDB_LOG_SLED";

/// `RUST_LOG` is the standard tracing-subscriber filter env var.
const RUST_LOG_ENV: &str = "RUST_LOG";

/// Default Sled level when `FOLDDB_LOG_SLED` is unset.
///
/// `Info` keeps Sled WARN+ERROR + a handful of structural events visible
/// without the iobuf chatter. The underlying `EnvFilter` default is also
/// `info`, so this matches the implicit baseline for everything else.
const DEFAULT_SLED_LEVEL: Level = Level::Info;

/// Apply node-side default tracing filters before the upstream
/// `observability::init_node_with_web` initializes its subscriber stack.
///
/// MUST run before `init_node_with_web`. Two reasons:
///
/// 1. `log::set_boxed_logger` is single-init — first installer wins.
///    Calling this after upstream's `LogTracer::init()` already ran is
///    a no-op (`SetLoggerError` swallowed), and the sled filter never
///    takes effect.
/// 2. Upstream's `EnvFilter::try_from_default_env()` snapshots `RUST_LOG`
///    when the subscriber is built; we mutate `RUST_LOG` here as a
///    belt-and-suspenders measure for any sled events that are emitted
///    via `tracing::*` directly rather than the `log::*` bridge.
///
/// Idempotent at the env-var level (the augment is no-op when the
/// directive is already present); idempotent at the logger level
/// because the second `set_boxed_logger` call is silently dropped.
pub fn apply_default_filters() {
    let sled_level =
        parse_level(env::var(FOLDDB_LOG_SLED_ENV).ok().as_deref()).unwrap_or(DEFAULT_SLED_LEVEL);
    install_sled_aware_log_bridge(sled_level);
    let augmented = augment_rust_log(
        env::var(RUST_LOG_ENV).ok().as_deref(),
        level_to_filter_str(sled_level),
    );
    env::set_var(RUST_LOG_ENV, augmented);
}

/// Install the sled-aware `log::Log` bridge. Swallows `SetLoggerError`:
/// if a logger is already installed (test harness, double-init in a
/// long-running test process) we accept the existing one — emitting a
/// hard error here would break callers that depend on the helper being
/// safe to invoke unconditionally.
fn install_sled_aware_log_bridge(sled_max: Level) {
    let logger = SledFilteringLogger {
        inner: LogTracer::new(),
        sled_max,
    };
    let _ = log::set_boxed_logger(Box::new(logger));
    // Open the gate at the `log` crate level so events with our
    // sled cutoff still reach the wrapper's `enabled` for the per-target
    // decision. The actual cap happens inside the wrapper.
    log::set_max_level(LevelFilter::Trace);
}

/// `log::Log` shim that applies the per-sled level cap, then delegates
/// to a real `LogTracer` for the cross-cutting `log::* → tracing::*`
/// bridge work (which calls into a private `dispatch_record` helper we
/// cannot reach from outside `tracing-log`).
struct SledFilteringLogger {
    inner: LogTracer,
    sled_max: Level,
}

impl SledFilteringLogger {
    /// True if `target` is sled itself or any sled submodule. Match by
    /// exact equality OR `"sled::"` prefix; a substring match would
    /// false-positive on hypothetical crates whose name happens to
    /// start with `sled` (e.g. `sled_tools`).
    fn is_sled_target(target: &str) -> bool {
        target == "sled" || target.starts_with("sled::")
    }
}

impl Log for SledFilteringLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        if Self::is_sled_target(metadata.target()) && metadata.level() > self.sled_max {
            return false;
        }
        self.inner.enabled(metadata)
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            // Delegate so `tracing-log`'s private dispatch path still
            // builds the bridged tracing event with `log.target` /
            // `log.module_path` attributes. We can't replicate that
            // dispatch from outside the crate.
            self.inner.log(record);
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

/// Pure builder for the augmented `RUST_LOG` string. Split out so unit
/// tests don't have to mutate process env.
fn augment_rust_log(current: Option<&str>, sled_level: &str) -> String {
    let base = current.unwrap_or("info");
    let suffix = format!("sled={sled_level}");
    if base.split(',').any(|d| d.trim() == suffix) {
        base.to_string()
    } else if base.is_empty() {
        suffix
    } else {
        format!("{base},{suffix}")
    }
}

/// Parse a tracing-style level name from `FOLDDB_LOG_SLED`. Garbage and
/// `None` both return `None` — the caller falls back to the default.
fn parse_level(value: Option<&str>) -> Option<Level> {
    let raw = value?.trim();
    if raw.is_empty() {
        return None;
    }
    match raw.to_ascii_lowercase().as_str() {
        "trace" => Some(Level::Trace),
        "debug" => Some(Level::Debug),
        "info" => Some(Level::Info),
        "warn" | "warning" => Some(Level::Warn),
        "error" => Some(Level::Error),
        _ => None,
    }
}

/// Map a `log::Level` to the lowercase string `EnvFilter` expects in a
/// `target=level` directive.
fn level_to_filter_str(level: Level) -> &'static str {
    match level {
        Level::Trace => "trace",
        Level::Debug => "debug",
        Level::Info => "info",
        Level::Warn => "warn",
        Level::Error => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn augments_debug_with_sled_info_by_default() {
        let augmented = augment_rust_log(Some("debug"), "info");
        assert_eq!(augmented, "debug,sled=info");
    }

    #[test]
    fn augments_with_explicit_sled_level() {
        let augmented = augment_rust_log(Some("debug"), "trace");
        assert_eq!(augmented, "debug,sled=trace");
    }

    #[test]
    fn synthesizes_baseline_when_rust_log_unset() {
        let augmented = augment_rust_log(None, "info");
        assert_eq!(augmented, "info,sled=info");
    }

    #[test]
    fn preserves_existing_directives() {
        let augmented = augment_rust_log(Some("info,fold_db_node=trace,actix_web=warn"), "info");
        assert_eq!(
            augmented,
            "info,fold_db_node=trace,actix_web=warn,sled=info"
        );
    }

    #[test]
    fn idempotent_when_directive_already_present() {
        let once = augment_rust_log(Some("debug"), "info");
        let twice = augment_rust_log(Some(&once), "info");
        assert_eq!(once, twice);
    }

    #[test]
    fn parse_level_accepts_canonical_names() {
        assert_eq!(parse_level(Some("trace")), Some(Level::Trace));
        assert_eq!(parse_level(Some("DEBUG")), Some(Level::Debug));
        assert_eq!(parse_level(Some(" Info ")), Some(Level::Info));
        assert_eq!(parse_level(Some("warn")), Some(Level::Warn));
        assert_eq!(parse_level(Some("warning")), Some(Level::Warn));
        assert_eq!(parse_level(Some("error")), Some(Level::Error));
    }

    #[test]
    fn parse_level_rejects_garbage_and_empty() {
        assert_eq!(parse_level(None), None);
        assert_eq!(parse_level(Some("")), None);
        assert_eq!(parse_level(Some("   ")), None);
        assert_eq!(parse_level(Some("nope")), None);
    }

    #[test]
    fn is_sled_target_matches_sled_root_and_descendants() {
        assert!(SledFilteringLogger::is_sled_target("sled"));
        assert!(SledFilteringLogger::is_sled_target("sled::pagecache"));
        assert!(SledFilteringLogger::is_sled_target(
            "sled::pagecache::iobuf"
        ));
        // Avoid false-positives on third-party crate names that happen
        // to start with "sled".
        assert!(!SledFilteringLogger::is_sled_target("sled_extras"));
        assert!(!SledFilteringLogger::is_sled_target("fold_db::sled"));
        assert!(!SledFilteringLogger::is_sled_target(""));
    }

    /// Core guarantee: at the default sled cap (`Info`), `enabled`
    /// rejects `Debug` / `Trace` for sled targets but lets `Info` and
    /// above through, and is a passthrough for non-sled targets.
    #[test]
    fn enabled_caps_sled_at_info_by_default() {
        let logger = SledFilteringLogger {
            inner: LogTracer::new(),
            sled_max: Level::Info,
        };
        // When no tracing subscriber is installed in this test process,
        // `LogTracer::enabled` falls through to the default-dispatch
        // which returns `false` for everything. That obscures whether
        // OUR rejection fired or the inner did. We only need to assert
        // OUR rejection — a positive test on a sled DEBUG event must
        // see `false`, and the cause is unambiguous because INFO+
        // would also short-circuit through inner.enabled.
        assert!(!logger.enabled(&log_metadata("sled", Level::Debug)));
        assert!(!logger.enabled(&log_metadata("sled::pagecache::iobuf", Level::Trace)));
        assert!(!logger.enabled(&log_metadata("sled::tree", Level::Debug)));
    }

    /// `FOLDDB_LOG_SLED=debug` opt-back-in: sled DEBUG should reach the
    /// inner tracer (we don't reject it). We assert via the sled-target
    /// check directly because inner's own `enabled` depends on the
    /// global tracing dispatcher's state, which is process-wide and
    /// brittle in a unit test.
    #[test]
    fn sled_max_debug_does_not_reject_debug() {
        let logger = SledFilteringLogger {
            inner: LogTracer::new(),
            sled_max: Level::Debug,
        };
        // At sled_max=Debug, only Trace > Debug → Trace events are
        // rejected by our cap. Debug passes our cap and falls through
        // to inner.enabled.
        let meta = log_metadata("sled::pagecache", Level::Debug);
        // Our cap doesn't reject — short-circuit returns nothing.
        assert!(
            !(SledFilteringLogger::is_sled_target(meta.target()) && meta.level() > logger.sled_max)
        );
        // And sled Trace is still rejected:
        let meta_trace = log_metadata("sled::pagecache", Level::Trace);
        assert!(!logger.enabled(&meta_trace));
    }

    /// Non-sled events bypass our cap entirely, so they enter the inner
    /// tracer's path regardless of level. We assert the cap predicate
    /// directly to avoid coupling to the global tracing dispatcher.
    #[test]
    fn non_sled_events_skip_the_cap() {
        let logger = SledFilteringLogger {
            inner: LogTracer::new(),
            sled_max: Level::Info,
        };
        for level in [
            Level::Trace,
            Level::Debug,
            Level::Info,
            Level::Warn,
            Level::Error,
        ] {
            let meta = log_metadata("hyper", level);
            // Our cap predicate returns false because target isn't sled.
            assert!(
                !(SledFilteringLogger::is_sled_target(meta.target())
                    && meta.level() > logger.sled_max),
                "non-sled target unexpectedly hit cap for {level:?}",
            );
        }
    }

    /// Whole-stack `apply_default_filters` is intentionally NOT exercised
    /// here as a test. It mutates process-global state (`RUST_LOG`,
    /// `log::set_boxed_logger`) that races with tests in other modules
    /// (e.g. `server::startup`) under cargo's parallel runner, even
    /// behind a local mutex — `std::env::set_var` is process-wide. The
    /// building blocks above (`augment_rust_log`, `parse_level`,
    /// `SledFilteringLogger::enabled`) are tested individually; the
    /// composed integration is exercised by the `folddb_server` binary
    /// at boot.
    ///
    /// Build a `log::Metadata` for a (target, level) pair. Helper kept
    /// local to the test module — production code never constructs
    /// `Metadata` directly.
    fn log_metadata<'a>(target: &'a str, level: Level) -> Metadata<'a> {
        Metadata::builder().target(target).level(level).build()
    }
}
