//! Extract notes from Apple Notes via osascript.

use regex::Regex;
use serde_json::{json, Value};

use super::{content_hash, run_osascript};
use crate::ingestion::IngestionError;

/// A single note extracted from Apple Notes.
pub struct Note {
    pub title: String,
    pub body: String,
    pub created_at: String,
    pub modified_at: String,
}

/// Extract all notes (or notes from a specific folder) from Apple Notes.
///
/// Returns a list of parsed [`Note`] structs.
pub fn extract(folder: Option<&str>) -> Result<Vec<Note>, IngestionError> {
    let script = build_script(folder);
    let raw = run_osascript(&script)?;
    parse_output(&raw)
}

/// Convert extracted notes into JSON records ready for ingestion.
pub fn to_json_records(notes: &[Note]) -> Vec<Value> {
    notes
        .iter()
        .map(|n| {
            let hash = content_hash(&n.body);
            json!({
                "title": n.title,
                "body": n.body,
                "created_at": n.created_at,
                "modified_at": n.modified_at,
                "content_hash": hash,
                "source": "apple_notes",
            })
        })
        .collect()
}

pub fn build_script(folder: Option<&str>) -> String {
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

pub fn parse_output(raw: &str) -> Result<Vec<Note>, IngestionError> {
    let re = Regex::new(
        r"<<<NOTE_START>>>(.*?)<<<SEP>>>(.*?)<<<SEP>>>(.*?)<<<SEP>>>(.*?)<<<NOTE_END>>>",
    )
    .map_err(|e| IngestionError::Extraction(format!("Regex error: {}", e)))?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_output_basic() {
        let raw = "<<<NOTE_START>>>My Title<<<SEP>>>This is the body of the note with enough text<<<SEP>>>2024-01-15 10:30:00<<<SEP>>>2024-01-16 14:00:00<<<NOTE_END>>>";
        let notes = parse_output(raw).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "My Title");
        assert_eq!(
            notes[0].body,
            "This is the body of the note with enough text"
        );
        assert_eq!(notes[0].created_at, "2024-01-15 10:30:00");
        assert_eq!(notes[0].modified_at, "2024-01-16 14:00:00");
    }

    #[test]
    fn parse_output_multiple() {
        let raw = "<<<NOTE_START>>>Note 1<<<SEP>>>Body 1<<<SEP>>>Date1<<<SEP>>>Date2<<<NOTE_END>>><<<NOTE_START>>>Note 2<<<SEP>>>Body 2<<<SEP>>>Date3<<<SEP>>>Date4<<<NOTE_END>>>";
        let notes = parse_output(raw).unwrap();
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].title, "Note 1");
        assert_eq!(notes[1].title, "Note 2");
    }

    #[test]
    fn parse_output_empty() {
        let notes = parse_output("").unwrap();
        assert!(notes.is_empty());
    }

    #[test]
    fn build_script_all_folders() {
        let script = build_script(None);
        assert!(script.contains("set noteList to every note"));
        assert!(!script.contains("targetFolder"));
    }

    #[test]
    fn build_script_specific_folder() {
        let script = build_script(Some("Work"));
        assert!(script.contains(r#"folder "Work""#));
        assert!(script.contains("targetFolder"));
    }

    #[test]
    fn to_json_records_produces_expected_fields() {
        let notes = vec![Note {
            title: "Test".to_string(),
            body: "Hello world content here".to_string(),
            created_at: "2024-01-01".to_string(),
            modified_at: "2024-01-02".to_string(),
        }];
        let records = to_json_records(&notes);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["title"], "Test");
        assert_eq!(records[0]["source"], "apple_notes");
        assert!(records[0]["content_hash"].is_string());
    }
}
