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

fn content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let result = hasher.finalize();
    hex::encode(&result[..8])
}

fn run_osascript(script: &str) -> Result<String, CliError> {
    let output = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|e| CliError::new(format!("Failed to run osascript: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::new(format!("osascript failed: {}", stderr)));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
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
) -> Result<CommandOutput, CliError> {
    eprintln!("Exporting notes from Apple Notes...");
    let script = build_notes_script(folder);
    let raw = run_osascript(&script)?;

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

fn build_reminders_script(list: Option<&str>) -> String {
    let list_filter = match list {
        Some(name) => format!(
            r#"set targetList to list "{}"
            set reminderList to every reminder of targetList"#,
            name.replace('"', "\\\"")
        ),
        None => "set reminderList to every reminder".to_string(),
    };

    format!(
        r#"tell application "Reminders"
    {list_filter}
    set output to ""
    repeat with r in reminderList
        set reminderName to name of r
        set reminderBody to body of r
        if reminderBody is missing value then set reminderBody to ""
        set reminderDue to ""
        try
            set reminderDue to (due date of r) as string
        end try
        set reminderCompleted to completed of r
        set output to output & "<<<REM_START>>>" & reminderName & "<<<SEP>>>" & reminderBody & "<<<SEP>>>" & reminderDue & "<<<SEP>>>" & reminderCompleted & "<<<REM_END>>>"
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
) -> Result<CommandOutput, CliError> {
    eprintln!("Exporting reminders from Apple Reminders...");
    let script = build_reminders_script(list);
    let raw = run_osascript(&script)?;

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

    client.ingest_process(&records).await?;

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
) -> Result<CommandOutput, CliError> {
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

    let raw = run_osascript(&script)?;

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
}
