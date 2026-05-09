//! Apple data import: extract Notes, Reminders, Photos, and Calendar events from macOS apps.
//!
//! This module provides shared extraction logic used by both the CLI
//! (`folddb ingest apple-*`) and the HTTP server (Apple Import tab).
//! Extraction uses `osascript` to call AppleScript on macOS.

#[cfg(target_os = "macos")]
pub mod calendar;
#[cfg(target_os = "macos")]
pub mod contacts;
#[cfg(target_os = "macos")]
pub mod notes;
#[cfg(target_os = "macos")]
pub mod photos;
#[cfg(target_os = "macos")]
pub mod reminders;
pub mod sync_config;
pub mod sync_scheduler;

#[cfg(target_os = "macos")]
use crate::ingestion::IngestionError;
#[cfg(target_os = "macos")]
use sha2::{Digest, Sha256};

/// Check whether we're running on macOS (Apple import requires osascript).
pub fn is_available() -> bool {
    cfg!(target_os = "macos")
}

/// Default timeout for osascript calls (5 minutes).
/// Photo exports can be slow for large batches; 5 min handles up to ~200 photos.
#[cfg(target_os = "macos")]
const OSASCRIPT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Wallclock budget for the TCC permission pre-flight probe. The probes
/// only read aggregate counts (`count people`, `count of lists`, etc.) so
/// they don't paginate or trigger iCloud resolution; if the probe doesn't
/// return within this window the calling process is missing Automation
/// access (or the target app is wedged on something we can't unblock).
#[cfg(target_os = "macos")]
const TCC_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Run an AppleScript via osascript and return stdout.
///
/// Convenience wrapper around [`run_osascript_with_timeout`] using the
/// default [`OSASCRIPT_TIMEOUT`]. Callers that know the operation should
/// be quick (e.g. Contacts extraction, which is bounded by local address
/// book size) should call [`run_osascript_with_timeout`] directly with a
/// tighter budget so a missing permission surfaces in seconds rather than
/// holding the worker for 5 minutes.
#[cfg(target_os = "macos")]
pub fn run_osascript(script: &str, app_label: &str) -> Result<String, IngestionError> {
    run_osascript_with_timeout(script, app_label, OSASCRIPT_TIMEOUT)
}

/// Run an AppleScript via osascript with a caller-supplied wallclock
/// `timeout` and return stdout.
///
/// Kills the process after `timeout` to prevent indefinite hangs (iCloud
/// sync, missing Automation permission, unresponsive target app).
///
/// `app_label` names the target macOS app (e.g. "Reminders.app") so the
/// timeout error can point the user at the correct System Settings pane.
/// It is also used to pre-launch the target app via Launch Services so
/// the script doesn't hit error -600 ("Application isn't running") when
/// AppleScript's auto-launch of `tell application "X"` fails — a common
/// failure mode on Sonoma+ for apps that aren't already running
/// (Calendar, Contacts, Photos). Apps already running are a no-op.
#[cfg(target_os = "macos")]
pub fn run_osascript_with_timeout(
    script: &str,
    app_label: &str,
    timeout: std::time::Duration,
) -> Result<String, IngestionError> {
    ensure_app_launched(app_label);

    let child = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| IngestionError::Extraction(format!("Failed to run osascript: {}", e)))?;

    // Wait with timeout using a background thread + channel.
    let (tx, rx) = std::sync::mpsc::channel();
    let child_id = child.id();
    std::thread::spawn(move || {
        let result = child.wait_with_output();
        let _ = tx.send(result);
    });

    match rx.recv_timeout(timeout) {
        Ok(Ok(output)) => {
            if !output.status.success() {
                let stderr_str = String::from_utf8_lossy(&output.stderr);
                return Err(IngestionError::Extraction(format!(
                    "AppleScript error ({}): {}",
                    app_label, stderr_str
                )));
            }
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        }
        Ok(Err(e)) => Err(IngestionError::Extraction(format!(
            "Failed to wait for osascript ({}): {}",
            app_label, e
        ))),
        Err(_timeout) => {
            // Kill the timed-out process via pkill (child ownership moved to thread)
            let _ = std::process::Command::new("kill")
                .arg("-9")
                .arg(child_id.to_string())
                .status();
            Err(IngestionError::Extraction(format!(
                "osascript timed out after {} seconds talking to {}. The app may be \
                 unresponsive, syncing with iCloud, or missing Automation permission. \
                 Grant access in System Settings → Privacy & Security → Automation \
                 (and Full Disk Access for Photos.app).",
                timeout.as_secs(),
                app_label,
            )))
        }
    }
}

/// Tiny side-effect-free AppleScript that confirms the calling process
/// has Automation access for `app_label`. Returns `None` for apps without
/// a registered probe (the caller falls through and runs the full extract
/// directly).
///
/// Probes operate on aggregate counts only so they finish in milliseconds
/// even on populated stores — the slow case (iCloud collection resolution)
/// only kicks in when the script enumerates records, which `count` does
/// not do.
///
/// Photos.app is included because the import path also reaches it via
/// AppleScript (`tell application "Photos" to export ...`), so an
/// Automation-permission gap surfaces here just like the other apps. A
/// missing-Full-Disk-Access library will still slip past this probe and
/// fail later in the export step — but that's strictly better than the
/// pre-fix wallclock hang, and matches the granularity the rest of the
/// pre-flight provides.
#[cfg(target_os = "macos")]
pub(crate) fn tcc_probe_script(app_label: &str) -> Option<&'static str> {
    match app_label {
        "Contacts.app" => Some(r#"tell application "Contacts" to count people"#),
        "Notes.app" => Some(r#"tell application "Notes" to count notes"#),
        "Calendar.app" => Some(r#"tell application "Calendar" to count calendars"#),
        "Reminders.app" => Some(r#"tell application "Reminders" to count lists"#),
        "Photos.app" => Some(r#"tell application "Photos" to count albums"#),
        _ => None,
    }
}

/// Verify that the calling process has Automation access for `app_label`
/// before running a long extract. Returns `Ok(())` when the probe succeeds
/// (or when no probe is registered for the app), and an
/// `IngestionError::Extraction` with an actionable Privacy & Security →
/// Automation hint when the probe errors or times out.
///
/// Without this, a missing-permission run sits inside the long extract's
/// timeout (`OSASCRIPT_TIMEOUT` = 5 min) before surfacing — the probe
/// fails fast (within `TCC_PROBE_TIMEOUT`) so the user gets the same
/// actionable message in seconds.
#[cfg(target_os = "macos")]
pub fn preflight_permission(app_label: &str) -> Result<(), IngestionError> {
    let Some(script) = tcc_probe_script(app_label) else {
        return Ok(());
    };
    // Re-use the standard runner so the kill-on-timeout and Launch Services
    // pre-launch logic stays in one place. The probe's tight `TCC_PROBE_TIMEOUT`
    // is what makes this fast; the runner itself is generic.
    match run_osascript_with_timeout(script, app_label, TCC_PROBE_TIMEOUT) {
        Ok(_) => Ok(()),
        Err(IngestionError::Extraction(inner)) => Err(IngestionError::Extraction(format!(
            "{} access not granted (probe: {}). Grant access in System Settings → \
             Privacy & Security → Automation, then retry.",
            app_label.strip_suffix(".app").unwrap_or(app_label),
            inner
        ))),
        Err(other) => Err(other),
    }
}

/// Wallclock budget for the HTTP pre-flight probe path. Tighter than
/// [`TCC_PROBE_TIMEOUT`] because the onboarding wizard fires all five
/// probes in parallel from a single browser request — the user's
/// perceived latency is `max(per_probe)`, so we want it short enough
/// that "Checking permissions..." doesn't itself feel like the hang
/// we're trying to eliminate.
#[cfg(target_os = "macos")]
const HTTP_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Lightweight permission probe used by the HTTP pre-flight endpoint.
///
/// Runs the same fast TCC probe as [`preflight_permission`] but collapses
/// every outcome into a `bool` — `true` when the probe succeeds, `false`
/// when osascript errors or times out. Apps without a registered probe
/// also return `true`: the assumption is the caller will fall through
/// and surface any later osascript failure in the import job's progress
/// stream. Caller-facing semantics: "we have no reason to block the
/// user from clicking Import yet."
///
/// Unlike `preflight_permission`, this does NOT format an error message,
/// which is the right shape for a pre-flight that needs to render a
/// per-source `{contacts: bool, notes: bool, ...}` object without
/// stringly-typed shoehorning.
#[cfg(target_os = "macos")]
pub fn probe_permission(app_label: &str) -> bool {
    let Some(script) = tcc_probe_script(app_label) else {
        return true;
    };
    run_osascript_with_timeout(script, app_label, HTTP_PROBE_TIMEOUT).is_ok()
}

/// Pre-launch the target macOS app via Launch Services so the subsequent
/// `tell application "X"` block doesn't fail with `-600 Application
/// isn't running`. `app_label` is a filename-style label like
/// `"Calendar.app"`; we strip the `.app` suffix for the `open -a`
/// argument. Flags:
///   * `-g` — do not bring the app to the foreground.
///   * `-j` — launch hidden so the ingestion doesn't disturb focus.
///
/// Errors are swallowed: if the launch fails (e.g. the app is not
/// installed, or `open` itself errors), we still run the script and let
/// its own error path produce the user-facing message — doubling up on
/// errors here would obscure the real cause.
#[cfg(target_os = "macos")]
fn ensure_app_launched(app_label: &str) {
    let app_name = app_name_from_label(app_label);
    let _ = std::process::Command::new("open")
        .arg("-g")
        .arg("-j")
        .arg("-a")
        .arg(app_name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Translate a `"X.app"` label into the bare `"X"` form expected by
/// `open -a`. Labels without the `.app` suffix pass through unchanged.
#[cfg(target_os = "macos")]
fn app_name_from_label(app_label: &str) -> &str {
    app_label.strip_suffix(".app").unwrap_or(app_label)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn app_name_from_label_strips_dot_app_suffix() {
        assert_eq!(app_name_from_label("Calendar.app"), "Calendar");
        assert_eq!(app_name_from_label("Contacts.app"), "Contacts");
        assert_eq!(app_name_from_label("Photos.app"), "Photos");
    }

    #[test]
    fn app_name_from_label_passes_through_bare_names() {
        assert_eq!(app_name_from_label("Calendar"), "Calendar");
        assert_eq!(app_name_from_label(""), "");
    }

    #[test]
    fn tcc_probe_script_registered_for_each_automation_app() {
        // The HTTP pre-flight endpoint relies on the probe set covering
        // every Apple data source the onboarding wizard offers. If an app
        // is missing here, the wizard has no way to detect its missing
        // permission before kicking off a 30s-osascript-hang import.
        for (app_label, expected_app) in [
            ("Contacts.app", "Contacts"),
            ("Notes.app", "Notes"),
            ("Calendar.app", "Calendar"),
            ("Reminders.app", "Reminders"),
            ("Photos.app", "Photos"),
        ] {
            let probe = tcc_probe_script(app_label)
                .unwrap_or_else(|| panic!("{} probe registered", app_label));
            // The probe MUST be a `count`-style aggregate read so it doesn't
            // paginate or trigger iCloud resolution — otherwise it stops being
            // a fast pre-flight and becomes the same hang it's meant to detect.
            assert!(
                probe.contains("count"),
                "{} probe must use count-style aggregate read, got: {}",
                app_label,
                probe,
            );
            assert!(
                probe.contains(&format!(r#"tell application "{}""#, expected_app)),
                "{} probe must address its app, got: {}",
                app_label,
                probe,
            );
        }
    }

    #[test]
    fn tcc_probe_script_unregistered_apps_return_none() {
        // Registering a probe for an app means the caller wants
        // preflight_permission to gate that app's extract. Apps without
        // a probe pass through silently — verify a few unrelated labels
        // don't accidentally pick up a probe.
        assert!(tcc_probe_script("UnregisteredApp.app").is_none());
        assert!(tcc_probe_script("").is_none());
    }

    #[test]
    fn preflight_permission_passes_through_for_unregistered_app() {
        // No probe → fast Ok(()), no osascript call. This guards against
        // someone adding a default-error fallback that would break Photos /
        // any other source whose pre-flight check isn't an Automation probe.
        let result = preflight_permission("UnregisteredApp.app");
        assert!(result.is_ok());
    }

    #[test]
    fn probe_permission_returns_true_for_unregistered_app() {
        // The HTTP pre-flight endpoint feeds the result of probe_permission
        // straight into the per-source bool returned to the wizard. An
        // unregistered app must NOT be reported as missing permission —
        // that would surface a false-positive "Grant Access" banner the
        // user can do nothing about.
        assert!(probe_permission("UnregisteredApp.app"));
    }

    #[test]
    fn run_osascript_with_timeout_kills_long_running_script() {
        // Use a no-op `delay` script that comfortably outlives the
        // sub-second timeout. The runner must kill the process and surface
        // the timeout message — not block until the script's own delay
        // expires (which would defeat the per-call timeout knob).
        let start = std::time::Instant::now();
        let result = run_osascript_with_timeout(
            "delay 30",
            "Contacts.app",
            std::time::Duration::from_millis(500),
        );
        let elapsed = start.elapsed();

        let err = result.expect_err("script should time out");
        let msg = err.to_string();
        assert!(
            msg.contains("timed out"),
            "expected timeout message, got: {msg}"
        );
        assert!(
            msg.contains("Contacts.app"),
            "timeout message must name the app for the user-facing hint, got: {msg}"
        );
        // Generous upper bound: the runner spawn + kill + Launch Services
        // pre-launch can reasonably take a few seconds on a loaded macOS
        // box, but it must not block on the full 30-second `delay`.
        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "runner blocked on script's own delay instead of enforcing timeout: {:?}",
            elapsed
        );
    }
}

/// Compute a short content hash for deduplication.
#[cfg(target_os = "macos")]
pub fn content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let result = hasher.finalize();
    hex::encode(&result[..8])
}
