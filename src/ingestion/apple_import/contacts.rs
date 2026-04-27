//! Extract contacts from Apple Contacts (Address Book) via osascript.
//!
//! ## Field shape
//!
//! Each contact emits:
//!   - `full_name` — display name (empty → row is skipped; Contacts.app seeds
//!     many no-name placeholders that have no searchable content).
//!   - `organization` — "" when missing.
//!   - `emails` — all email addresses as a comma-joined string in AppleScript
//!     output; split into `Vec<String>` on the Rust side.
//!   - `phones` — same shape as `emails`, for phone numbers.
//!   - `birthday` — "" when missing.
//!   - `note` — "" when missing.
//!
//! ## AppleScript notes
//!
//! - `people` is the singular iterator on Contacts.app. `whose` filters cannot
//!   be combined with property bulk-reads the way Reminders does, so we iterate
//!   one record at a time and wrap each property access in a `try` to cope
//!   with the occasional "missing value" or permission error.
//! - Emails and phones are each emitted as comma-joined lists. Downstream
//!   fingerprint extraction for emails/phones will re-tokenize as needed; the
//!   core schema only needs the text.
//! - First-run without Automation permission is reported back through
//!   `IngestionError::Extraction` (see `run_osascript`'s timeout/permission
//!   path in `mod.rs`).

use regex::Regex;
use serde_json::{json, Value};

use super::{content_hash, run_osascript};
use crate::ingestion::IngestionError;

/// A single contact extracted from Apple Contacts.
pub struct Contact {
    pub full_name: String,
    pub organization: String,
    pub emails: Vec<String>,
    pub phones: Vec<String>,
    pub birthday: String,
    pub note: String,
}

/// Extract all contacts from Apple Contacts.
///
/// Rows with an empty `full_name` are dropped — Contacts.app seeds many
/// no-name placeholders that have no searchable content and would only pollute
/// the molecule store with empty-key rows.
pub fn extract() -> Result<Vec<Contact>, IngestionError> {
    let script = build_script();
    let raw = run_osascript(&script, "Contacts.app")?;
    parse_output(&raw)
}

/// Convert extracted contacts into JSON records ready for ingestion.
pub fn to_json_records(contacts: &[Contact]) -> Vec<Value> {
    contacts
        .iter()
        .map(|c| {
            let hash_input = format!(
                "{}|{}|{}",
                c.full_name,
                c.emails.join(","),
                c.phones.join(",")
            );
            let hash = content_hash(&hash_input);
            json!({
                "full_name": c.full_name,
                "organization": c.organization,
                "emails": c.emails,
                "phones": c.phones,
                "birthday": c.birthday,
                "note": c.note,
                "content_hash": hash,
                "source": "apple_contacts",
            })
        })
        .collect()
}

pub fn build_script() -> String {
    // We iterate `every person` and read each property inside a `try` so the
    // whole export doesn't abort on a single malformed row. Emails and phones
    // are serialised as comma-joined lists — the Rust parser splits on `,`.
    //
    // `<<<CON_START>>>` / `<<<SEP>>>` / `<<<CON_END>>>` mirror the framing
    // used by notes, reminders, and calendar so the same parser strategy
    // applies (scan record boundaries first, then split each record into
    // fields).
    r#"tell application "Contacts"
    set output to ""
    repeat with p in every person
        try
            set fullName to name of p
        on error
            set fullName to ""
        end try
        if fullName is missing value then set fullName to ""
        if fullName is not "" then
            try
                set orgName to organization of p
                if orgName is missing value then set orgName to ""
            on error
                set orgName to ""
            end try
            set emailStr to ""
            try
                repeat with anEmail in emails of p
                    try
                        set eVal to value of anEmail
                        if eVal is not missing value and eVal is not "" then
                            if emailStr is "" then
                                set emailStr to eVal
                            else
                                set emailStr to emailStr & "," & eVal
                            end if
                        end if
                    end try
                end repeat
            end try
            set phoneStr to ""
            try
                repeat with aPhone in phones of p
                    try
                        set pVal to value of aPhone
                        if pVal is not missing value and pVal is not "" then
                            if phoneStr is "" then
                                set phoneStr to pVal
                            else
                                set phoneStr to phoneStr & "," & pVal
                            end if
                        end if
                    end try
                end repeat
            end try
            set bday to ""
            try
                set bd to birth date of p
                if bd is not missing value then set bday to bd as string
            end try
            set noteText to ""
            try
                set noteText to note of p
                if noteText is missing value then set noteText to ""
            end try
            set output to output & "<<<CON_START>>>" & fullName & "<<<SEP>>>" & orgName & "<<<SEP>>>" & emailStr & "<<<SEP>>>" & phoneStr & "<<<SEP>>>" & bday & "<<<SEP>>>" & noteText & "<<<CON_END>>>"
        end if
    end repeat
    return output
end tell"#
        .to_string()
}

pub fn parse_output(raw: &str) -> Result<Vec<Contact>, IngestionError> {
    // Scan record boundaries first, then split each record on `<<<SEP>>>`.
    // Splitting per-record keeps us agnostic to future field additions and
    // avoids the multi-field-regex cross-record match bug documented in the
    // calendar parser.
    let record_re = Regex::new(r"<<<CON_START>>>(.*?)<<<CON_END>>>")
        .map_err(|e| IngestionError::Extraction(format!("Regex error: {}", e)))?;

    let mut contacts = Vec::new();
    for cap in record_re.captures_iter(raw) {
        let body = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let fields: Vec<&str> = body.split("<<<SEP>>>").collect();
        if fields.len() != 6 {
            tracing::warn!(
                "apple_contacts.parse_output: skipping malformed record with {} fields",
                fields.len()
            );
            continue;
        }
        let full_name = fields[0].trim().to_string();
        if full_name.is_empty() {
            continue;
        }
        contacts.push(Contact {
            full_name,
            organization: fields[1].trim().to_string(),
            emails: parse_multi(fields[2].trim()),
            phones: parse_multi(fields[3].trim()),
            birthday: fields[4].trim().to_string(),
            note: fields[5].trim().to_string(),
        });
    }
    Ok(contacts)
}

/// Parse a comma-separated list of values. Empty input → empty vec. Empty
/// entries and surrounding whitespace are dropped. Order is preserved.
fn parse_multi(raw: &str) -> Vec<String> {
    if raw.is_empty() {
        return Vec::new();
    }
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_output_basic() {
        let raw = "<<<CON_START>>>Alice Example<<<SEP>>>Acme Corp<<<SEP>>>alice@example.com<<<SEP>>>+1-555-0100<<<SEP>>>1990-01-15<<<SEP>>>met at conference<<<CON_END>>>";
        let contacts = parse_output(raw).unwrap();
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].full_name, "Alice Example");
        assert_eq!(contacts[0].organization, "Acme Corp");
        assert_eq!(contacts[0].emails, vec!["alice@example.com"]);
        assert_eq!(contacts[0].phones, vec!["+1-555-0100"]);
        assert_eq!(contacts[0].birthday, "1990-01-15");
        assert_eq!(contacts[0].note, "met at conference");
    }

    #[test]
    fn parse_output_multiple_emails_and_phones() {
        let raw = "<<<CON_START>>>Bob Example<<<SEP>>><<<SEP>>>bob@work.com,bob@home.com<<<SEP>>>+1-555-0101,+1-555-0102<<<SEP>>><<<SEP>>><<<CON_END>>>";
        let contacts = parse_output(raw).unwrap();
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].emails, vec!["bob@work.com", "bob@home.com"]);
        assert_eq!(contacts[0].phones, vec!["+1-555-0101", "+1-555-0102"]);
        assert!(contacts[0].organization.is_empty());
        assert!(contacts[0].birthday.is_empty());
        assert!(contacts[0].note.is_empty());
    }

    #[test]
    fn parse_output_skips_empty_name() {
        // Contacts with no display name would index at an empty range key and
        // pollute the store. The extractor's AppleScript already drops them;
        // this is belt-and-suspenders on the Rust side.
        let raw = "<<<CON_START>>><<<SEP>>>Acme<<<SEP>>>nobody@acme.com<<<SEP>>><<<SEP>>><<<SEP>>><<<CON_END>>>";
        let contacts = parse_output(raw).unwrap();
        assert!(contacts.is_empty());
    }

    #[test]
    fn parse_output_skips_malformed_field_count() {
        // 5 fields instead of 6.
        let raw = "<<<CON_START>>>Short<<<SEP>>>Acme<<<SEP>>><<<SEP>>><<<SEP>>><<<CON_END>>>";
        let contacts = parse_output(raw).unwrap();
        assert!(contacts.is_empty());
    }

    #[test]
    fn parse_output_multiple() {
        let raw = "<<<CON_START>>>Alice<<<SEP>>>A<<<SEP>>>a@x.com<<<SEP>>>+1<<<SEP>>><<<SEP>>><<<CON_END>>><<<CON_START>>>Bob<<<SEP>>>B<<<SEP>>>b@x.com<<<SEP>>>+2<<<SEP>>><<<SEP>>><<<CON_END>>>";
        let contacts = parse_output(raw).unwrap();
        assert_eq!(contacts.len(), 2);
        assert_eq!(contacts[0].full_name, "Alice");
        assert_eq!(contacts[1].full_name, "Bob");
    }

    #[test]
    fn parse_output_empty() {
        let contacts = parse_output("").unwrap();
        assert!(contacts.is_empty());
    }

    #[test]
    fn parse_multi_trims_whitespace_and_drops_empties() {
        assert_eq!(parse_multi(""), Vec::<String>::new());
        assert_eq!(
            parse_multi(" a@x.com , ,b@x.com ,"),
            vec!["a@x.com", "b@x.com"]
        );
    }

    #[test]
    fn to_json_records_produces_expected_fields() {
        let contacts = vec![Contact {
            full_name: "Alice Example".to_string(),
            organization: "Acme".to_string(),
            emails: vec!["alice@example.com".to_string()],
            phones: vec!["+1-555-0100".to_string()],
            birthday: "1990-01-15".to_string(),
            note: "".to_string(),
        }];
        let records = to_json_records(&contacts);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["full_name"], "Alice Example");
        assert_eq!(records[0]["organization"], "Acme");
        assert_eq!(records[0]["source"], "apple_contacts");
        assert_eq!(records[0]["emails"][0], "alice@example.com");
        assert_eq!(records[0]["phones"][0], "+1-555-0100");
        assert_eq!(records[0]["birthday"], "1990-01-15");
        assert!(records[0]["content_hash"].is_string());
    }

    #[test]
    fn to_json_records_hash_distinguishes_distinct_contacts() {
        let alice = Contact {
            full_name: "Alice".to_string(),
            organization: "".to_string(),
            emails: vec!["a@x.com".to_string()],
            phones: vec![],
            birthday: "".to_string(),
            note: "".to_string(),
        };
        let bob = Contact {
            full_name: "Bob".to_string(),
            organization: "".to_string(),
            emails: vec!["b@x.com".to_string()],
            phones: vec![],
            birthday: "".to_string(),
            note: "".to_string(),
        };
        let records = to_json_records(&[alice, bob]);
        assert_ne!(records[0]["content_hash"], records[1]["content_hash"]);
    }

    #[test]
    fn build_script_iterates_people_and_serialises_fields() {
        let script = build_script();
        assert!(script.contains(r#"tell application "Contacts""#));
        assert!(script.contains("repeat with p in every person"));
        assert!(script.contains("emails of p"));
        assert!(script.contains("phones of p"));
        assert!(script.contains("<<<CON_START>>>"));
        assert!(script.contains("<<<CON_END>>>"));
        assert!(script.contains("<<<SEP>>>"));
    }

    #[test]
    fn build_script_skips_empty_name_rows_in_applescript_too() {
        // Defense-in-depth: the Rust parser already drops empty-name rows,
        // but the AppleScript should not even emit them — otherwise every
        // iCloud auto-generated placeholder contact burns an IPC round-trip.
        let script = build_script();
        assert!(script.contains("if fullName is not \"\" then"));
    }
}
