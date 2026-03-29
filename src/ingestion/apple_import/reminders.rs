//! Extract reminders from Apple Reminders via osascript.

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

/// Extract all reminders (or from a specific list) from Apple Reminders.
pub fn extract(list: Option<&str>) -> Result<Vec<Reminder>, IngestionError> {
    let script = build_script(list);
    let raw = run_osascript(&script)?;
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
    let list_filter = match list {
        Some(name) => format!(
            r#"set targetList to list "{}"
    set reminderItems to every reminder of targetList"#,
            name.replace('"', "\\\"")
        ),
        None => "set reminderItems to every reminder".to_string(),
    };

    format!(
        r#"tell application "Reminders"
    {list_filter}
    set output to ""
    repeat with r in reminderItems
        set rName to name of r
        set rCompleted to completed of r
        set rContainer to name of container of r
        try
            set rDue to (due date of r) as string
        on error
            set rDue to "none"
        end try
        try
            set rPriority to priority of r
        on error
            set rPriority to 0
        end try
        set output to output & "<<<REM_START>>>" & rName & "<<<SEP>>>" & rContainer & "<<<SEP>>>" & rCompleted & "<<<SEP>>>" & rDue & "<<<SEP>>>" & rPriority & "<<<REM_END>>>"
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
    fn build_script_all() {
        let script = build_script(None);
        assert!(script.contains("set reminderItems to every reminder"));
        assert!(!script.contains("targetList"));
    }

    #[test]
    fn build_script_specific_list() {
        let script = build_script(Some("Shopping"));
        assert!(script.contains(r#"list "Shopping""#));
        assert!(script.contains("targetList"));
    }
}
