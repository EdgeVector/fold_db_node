use crate::commands::apple::{build_client, content_hash, post_ingestion_batch, run_osascript};
use crate::commands::CommandOutput;
use crate::error::CliError;
use crate::output::spinner;
use crate::output::OutputMode;
use regex::Regex;
use serde_json::json;

pub async fn run(
    folder: Option<&str>,
    batch_size: usize,
    user_hash: &str,
    mode: OutputMode,
) -> Result<CommandOutput, CliError> {
    let sp = if mode == OutputMode::Human {
        Some(spinner::new_spinner("Exporting notes from Apple Notes..."))
    } else {
        None
    };

    let script = build_notes_script(folder);
    let raw = run_osascript(&script)?;

    if let Some(ref pb) = sp {
        spinner::finish_spinner(pb, "Notes exported");
    }

    let notes = parse_notes_output(&raw)?;
    if notes.is_empty() {
        return Err(CliError::new("No notes found in Apple Notes"));
    }

    let total = notes.len();
    let (client, base_url) = build_client(user_hash)?;

    let pb = if mode == OutputMode::Human {
        Some(spinner::new_progress_bar(total as u64, "Ingesting notes"))
    } else {
        None
    };

    let mut all_ids = Vec::new();
    for (i, chunk) in notes.chunks(batch_size).enumerate() {
        let records: Vec<serde_json::Value> = chunk
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
            .collect();

        let ids = post_ingestion_batch(&client, &base_url, records).await?;
        all_ids.extend(ids);

        if let Some(ref pb) = pb {
            let pos = ((i + 1) * chunk.len()).min(total) as u64;
            pb.set_position(pos);
        }
    }

    if let Some(ref pb) = pb {
        pb.finish_and_clear();
    }

    Ok(CommandOutput::AppleIngestSuccess {
        source: "apple_notes".to_string(),
        total,
        ingested: all_ids.len(),
        ids: all_ids,
    })
}

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
    let re = Regex::new(r"<<<NOTE_START>>>(.*?)<<<SEP>>>(.*?)<<<SEP>>>(.*?)<<<SEP>>>(.*?)<<<NOTE_END>>>")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_notes_output_basic() {
        let raw = "<<<NOTE_START>>>My Title<<<SEP>>>This is the body of the note with enough text<<<SEP>>>2024-01-15 10:30:00<<<SEP>>>2024-01-16 14:00:00<<<NOTE_END>>>";
        let notes = parse_notes_output(raw).map_err(|e| e.to_string()).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "My Title");
        assert_eq!(notes[0].body, "This is the body of the note with enough text");
        assert_eq!(notes[0].created_at, "2024-01-15 10:30:00");
        assert_eq!(notes[0].modified_at, "2024-01-16 14:00:00");
    }

    #[test]
    fn parse_notes_output_multiple() {
        let raw = "<<<NOTE_START>>>Note 1<<<SEP>>>Body 1<<<SEP>>>Date1<<<SEP>>>Date2<<<NOTE_END>>><<<NOTE_START>>>Note 2<<<SEP>>>Body 2<<<SEP>>>Date3<<<SEP>>>Date4<<<NOTE_END>>>";
        let notes = parse_notes_output(raw).map_err(|e| e.to_string()).unwrap();
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].title, "Note 1");
        assert_eq!(notes[1].title, "Note 2");
    }

    #[test]
    fn parse_notes_output_empty() {
        let notes = parse_notes_output("").map_err(|e| e.to_string()).unwrap();
        assert!(notes.is_empty());
    }

    #[test]
    fn build_notes_script_all_folders() {
        let script = build_notes_script(None);
        assert!(script.contains("set noteList to every note"));
        assert!(!script.contains("targetFolder"));
    }

    #[test]
    fn build_notes_script_specific_folder() {
        let script = build_notes_script(Some("Work"));
        assert!(script.contains(r#"folder "Work""#));
        assert!(script.contains("targetFolder"));
    }
}
