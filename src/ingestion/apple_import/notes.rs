//! Extract notes from Apple Notes via osascript.
//!
//! Each note carries its stable Apple-native id (`id of note`, a
//! `x-coredata://…/ICNote/pN` URL that survives launches and edits).
//! Records omit any precomputed content_hash, so the forced-schema
//! builder (`build_forced_schema_response`) picks `apple_note_id` as
//! the schema's `hash_field` — two notes with identical bodies stay
//! distinct under the storage key.

use serde_json::{json, Value};

use super::{parse_records, run_extract};
use crate::ingestion::IngestionError;

/// A single note extracted from Apple Notes.
pub struct Note {
    /// Apple-native stable id, e.g. `x-coredata://…/ICNote/p137`. Survives
    /// edits and relaunches; used as the schema's primary key.
    pub id: String,
    pub title: String,
    pub body: String,
    pub created_at: String,
    pub modified_at: String,
}

/// Extract all notes (or notes from a specific folder) from Apple Notes.
///
/// Returns a list of parsed [`Note`] structs.
pub fn extract(folder: Option<&str>) -> Result<Vec<Note>, IngestionError> {
    run_extract("Notes.app", &build_script(folder), parse_output)
}

/// Convert extracted notes into JSON records ready for ingestion.
///
/// Notes whose AppleScript id is empty are dropped: the schema pins
/// `hash_field = apple_note_id`, so an empty id would collapse every
/// such record onto the same storage key (the exact collision class
/// this whole change exists to prevent).
pub fn to_json_records(notes: &[Note]) -> Vec<Value> {
    notes
        .iter()
        .filter(|n| !n.id.is_empty())
        .map(|n| {
            json!({
                "apple_note_id": n.id,
                "title": n.title,
                "body": n.body,
                "created_at": n.created_at,
                "modified_at": n.modified_at,
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
            set noteId to (id of n) as string
            set noteTitle to name of n
            set noteCreated to (creation date of n) as string
            set noteModified to (modification date of n) as string
            set output to output & "<<<NOTE_START>>>" & noteId & "<<<SEP>>>" & noteTitle & "<<<SEP>>>" & noteBody & "<<<SEP>>>" & noteCreated & "<<<SEP>>>" & noteModified & "<<<NOTE_END>>>"
        end if
    end repeat
    return output
end tell"#
    )
}

pub fn parse_output(raw: &str) -> Result<Vec<Note>, IngestionError> {
    parse_records(raw, "NOTE", |fields| {
        if fields.len() != 5 {
            return None;
        }
        Some(Note {
            id: fields[0].trim().to_string(),
            title: fields[1].trim().to_string(),
            body: fields[2].trim().to_string(),
            created_at: fields[3].trim().to_string(),
            modified_at: fields[4].trim().to_string(),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: &str, title: &str, body: &str) -> Note {
        Note {
            id: id.to_string(),
            title: title.to_string(),
            body: body.to_string(),
            created_at: "2024-01-01".to_string(),
            modified_at: "2024-01-02".to_string(),
        }
    }

    #[test]
    fn parse_output_basic() {
        let raw = "<<<NOTE_START>>>x-coredata://AC8E/ICNote/p137<<<SEP>>>My Title<<<SEP>>>This is the body of the note with enough text<<<SEP>>>2024-01-15 10:30:00<<<SEP>>>2024-01-16 14:00:00<<<NOTE_END>>>";
        let notes = parse_output(raw).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].id, "x-coredata://AC8E/ICNote/p137");
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
        let raw = "<<<NOTE_START>>>id1<<<SEP>>>Note 1<<<SEP>>>Body 1<<<SEP>>>Date1<<<SEP>>>Date2<<<NOTE_END>>><<<NOTE_START>>>id2<<<SEP>>>Note 2<<<SEP>>>Body 2<<<SEP>>>Date3<<<SEP>>>Date4<<<NOTE_END>>>";
        let notes = parse_output(raw).unwrap();
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].id, "id1");
        assert_eq!(notes[0].title, "Note 1");
        assert_eq!(notes[1].id, "id2");
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
    fn build_script_includes_note_id_extraction() {
        let script = build_script(None);
        assert!(
            script.contains("(id of n) as string"),
            "script must extract the Apple-native note id: {script}"
        );
        assert!(
            script.contains("& noteId & \"<<<SEP>>>\" & noteTitle"),
            "id must be the first field in the serialized record so parse_output's regex matches",
        );
    }

    #[test]
    fn to_json_records_emits_apple_note_id_not_content_hash() {
        let records = to_json_records(&[note("id-abc", "Test", "Hello world content here")]);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["apple_note_id"], "id-abc");
        assert_eq!(records[0]["title"], "Test");
        assert_eq!(records[0]["source"], "apple_notes");
        // content_hash must NOT be precomputed in the record — the
        // schema's hash_field is apple_note_id, and content_hash is
        // injected post-AI-analysis by inject_content_hash.
        assert!(
            records[0].get("content_hash").is_none(),
            "to_json_records must not emit content_hash: {}",
            records[0]
        );
    }

    /// Regression: identical body + identical timestamps must still
    /// produce two distinct ingestion records as long as the Apple
    /// note ids differ. Before the apple_note_id switch, both records
    /// hashed to the same key and one silently overwrote the other.
    #[test]
    fn identical_body_with_distinct_ids_yields_two_records_with_distinct_keys() {
        let records = to_json_records(&[
            note(
                "id-1",
                "same title",
                "identical body content that is long enough",
            ),
            note(
                "id-2",
                "same title",
                "identical body content that is long enough",
            ),
        ]);
        assert_eq!(records.len(), 2);
        assert_ne!(
            records[0]["apple_note_id"], records[1]["apple_note_id"],
            "the two records must keep their distinct Apple ids"
        );
        // Confirm the records are not collapsed into one by serde_json
        // duplicate-key dedup or by any future dedup-by-body filter.
        assert_eq!(records[0]["body"], records[1]["body"]);
    }

    /// Records whose AppleScript id came back empty must be dropped
    /// rather than emitted: with `hash_field = apple_note_id`, an
    /// empty id would collide every such record onto the same key.
    #[test]
    fn to_json_records_drops_notes_with_empty_id() {
        let records = to_json_records(&[
            note(
                "",
                "blank id note",
                "this one should be dropped because of empty id",
            ),
            note("id-2", "kept", "this one should be kept"),
        ]);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["apple_note_id"], "id-2");
    }
}
