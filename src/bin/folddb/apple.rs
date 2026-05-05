//! Apple platform ingestion (macOS only).
//!
//! Extracts data from Apple Notes, Photos, and Reminders using osascript,
//! then POSTs to the daemon's ingestion endpoint.

use crate::client::FoldDbClient;
use crate::commands::CommandOutput;
use crate::error::CliError;
use regex::Regex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Wallclock budget for the TCC pre-flight probe. The whole point is to
/// fail fast when access is denied or pending; iCloud resolution doesn't
/// kick in for the trivial probes we send, so 5s is plenty.
const TCC_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Cadence for the "still working..." progress line during long extracts.
const PROGRESS_TICK: Duration = Duration::from_secs(10);

fn content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let result = hasher.finalize();
    hex::encode(&result[..8])
}

/// Map an `app_label` like `"Reminders.app"` into a TCC pre-flight probe.
///
/// The probes are intentionally side-effect-free and operate on aggregate
/// metadata only (`count of <collection>`), so they don't paginate or
/// resolve large collections through iCloud. If macOS has not yet granted
/// the calling process access, the probe either returns an osascript error
/// (-1743 / "Not authorized") OR hangs waiting on a TCC prompt that never
/// surfaces in non-interactive contexts — both cases are handled by the
/// short wallclock timeout in [`tcc_preflight`].
fn tcc_probe_script(app_label: &str) -> Option<&'static str> {
    match app_label {
        "Reminders.app" => Some(r#"tell application "Reminders" to count of lists"#),
        "Notes.app" => Some(r#"tell application "Notes" to count of folders"#),
        "Photos.app" => Some(r#"tell application "Photos" to count of albums"#),
        "Calendar.app" => Some(r#"tell application "Calendar" to count of calendars"#),
        _ => None,
    }
}

/// Build the user-facing hint shown on TCC denial / probe timeout. The
/// message names the System Settings pane the user needs to visit.
fn tcc_hint(app_label: &str) -> String {
    let app = app_label.strip_suffix(".app").unwrap_or(app_label);
    format!(
        "Grant access in System Settings → Privacy & Security → Automation → \
         enable \"{app}\" for the process running folddb (Terminal.app, your \
         shell, or folddb_server). Then re-run the command."
    )
}

/// Run a tiny AppleScript probe against `app_label` to verify TCC access.
///
/// Returns `Ok(())` if the probe completes successfully within
/// `TCC_PROBE_TIMEOUT`. Otherwise returns a [`CliError`] whose message
/// names the missing permission and whose hint points at the System
/// Settings pane the user should open. Apps without a registered probe
/// pass through without checking.
fn tcc_preflight(app_label: &str) -> Result<(), CliError> {
    let Some(script) = tcc_probe_script(app_label) else {
        return Ok(());
    };

    let mut child = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            CliError::new(format!("Failed to spawn osascript probe: {}", e))
                .with_hint(tcc_hint(app_label))
        })?;

    let deadline = Instant::now() + TCC_PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    return Ok(());
                }
                let mut stderr_buf = String::new();
                if let Some(mut stderr) = child.stderr.take() {
                    use std::io::Read;
                    let _ = stderr.read_to_string(&mut stderr_buf);
                }
                return Err(CliError::new(format!(
                    "{} access denied (osascript: {})",
                    app_label.strip_suffix(".app").unwrap_or(app_label),
                    stderr_buf.trim()
                ))
                .with_hint(tcc_hint(app_label)));
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    return Err(CliError::new(format!(
                        "{} access not yet granted (TCC probe timed out after {}s)",
                        app_label.strip_suffix(".app").unwrap_or(app_label),
                        TCC_PROBE_TIMEOUT.as_secs()
                    ))
                    .with_hint(tcc_hint(app_label)));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                let _ = child.kill();
                return Err(CliError::new(format!("osascript probe failed: {}", e))
                    .with_hint(tcc_hint(app_label)));
            }
        }
    }
}

/// Run an AppleScript and return its stdout, with a wallclock timeout and
/// a periodic "still working..." progress line.
///
/// `app_label` (e.g. `"Reminders.app"`) is used in the timeout error so the
/// user knows which TCC pane to open. `progress_label` is the short noun
/// shown in the periodic line (e.g. `"reminders"`).
///
/// On timeout we kill the osascript process and return a `CliError` with
/// a TCC hint — the most common cause of an osascript hang on macOS is a
/// deferred privacy prompt that the calling process can't surface.
fn run_osascript(
    script: &str,
    app_label: &str,
    progress_label: &str,
    timeout: Duration,
) -> Result<String, CliError> {
    let mut child = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| CliError::new(format!("Failed to run osascript: {}", e)))?;

    let stop = Arc::new(AtomicBool::new(false));
    let progress_handle = {
        let stop = Arc::clone(&stop);
        let label = progress_label.to_string();
        std::thread::spawn(move || {
            let started = Instant::now();
            let mut next_tick = started + PROGRESS_TICK;
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(200));
                if Instant::now() >= next_tick && !stop.load(Ordering::Relaxed) {
                    let elapsed = started.elapsed().as_secs();
                    eprintln!("Still exporting {label}... ({elapsed}s elapsed)");
                    next_tick = Instant::now() + PROGRESS_TICK;
                }
            }
        })
    };

    let deadline = Instant::now() + timeout;
    let outcome = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(CliError::new(format!(
                        "osascript timed out after {}s talking to {}. The app may be \
                         unresponsive, syncing with iCloud, or missing Automation \
                         permission.",
                        timeout.as_secs(),
                        app_label
                    ))
                    .with_hint(tcc_hint(app_label)));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(CliError::new(format!("osascript wait failed: {}", e)));
            }
        }
    };

    stop.store(true, Ordering::Relaxed);
    let _ = progress_handle.join();

    let status = outcome?;
    let mut stdout_buf = Vec::new();
    let mut stderr_buf = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        use std::io::Read;
        let _ = stdout.read_to_end(&mut stdout_buf);
    }
    if let Some(mut stderr) = child.stderr.take() {
        use std::io::Read;
        let _ = stderr.read_to_string(&mut stderr_buf);
    }

    if !status.success() {
        return Err(
            CliError::new(format!("osascript failed: {}", stderr_buf.trim()))
                .with_hint(tcc_hint(app_label)),
        );
    }

    Ok(String::from_utf8_lossy(&stdout_buf).to_string())
}

// ---------------------------------------------------------------------------
// Apple Notes
// ---------------------------------------------------------------------------

struct Note {
    title: String,
    body: String,
    created_at: String,
    modified_at: String,
}

fn build_notes_script(folder: Option<&str>) -> String {
    let folder_filter = match folder {
        Some(name) => format!(
            r#"set targetFolder to folder "{}"
            set noteList to every note of targetFolder"#,
            name.replace('"', "\\\"")
        ),
        None => "set noteList to every note".to_string(),
    };

    format!(
        r#"tell application "Notes"
    {folder_filter}
    set output to ""
    repeat with n in noteList
        set noteBody to plaintext of n
        if length of noteBody > 20 then
            set noteTitle to name of n
            set noteCreated to (creation date of n) as string
            set noteModified to (modification date of n) as string
            set output to output & "<<<NOTE_START>>>" & noteTitle & "<<<SEP>>>" & noteBody & "<<<SEP>>>" & noteCreated & "<<<SEP>>>" & noteModified & "<<<NOTE_END>>>"
        end if
    end repeat
    return output
end tell"#
    )
}

fn parse_notes_output(raw: &str) -> Result<Vec<Note>, CliError> {
    let re = Regex::new(
        r"<<<NOTE_START>>>(.*?)<<<SEP>>>(.*?)<<<SEP>>>(.*?)<<<SEP>>>(.*?)<<<NOTE_END>>>",
    )
    .map_err(|e| CliError::new(format!("Regex error: {}", e)))?;

    let mut notes = Vec::new();
    for cap in re.captures_iter(raw) {
        notes.push(Note {
            title: cap[1].trim().to_string(),
            body: cap[2].trim().to_string(),
            created_at: cap[3].trim().to_string(),
            modified_at: cap[4].trim().to_string(),
        });
    }
    Ok(notes)
}

pub async fn ingest_notes(
    client: &FoldDbClient,
    folder: Option<&str>,
    batch_size: usize,
    timeout_seconds: u64,
) -> Result<CommandOutput, CliError> {
    tcc_preflight("Notes.app")?;
    eprintln!("Exporting notes from Apple Notes...");
    let script = build_notes_script(folder);
    let raw = run_osascript(
        &script,
        "Notes.app",
        "notes",
        Duration::from_secs(timeout_seconds),
    )?;

    let notes = parse_notes_output(&raw)?;
    if notes.is_empty() {
        return Err(CliError::new("No notes found in Apple Notes"));
    }

    let total = notes.len();
    eprintln!("Found {} notes. Ingesting...", total);

    let mut all_results = Vec::new();
    for chunk in notes.chunks(batch_size) {
        let records: Vec<Value> = chunk
            .iter()
            .map(|n| {
                json!({
                    "title": n.title,
                    "body": n.body,
                    "created_at": n.created_at,
                    "modified_at": n.modified_at,
                    "content_hash": content_hash(&n.body),
                    "source": "apple_notes",
                })
            })
            .collect();

        let result = client.ingest_process(&records).await?;
        all_results.push(result);
    }

    Ok(CommandOutput::Message(format!(
        "Ingested {} notes from Apple Notes",
        total
    )))
}

// ---------------------------------------------------------------------------
// Apple Reminders
// ---------------------------------------------------------------------------

struct Reminder {
    name: String,
    body: String,
    due_date: String,
    completed: bool,
}

/// Build the reminders extraction AppleScript.
///
/// Mirrors the optimisation pattern in `src/ingestion/apple_import/reminders.rs`:
/// iterate **per list** rather than `every reminder` globally, filter
/// completed reminders out (they accumulate forever on iCloud accounts and
/// were the historical hang source), and use bulk property accessors so
/// each property is one IPC round-trip per list. The CLI keeps its own
/// record shape (`name / body / due_date / completed`) to preserve
/// backwards-compatibility with existing user schemas — that's why we
/// don't simply call into the library extractor.
fn build_reminders_script(list: Option<&str>) -> String {
    let target_filter = match list {
        Some(name) => format!(r#""{}""#, name.replace('"', "\\\"")),
        None => "\"\"".to_string(),
    };

    format!(
        r#"tell application "Reminders"
    set output to ""
    set targetFilter to {target_filter}
    repeat with lst in lists
        set lstName to name of lst
        if targetFilter is "" or targetFilter is equal to lstName then
            set allNames to name of reminders of lst whose completed is false
            set remCount to count of allNames
            if remCount > 0 then
                set allBodies to body of reminders of lst whose completed is false
                set allDues to due date of reminders of lst whose completed is false
                repeat with i from 1 to remCount
                    set rName to item i of allNames
                    set rBody to item i of allBodies
                    if rBody is missing value then set rBody to ""
                    set rDue to item i of allDues
                    if rDue is missing value then
                        set rDueStr to ""
                    else
                        set rDueStr to rDue as string
                    end if
                    set output to output & "<<<REM_START>>>" & rName & "<<<SEP>>>" & rBody & "<<<SEP>>>" & rDueStr & "<<<SEP>>>false<<<REM_END>>>"
                end repeat
            end if
        end if
    end repeat
    return output
end tell"#
    )
}

fn parse_reminders_output(raw: &str) -> Result<Vec<Reminder>, CliError> {
    let re =
        Regex::new(r"<<<REM_START>>>(.*?)<<<SEP>>>(.*?)<<<SEP>>>(.*?)<<<SEP>>>(.*?)<<<REM_END>>>")
            .map_err(|e| CliError::new(format!("Regex error: {}", e)))?;

    let mut reminders = Vec::new();
    for cap in re.captures_iter(raw) {
        reminders.push(Reminder {
            name: cap[1].trim().to_string(),
            body: cap[2].trim().to_string(),
            due_date: cap[3].trim().to_string(),
            completed: cap[4].trim() == "true",
        });
    }
    Ok(reminders)
}

pub async fn ingest_reminders(
    client: &FoldDbClient,
    list: Option<&str>,
    batch_size: usize,
    timeout_seconds: u64,
) -> Result<CommandOutput, CliError> {
    tcc_preflight("Reminders.app")?;
    eprintln!("Exporting reminders from Apple Reminders...");
    let script = build_reminders_script(list);
    let raw = run_osascript(
        &script,
        "Reminders.app",
        "reminders",
        Duration::from_secs(timeout_seconds),
    )?;

    let reminders = parse_reminders_output(&raw)?;
    if reminders.is_empty() {
        return Err(CliError::new("No reminders found in Apple Reminders"));
    }

    let total = reminders.len();
    eprintln!("Found {} reminders. Ingesting...", total);

    let records: Vec<Value> = reminders
        .iter()
        .map(|r| {
            json!({
                "name": r.name,
                "body": r.body,
                "due_date": r.due_date,
                "completed": r.completed,
                "content_hash": content_hash(&r.name),
                "source": "apple_reminders",
            })
        })
        .collect();

    for chunk in records.chunks(batch_size) {
        client.ingest_process(chunk).await?;
    }

    Ok(CommandOutput::Message(format!(
        "Ingested {} reminders from Apple Reminders",
        total
    )))
}

// ---------------------------------------------------------------------------
// Apple Photos
// ---------------------------------------------------------------------------

pub async fn ingest_photos(
    client: &FoldDbClient,
    album: Option<&str>,
    limit: usize,
    batch_size: usize,
    timeout_seconds: u64,
) -> Result<CommandOutput, CliError> {
    tcc_preflight("Photos.app")?;
    eprintln!("Exporting photos from Apple Photos...");

    let album_filter = match album {
        Some(name) => format!(
            r#"set targetAlbum to album "{}"
            set photoList to every media item of targetAlbum"#,
            name.replace('"', "\\\"")
        ),
        None => "set photoList to every media item".to_string(),
    };

    let script = format!(
        r#"tell application "Photos"
    {album_filter}
    set output to ""
    set counter to 0
    repeat with p in photoList
        if counter >= {limit} then exit repeat
        set photoName to filename of p
        set photoDate to (date of p) as string
        set photoDesc to ""
        try
            set photoDesc to description of p
            if photoDesc is missing value then set photoDesc to ""
        end try
        set output to output & "<<<PHOTO_START>>>" & photoName & "<<<SEP>>>" & photoDate & "<<<SEP>>>" & photoDesc & "<<<PHOTO_END>>>"
        set counter to counter + 1
    end repeat
    return output
end tell"#
    );

    let raw = run_osascript(
        &script,
        "Photos.app",
        "photos",
        Duration::from_secs(timeout_seconds),
    )?;

    let re = Regex::new(r"<<<PHOTO_START>>>(.*?)<<<SEP>>>(.*?)<<<SEP>>>(.*?)<<<PHOTO_END>>>")
        .map_err(|e| CliError::new(format!("Regex error: {}", e)))?;

    let mut photos = Vec::new();
    for cap in re.captures_iter(&raw) {
        photos.push(json!({
            "filename": cap[1].trim(),
            "date": cap[2].trim(),
            "description": cap[3].trim(),
            "content_hash": content_hash(cap[1].trim()),
            "source": "apple_photos",
        }));
    }

    if photos.is_empty() {
        return Err(CliError::new("No photos found in Apple Photos"));
    }

    let total = photos.len();
    eprintln!("Found {} photos. Ingesting...", total);

    for chunk in photos.chunks(batch_size) {
        client.ingest_process(chunk).await?;
    }

    Ok(CommandOutput::Message(format!(
        "Ingested {} photos from Apple Photos",
        total
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_notes_basic() {
        let raw = "<<<NOTE_START>>>My Title<<<SEP>>>This is the body of the note<<<SEP>>>2024-01-15<<<SEP>>>2024-01-16<<<NOTE_END>>>";
        let notes = parse_notes_output(raw).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "My Title");
    }

    #[test]
    fn parse_notes_empty() {
        let notes = parse_notes_output("").unwrap();
        assert!(notes.is_empty());
    }

    #[test]
    fn parse_reminders_basic() {
        let raw = "<<<REM_START>>>Buy milk<<<SEP>>>From the store<<<SEP>>>2024-01-15<<<SEP>>>false<<<REM_END>>>";
        let reminders = parse_reminders_output(raw).unwrap();
        assert_eq!(reminders.len(), 1);
        assert_eq!(reminders[0].name, "Buy milk");
        assert!(!reminders[0].completed);
    }

    #[test]
    fn content_hash_deterministic() {
        assert_eq!(content_hash("hello"), content_hash("hello"));
        assert_ne!(content_hash("hello"), content_hash("world"));
    }

    #[test]
    fn build_reminders_script_skips_completed_and_uses_bulk_reads() {
        // Regression guard for the hang fix: the CLI script must filter
        // completed reminders and use bulk property accessors so iCloud
        // resolution doesn't fan out to per-item round-trips.
        let script = build_reminders_script(None);
        assert!(script.contains("whose completed is false"));
        assert!(script.contains("name of reminders of lst"));
        assert!(script.contains("body of reminders of lst"));
        assert!(script.contains("due date of reminders of lst"));
        // The slow patterns must be gone.
        assert!(!script.contains("set reminderList to every reminder"));
        assert!(!script.contains("repeat with r in reminderList"));
    }

    #[test]
    fn build_reminders_script_specific_list_uses_name_filter() {
        let script = build_reminders_script(Some("Shopping"));
        assert!(script.contains(r#"set targetFilter to "Shopping""#));
        assert!(script.contains("is equal to lstName"));
        // No hard `list "Shopping"` lookup — that throws on a missing or
        // renamed list. We filter inside the loop instead.
        assert!(!script.contains(r#"list "Shopping""#));
    }

    #[test]
    fn build_reminders_script_escapes_list_quotes() {
        let script = build_reminders_script(Some(r#"My "Quoted" List"#));
        assert!(script.contains(r#"set targetFilter to "My \"Quoted\" List""#));
    }

    #[test]
    fn tcc_probe_script_covers_all_apple_apps() {
        // Each apple-* ingest path needs a side-effect-free probe so
        // tcc_preflight can fail fast with a useful error.
        assert!(tcc_probe_script("Reminders.app").is_some());
        assert!(tcc_probe_script("Notes.app").is_some());
        assert!(tcc_probe_script("Photos.app").is_some());
        assert!(tcc_probe_script("Calendar.app").is_some());
    }

    #[test]
    fn tcc_probe_script_unknown_app_returns_none() {
        assert!(tcc_probe_script("Mail.app").is_none());
    }

    #[test]
    fn tcc_hint_strips_dot_app_and_names_the_app() {
        let hint = tcc_hint("Reminders.app");
        assert!(hint.contains("\"Reminders\""));
        assert!(hint.contains("System Settings"));
        assert!(hint.contains("Privacy & Security"));
        assert!(hint.contains("Automation"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn run_osascript_times_out() {
        // A `delay` call that exceeds the timeout must produce a CliError
        // mentioning the wallclock budget — and must not block forever.
        let result = run_osascript(
            r#"delay 5"#,
            "Reminders.app",
            "reminders",
            Duration::from_millis(500),
        );
        let err = result.expect_err("expected timeout error");
        assert!(
            err.message.contains("timed out"),
            "unexpected error message: {}",
            err.message
        );
    }
}
