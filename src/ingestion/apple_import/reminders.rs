//! Extract reminders from Apple Reminders via osascript.
//!
//! ## Performance note
//!
//! Earlier versions iterated `every reminder` globally and read properties one
//! at a time. On an iCloud-backed Reminders with a few hundred completed items
//! that hung for minutes (see the `OSASCRIPT_TIMEOUT` in `mod.rs`) because
//! each property access is a round-trip to Reminders.app and iCloud resolution
//! runs per item.
//!
//! The current script:
//!   - iterates **per list** rather than `every reminder` globally,
//!   - filters out completed reminders with `whose completed is false`,
//!   - uses **bulk property access** (`name of reminders of lst whose …`)
//!     which returns a flat list in one IPC round-trip per property.
//!
//! Dropping 83 completed + per-property iteration, the 12-reminder dogfood
//! case went from > 5 min (timeout) to ~12 s.

use regex::Regex;
use serde_json::{json, Value};

use super::{content_hash, run_osascript};
use crate::ingestion::IngestionError;

/// A single reminder extracted from Apple Reminders.
pub struct Reminder {
    pub name: String,
    pub list: String,
    pub completed: bool,
    pub due_date: String,
    pub priority: i64,
}

/// Extract active (not-yet-completed) reminders.
///
/// Pass `Some(list_name)` to restrict to a single list, or `None` for every
/// list. Completed reminders are always skipped — they pile up forever on a
/// normal iCloud account and are the root cause of the 5-minute timeout this
/// module used to hit. Re-importing completed reminders is not something the
/// UI currently offers; a future param can restore them if needed.
pub fn extract(list: Option<&str>) -> Result<Vec<Reminder>, IngestionError> {
    let script = build_script(list);
    let raw = run_osascript(&script, "Reminders.app")?;
    parse_output(&raw)
}

/// Convert extracted reminders into JSON records ready for ingestion.
pub fn to_json_records(reminders: &[Reminder]) -> Vec<Value> {
    reminders
        .iter()
        .map(|r| {
            let hash = content_hash(&r.name);
            json!({
                "name": r.name,
                "list": r.list,
                "completed": r.completed,
                "due_date": r.due_date,
                "priority": r.priority,
                "content_hash": hash,
                "source": "apple_reminders",
            })
        })
        .collect()
}

pub fn build_script(list: Option<&str>) -> String {
    let target_filter = match list {
        Some(name) => format!(r#""{}""#, name.replace('"', "\\\"")),
        None => "\"\"".to_string(),
    };

    // Implementation notes for the AppleScript below:
    //
    // * We iterate `lists` and compare names inside the loop rather than
    //   using `list "name"` lookup so a missing/renamed list yields an
    //   empty result instead of a hard AppleScript error.
    // * Each property (name / due date / priority) is fetched with a
    //   single specifier expression (`name of reminders of lst whose …`)
    //   so the whole property becomes one IPC round-trip. Binding the
    //   filtered collection to a local variable and then reading
    //   `name of that variable` fails at runtime because AppleScript
    //   serialises the reference list and can't apply `name of` to the
    //   literal. This triple-read is still ~30× faster than the old
    //   per-reminder loop because the slow part is per-object resolution.
    // * `missing value` for `due date` / `priority` is common (reminders
    //   without a due date or priority); we normalise to `"none"` / `0`
    //   before emitting.
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
                set allDues to due date of reminders of lst whose completed is false
                set allPrios to priority of reminders of lst whose completed is false
                repeat with i from 1 to remCount
                    set rName to item i of allNames
                    set rDue to item i of allDues
                    set rPrio to item i of allPrios
                    if rDue is missing value then
                        set rDueStr to "none"
                    else
                        set rDueStr to rDue as string
                    end if
                    if rPrio is missing value then set rPrio to 0
                    set output to output & "<<<REM_START>>>" & rName & "<<<SEP>>>" & lstName & "<<<SEP>>>false<<<SEP>>>" & rDueStr & "<<<SEP>>>" & rPrio & "<<<REM_END>>>"
                end repeat
            end if
        end if
    end repeat
    return output
end tell"#
    )
}

pub fn parse_output(raw: &str) -> Result<Vec<Reminder>, IngestionError> {
    let re = Regex::new(
        r"<<<REM_START>>>(.*?)<<<SEP>>>(.*?)<<<SEP>>>(.*?)<<<SEP>>>(.*?)<<<SEP>>>(.*?)<<<REM_END>>>"
    )
    .map_err(|e| IngestionError::Extraction(format!("Regex error: {}", e)))?;

    let mut reminders = Vec::new();
    for cap in re.captures_iter(raw) {
        let completed_str = cap[3].trim().to_lowercase();
        let completed = completed_str == "true";
        let priority: i64 = cap[5].trim().parse().unwrap_or(0);

        reminders.push(Reminder {
            name: cap[1].trim().to_string(),
            list: cap[2].trim().to_string(),
            completed,
            due_date: cap[4].trim().to_string(),
            priority,
        });
    }
    Ok(reminders)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_output_basic() {
        let raw = "<<<REM_START>>>Buy groceries<<<SEP>>>Shopping<<<SEP>>>false<<<SEP>>>2024-01-20 10:00:00<<<SEP>>>1<<<REM_END>>>";
        let reminders = parse_output(raw).unwrap();
        assert_eq!(reminders.len(), 1);
        assert_eq!(reminders[0].name, "Buy groceries");
        assert_eq!(reminders[0].list, "Shopping");
        assert!(!reminders[0].completed);
        assert_eq!(reminders[0].due_date, "2024-01-20 10:00:00");
        assert_eq!(reminders[0].priority, 1);
    }

    #[test]
    fn parse_output_completed() {
        let raw = "<<<REM_START>>>Done task<<<SEP>>>Work<<<SEP>>>true<<<SEP>>>none<<<SEP>>>0<<<REM_END>>>";
        let reminders = parse_output(raw).unwrap();
        assert_eq!(reminders.len(), 1);
        assert!(reminders[0].completed);
    }

    #[test]
    fn parse_output_multiple() {
        let raw = "<<<REM_START>>>Task 1<<<SEP>>>List A<<<SEP>>>false<<<SEP>>>none<<<SEP>>>0<<<REM_END>>><<<REM_START>>>Task 2<<<SEP>>>List B<<<SEP>>>true<<<SEP>>>none<<<SEP>>>5<<<REM_END>>>";
        let reminders = parse_output(raw).unwrap();
        assert_eq!(reminders.len(), 2);
        assert_eq!(reminders[1].priority, 5);
    }

    #[test]
    fn parse_output_empty() {
        let reminders = parse_output("").unwrap();
        assert!(reminders.is_empty());
    }

    #[test]
    fn build_script_all_lists_skips_completed() {
        // Regression guard for the hang fix: the script must filter out
        // completed reminders (they're the bulk of data on a typical
        // iCloud-backed Reminders account) and use bulk property access
        // rather than per-reminder iteration.
        let script = build_script(None);
        assert!(script.contains("whose completed is false"));
        assert!(script.contains("name of reminders of lst"));
        assert!(script.contains("due date of reminders of lst"));
        assert!(script.contains("priority of reminders of lst"));
        // The old slow patterns must be gone.
        assert!(!script.contains("set reminderItems to every reminder"));
        assert!(!script.contains("repeat with r in reminderItems"));
    }

    #[test]
    fn build_script_specific_list_uses_name_filter() {
        let script = build_script(Some("Shopping"));
        assert!(script.contains(r#"set targetFilter to "Shopping""#));
        assert!(script.contains("is equal to lstName"));
        // No hard `list "Shopping"` lookup — that would throw on a
        // missing/renamed list; we filter inside the loop instead.
        assert!(!script.contains(r#"list "Shopping""#));
    }

    #[test]
    fn build_script_escapes_list_name_quotes() {
        let script = build_script(Some(r#"My "Quoted" List"#));
        assert!(script.contains(r#"set targetFilter to "My \"Quoted\" List""#));
    }

    #[test]
    fn to_json_records_emits_expected_fields() {
        let reminders = vec![Reminder {
            name: "Walk dog".into(),
            list: "Home".into(),
            completed: false,
            due_date: "2026-04-20 09:00:00".into(),
            priority: 1,
        }];
        let records = to_json_records(&reminders);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["name"], "Walk dog");
        assert_eq!(records[0]["list"], "Home");
        assert_eq!(records[0]["completed"], false);
        assert_eq!(records[0]["priority"], 1);
        assert_eq!(records[0]["source"], "apple_reminders");
        assert!(records[0]["content_hash"].is_string());
    }
}
