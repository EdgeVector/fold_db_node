//! Extract calendar events from Apple Calendar via osascript.

use regex::Regex;
use serde_json::{json, Value};

use super::{content_hash, run_osascript};
use crate::ingestion::IngestionError;

/// A single event extracted from Apple Calendar.
pub struct CalendarEvent {
    pub summary: String,
    pub start_time: String,
    pub end_time: String,
    pub location: String,
    pub description: String,
    pub calendar: String,
    pub all_day: bool,
    pub recurring: bool,
    /// Attendee email addresses extracted from the event. Empty
    /// when the event has no attendees or Calendar refuses access
    /// to the attendees list (some accounts — notably Exchange —
    /// block this). Downstream fingerprint extraction picks these
    /// up by running the text regex extractor over the attendees
    /// field alongside `description` and `summary`, so every
    /// attendee email becomes a Fingerprint + Mention connected
    /// via CoOccurrence edges to every other attendee of the
    /// same event.
    pub attendees: Vec<String>,
}

/// Extract all events (or events from a specific calendar) from Apple Calendar.
pub fn extract(calendar: Option<&str>) -> Result<Vec<CalendarEvent>, IngestionError> {
    let script = build_script(calendar);
    let raw = run_osascript(&script, "Calendar.app")?;
    parse_output(&raw)
}

/// Convert extracted calendar events into JSON records ready for ingestion.
pub fn to_json_records(events: &[CalendarEvent]) -> Vec<Value> {
    events
        .iter()
        .map(|e| {
            let hash_input = format!("{}|{}|{}", e.summary, e.start_time, e.calendar);
            let hash = content_hash(&hash_input);
            json!({
                "summary": e.summary,
                "start_time": e.start_time,
                "end_time": e.end_time,
                "location": e.location,
                "description": e.description,
                "calendar": e.calendar,
                "all_day": e.all_day,
                "recurring": e.recurring,
                "attendees": e.attendees,
                "content_hash": hash,
                "source": "apple_calendar",
            })
        })
        .collect()
}

pub fn build_script(calendar: Option<&str>) -> String {
    // Note on variable naming: AppleScript treats prepositions like
    // `at`, `in`, `of`, `to` as reserved parameter-name tokens.
    // Using them as loop variables (e.g. `repeat with at in list`)
    // fails with "-2741: Expected variable name or property but
    // found parameter name." All loop variables below are chosen
    // to avoid that collision.
    //
    // Note on calendar-name lookup: events collected across multiple
    // calendars lose their containing-calendar reference inside
    // AppleScript (error -1728 on `name of calendar of e`). We track
    // the calendar name in the outer loop instead of resolving it
    // from each event reference.
    let calendars_loop = match calendar {
        Some(name) => format!(
            r#"set targetCalendar to calendar "{}"
    set calName to name of targetCalendar
    set eventList to (every event of targetCalendar whose start date ≥ (current date) - 30 * days and start date ≤ (current date) + 90 * days)
    {event_body}"#,
            name.replace('"', "\\\""),
            event_body = event_body_block("calName")
        ),
        None => format!(
            r#"repeat with cal in calendars
        set calName to name of cal
        set eventList to (every event of cal whose start date ≥ (current date) - 30 * days and start date ≤ (current date) + 90 * days)
        {event_body}
    end repeat"#,
            event_body = event_body_block("calName")
        ),
    };

    format!(
        r#"tell application "Calendar"
    set output to ""
    {calendars_loop}
    return output
end tell"#
    )
}

/// Inner AppleScript snippet that iterates `eventList` and appends
/// one `<<<EVT_START>>>…<<<EVT_END>>>` record per event to `output`.
/// Accepts the name of the outer-scope variable holding the current
/// calendar's name (populated by the caller before this block runs).
fn event_body_block(cal_name_var: &str) -> String {
    format!(
        r#"repeat with e in eventList
        set eSummary to summary of e
        set eStart to (start date of e) as string
        set eEnd to (end date of e) as string
        set eAllDay to allday event of e
        set eRecurring to false
        try
            set eRecurrence to recurrence of e
            if eRecurrence is not missing value then set eRecurring to true
        end try
        try
            set eLocation to location of e
            if eLocation is missing value then set eLocation to ""
        on error
            set eLocation to ""
        end try
        try
            set eDescription to description of e
            if eDescription is missing value then set eDescription to ""
        on error
            set eDescription to ""
        end try
        -- Attendees. Some account types (especially Exchange) block
        -- AppleScript access to the attendees list; we swallow the
        -- error and emit an empty string in that case. Attendee
        -- emails are comma-separated; the Rust parser splits and
        -- trims them.
        set eAttendees to ""
        try
            set attendeeList to attendees of e
            repeat with anAttendee in attendeeList
                try
                    set attendeeEmail to email of anAttendee
                    if attendeeEmail is not missing value and attendeeEmail is not "" then
                        if eAttendees is "" then
                            set eAttendees to attendeeEmail
                        else
                            set eAttendees to eAttendees & "," & attendeeEmail
                        end if
                    end if
                end try
            end repeat
        end try
        set eCalName to {cal_name_var}
        set output to output & "<<<EVT_START>>>" & eSummary & "<<<SEP>>>" & eStart & "<<<SEP>>>" & eEnd & "<<<SEP>>>" & eLocation & "<<<SEP>>>" & eDescription & "<<<SEP>>>" & eCalName & "<<<SEP>>>" & eAllDay & "<<<SEP>>>" & eRecurring & "<<<SEP>>>" & eAttendees & "<<<EVT_END>>>"
    end repeat"#
    )
}

pub fn parse_output(raw: &str) -> Result<Vec<CalendarEvent>, IngestionError> {
    // The AppleScript emits one `<<<EVT_START>>>…<<<EVT_END>>>` block
    // per event. We scan for full records first, then split each
    // record on `<<<SEP>>>` to get its fields. This avoids a subtle
    // bug with a single multi-field regex: with non-greedy `.*?`
    // captures, a 9-SEP regex will happily match across the
    // boundary between two 8-SEP records in the legacy format.
    //
    // Splitting per-record keeps us correctly agnostic to field
    // count — 8 fields means legacy (attendees: empty), 9 fields
    // means the new attendees-aware format.
    let record_re = Regex::new(r"<<<EVT_START>>>(.*?)<<<EVT_END>>>")
        .map_err(|e| IngestionError::Extraction(format!("Regex error: {}", e)))?;

    let mut events = Vec::new();
    for cap in record_re.captures_iter(raw) {
        let body = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let fields: Vec<&str> = body.split("<<<SEP>>>").collect();
        // Accept 8 (legacy) or 9 (with attendees) fields. Anything
        // else is a malformed record; skip silently because callers
        // must never abort ingestion on one bad row.
        if fields.len() < 8 || fields.len() > 9 {
            tracing::warn!(
                "apple_calendar.parse_output: skipping malformed record with {} fields",
                fields.len()
            );
            continue;
        }
        let all_day = fields[6].trim().to_lowercase() == "true";
        let recurring = fields[7].trim().to_lowercase() == "true";
        let attendees = if fields.len() == 9 {
            parse_attendees(fields[8].trim())
        } else {
            Vec::new()
        };
        events.push(CalendarEvent {
            summary: fields[0].trim().to_string(),
            start_time: fields[1].trim().to_string(),
            end_time: fields[2].trim().to_string(),
            location: fields[3].trim().to_string(),
            description: fields[4].trim().to_string(),
            calendar: fields[5].trim().to_string(),
            all_day,
            recurring,
            attendees,
        });
    }
    Ok(events)
}

/// Parse the comma-separated attendee email list emitted by the
/// AppleScript. Empty input → empty vec. Whitespace and empty
/// entries are dropped. Duplicates are preserved; downstream
/// content-keyed fingerprint dedup handles collapsing to one
/// Fingerprint per unique email.
fn parse_attendees(raw: &str) -> Vec<String> {
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
        let raw = "<<<EVT_START>>>Team Standup<<<SEP>>>2026-03-28 09:00:00<<<SEP>>>2026-03-28 09:15:00<<<SEP>>>Zoom<<<SEP>>>Daily sync<<<SEP>>>Work<<<SEP>>>false<<<SEP>>>true<<<EVT_END>>>";
        let events = parse_output(raw).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].summary, "Team Standup");
        assert_eq!(events[0].start_time, "2026-03-28 09:00:00");
        assert_eq!(events[0].end_time, "2026-03-28 09:15:00");
        assert_eq!(events[0].location, "Zoom");
        assert_eq!(events[0].description, "Daily sync");
        assert_eq!(events[0].calendar, "Work");
        assert!(!events[0].all_day);
        assert!(events[0].recurring);
    }

    #[test]
    fn parse_output_all_day_event() {
        let raw = "<<<EVT_START>>>Company Holiday<<<SEP>>>2026-03-28 00:00:00<<<SEP>>>2026-03-29 00:00:00<<<SEP>>><<<SEP>>><<<SEP>>>Personal<<<SEP>>>true<<<SEP>>>false<<<EVT_END>>>";
        let events = parse_output(raw).unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].all_day);
        assert!(!events[0].recurring);
        assert!(events[0].location.is_empty());
        assert!(events[0].description.is_empty());
    }

    #[test]
    fn parse_output_multiple() {
        let raw = "<<<EVT_START>>>Event 1<<<SEP>>>Start1<<<SEP>>>End1<<<SEP>>>Loc1<<<SEP>>>Desc1<<<SEP>>>Cal1<<<SEP>>>false<<<SEP>>>false<<<EVT_END>>><<<EVT_START>>>Event 2<<<SEP>>>Start2<<<SEP>>>End2<<<SEP>>>Loc2<<<SEP>>>Desc2<<<SEP>>>Cal2<<<SEP>>>true<<<SEP>>>true<<<EVT_END>>>";
        let events = parse_output(raw).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].summary, "Event 1");
        assert_eq!(events[1].summary, "Event 2");
        assert!(events[1].all_day);
        assert!(events[1].recurring);
    }

    #[test]
    fn parse_output_empty() {
        let events = parse_output("").unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn build_script_all_calendars() {
        let script = build_script(None);
        assert!(script.contains("repeat with cal in calendars"));
        assert!(!script.contains("targetCalendar"));
    }

    #[test]
    fn build_script_specific_calendar() {
        let script = build_script(Some("Work"));
        assert!(script.contains(r#"calendar "Work""#));
        assert!(script.contains("targetCalendar"));
    }

    #[test]
    fn to_json_records_produces_expected_fields() {
        let events = vec![CalendarEvent {
            summary: "Test Event".to_string(),
            start_time: "2026-03-28 10:00:00".to_string(),
            end_time: "2026-03-28 11:00:00".to_string(),
            location: "Office".to_string(),
            description: "A test event".to_string(),
            calendar: "Work".to_string(),
            all_day: false,
            recurring: false,
            attendees: vec!["tom@acme.com".to_string(), "alice@acme.com".to_string()],
        }];
        let records = to_json_records(&events);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["summary"], "Test Event");
        assert_eq!(records[0]["source"], "apple_calendar");
        assert_eq!(records[0]["all_day"], false);
        assert_eq!(records[0]["recurring"], false);
        assert!(records[0]["content_hash"].is_string());
        assert_eq!(records[0]["attendees"][0], "tom@acme.com");
        assert_eq!(records[0]["attendees"][1], "alice@acme.com");
    }

    // ── Attendee extraction tests ───────────────────────────────

    #[test]
    fn parse_output_extracts_attendees() {
        let raw = "<<<EVT_START>>>Planning<<<SEP>>>2026-03-28 10:00:00<<<SEP>>>2026-03-28 11:00:00<<<SEP>>>Zoom<<<SEP>>>Quarterly planning<<<SEP>>>Work<<<SEP>>>false<<<SEP>>>false<<<SEP>>>tom@acme.com,alice@acme.com,bob@example.com<<<EVT_END>>>";
        let events = parse_output(raw).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].attendees,
            vec!["tom@acme.com", "alice@acme.com", "bob@example.com"]
        );
    }

    #[test]
    fn parse_output_handles_empty_attendees_in_v9_format() {
        let raw = "<<<EVT_START>>>Solo<<<SEP>>>2026-03-28 10:00:00<<<SEP>>>2026-03-28 11:00:00<<<SEP>>><<<SEP>>><<<SEP>>>Personal<<<SEP>>>false<<<SEP>>>false<<<SEP>>><<<EVT_END>>>";
        let events = parse_output(raw).unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].attendees.is_empty());
    }

    #[test]
    fn parse_output_legacy_8_field_format_still_parses_with_empty_attendees() {
        // This is the pre-attendees format some older AppleScript
        // output or goldens may still produce. Parser falls back to
        // 8-field regex and leaves attendees empty.
        let raw = "<<<EVT_START>>>Legacy<<<SEP>>>2026-03-28 09:00:00<<<SEP>>>2026-03-28 09:15:00<<<SEP>>>Zoom<<<SEP>>>Daily sync<<<SEP>>>Work<<<SEP>>>false<<<SEP>>>true<<<EVT_END>>>";
        let events = parse_output(raw).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].summary, "Legacy");
        assert!(events[0].attendees.is_empty());
    }

    #[test]
    fn parse_output_trims_attendee_whitespace_and_drops_empties() {
        let raw = "<<<EVT_START>>>x<<<SEP>>>s<<<SEP>>>e<<<SEP>>><<<SEP>>><<<SEP>>>c<<<SEP>>>false<<<SEP>>>false<<<SEP>>> tom@acme.com , ,alice@acme.com ,<<<EVT_END>>>";
        let events = parse_output(raw).unwrap();
        assert_eq!(events[0].attendees, vec!["tom@acme.com", "alice@acme.com"]);
    }

    #[test]
    fn build_script_includes_attendee_block() {
        // The AppleScript must extract the email for each attendee
        // and emit them comma-separated as the 9th field. Regression
        // guard — if someone reformats the script, tests catch a loss
        // of attendee extraction before it hits a dogfood node.
        let script = build_script(None);
        assert!(script.contains("attendees of e"));
        assert!(script.contains("email of anAttendee"));
        assert!(script.contains("& eAttendees"));
    }

    #[test]
    fn build_script_avoids_reserved_applescript_prepositions_as_loop_vars() {
        // AppleScript fails with `-2741: Expected variable name or
        // property but found parameter name` when a preposition like
        // `at`, `in`, `of`, `to`, `by`, `from` is used as a loop
        // variable (e.g. `repeat with at in list`). This regression
        // guards both the `repeat with <var> in ...` and
        // `set <var> to ...` forms.
        let script = build_script(None);
        for bad in [
            "repeat with at in",
            "repeat with in in",
            "repeat with of in",
            "repeat with to in",
            "repeat with by in",
            "repeat with from in",
            "set at to",
            "set of to",
            "set to to",
        ] {
            assert!(
                !script.contains(bad),
                "AppleScript uses reserved preposition as a variable: {bad:?}\nscript:\n{script}"
            );
        }
    }

    #[test]
    fn build_script_tracks_calendar_name_in_outer_loop() {
        // Events collected across calendars lose their containing
        // calendar reference in AppleScript (error -1728 on
        // `name of calendar of e`). The script must capture the
        // calendar's name in the enclosing loop and reuse it per
        // event instead of dereferencing it through the event ref.
        let all = build_script(None);
        assert!(!all.contains("name of calendar of e"));
        assert!(all.contains("set calName to name of cal"));
        assert!(all.contains("set eCalName to calName"));

        let specific = build_script(Some("Work"));
        assert!(!specific.contains("name of calendar of e"));
        assert!(specific.contains("set calName to name of targetCalendar"));
        assert!(specific.contains("set eCalName to calName"));
    }
}
