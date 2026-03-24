use crate::commands::apple::{build_client, content_hash, post_ingestion_batch, run_osascript};
use crate::commands::CommandOutput;
use crate::error::CliError;
use crate::output::spinner;
use crate::output::OutputMode;
use serde_json::json;

pub async fn run(
    list: Option<&str>,
    user_hash: &str,
    mode: OutputMode,
) -> Result<CommandOutput, CliError> {
    let sp = if mode == OutputMode::Human {
        Some(spinner::new_spinner("Exporting reminders from Apple Reminders..."))
    } else {
        None
    };

    let script = build_reminders_script(list);
    let raw = run_osascript(&script)?;

    if let Some(ref pb) = sp {
        spinner::finish_spinner(pb, "Reminders exported");
    }

    let reminders = parse_reminders_output(&raw)?;
    if reminders.is_empty() {
        return Err(CliError::new("No reminders found in Apple Reminders"));
    }

    let total = reminders.len();
    let (client, base_url) = build_client(user_hash)?;

    let sp2 = if mode == OutputMode::Human {
        Some(spinner::new_spinner(&format!("Ingesting {} reminders...", total)))
    } else {
        None
    };

    let records: Vec<serde_json::Value> = reminders
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
        .collect();

    let ids = post_ingestion_batch(&client, &base_url, records).await?;

    if let Some(ref pb) = sp2 {
        spinner::finish_spinner(pb, &format!("Ingested {} reminders", ids.len()));
    }

    Ok(CommandOutput::AppleIngestSuccess {
        source: "apple_reminders".to_string(),
        total,
        ingested: ids.len(),
        ids,
    })
}

struct Reminder {
    name: String,
    list: String,
    completed: bool,
    due_date: String,
    priority: i64,
}

fn build_reminders_script(list: Option<&str>) -> String {
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

fn parse_reminders_output(raw: &str) -> Result<Vec<Reminder>, CliError> {
    let re = regex::Regex::new(
        r"<<<REM_START>>>(.*?)<<<SEP>>>(.*?)<<<SEP>>>(.*?)<<<SEP>>>(.*?)<<<SEP>>>(.*?)<<<REM_END>>>"
    )
    .map_err(|e| CliError::new(format!("Regex error: {}", e)))?;

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
    fn parse_reminders_output_basic() {
        let raw = "<<<REM_START>>>Buy groceries<<<SEP>>>Shopping<<<SEP>>>false<<<SEP>>>2024-01-20 10:00:00<<<SEP>>>1<<<REM_END>>>";
        let reminders = parse_reminders_output(raw).map_err(|e| e.to_string()).unwrap();
        assert_eq!(reminders.len(), 1);
        assert_eq!(reminders[0].name, "Buy groceries");
        assert_eq!(reminders[0].list, "Shopping");
        assert!(!reminders[0].completed);
        assert_eq!(reminders[0].due_date, "2024-01-20 10:00:00");
        assert_eq!(reminders[0].priority, 1);
    }

    #[test]
    fn parse_reminders_output_completed() {
        let raw = "<<<REM_START>>>Done task<<<SEP>>>Work<<<SEP>>>true<<<SEP>>>none<<<SEP>>>0<<<REM_END>>>";
        let reminders = parse_reminders_output(raw).map_err(|e| e.to_string()).unwrap();
        assert_eq!(reminders.len(), 1);
        assert!(reminders[0].completed);
    }

    #[test]
    fn parse_reminders_output_multiple() {
        let raw = "<<<REM_START>>>Task 1<<<SEP>>>List A<<<SEP>>>false<<<SEP>>>none<<<SEP>>>0<<<REM_END>>><<<REM_START>>>Task 2<<<SEP>>>List B<<<SEP>>>true<<<SEP>>>none<<<SEP>>>5<<<REM_END>>>";
        let reminders = parse_reminders_output(raw).map_err(|e| e.to_string()).unwrap();
        assert_eq!(reminders.len(), 2);
        assert_eq!(reminders[1].priority, 5);
    }

    #[test]
    fn parse_reminders_output_empty() {
        let reminders = parse_reminders_output("").map_err(|e| e.to_string()).unwrap();
        assert!(reminders.is_empty());
    }

    #[test]
    fn build_reminders_script_all() {
        let script = build_reminders_script(None);
        assert!(script.contains("set reminderItems to every reminder"));
        assert!(!script.contains("targetList"));
    }

    #[test]
    fn build_reminders_script_specific_list() {
        let script = build_reminders_script(Some("Shopping"));
        assert!(script.contains(r#"list "Shopping""#));
        assert!(script.contains("targetList"));
    }
}
