//! Extract events from Apple Calendar via osascript.

use regex::Regex;
use serde_json::{json, Value};

use super::{content_hash, run_osascript};
use crate::ingestion::IngestionError;

/// A single event extracted from Apple Calendar.
pub struct Event {
    pub title: String,
    pub start_date: String,
    pub end_date: String,
    pub location: String,
    pub notes: String,
    pub calendar_name: String,
    pub attendees: String,
    pub is_all_day: bool,
    pub recurrence_rule: String,
}

/// Extract all events (or from a specific calendar) from Apple Calendar.
pub fn extract(calendar: Option<&str>) -> Result<Vec<Event>, IngestionError> {
    let script = build_script(calendar);
    let raw = run_osascript(&script)?;
    parse_output(&raw)
}

/// Convert extracted events into JSON records ready for ingestion.
pub fn to_json_records(events: &[Event]) -> Vec<Value> {
    events
        .iter()
        .map(|e| {
            let hash = content_hash(&format!("{}{}{}", e.title, e.start_date, e.calendar_name));
            json!({
                "title": e.title,
                "start_date": e.start_date,
                "end_date": e.end_date,
                "location": e.location,
                "notes": e.notes,
                "calendar_name": e.calendar_name,
                "attendees": e.attendees,
                "is_all_day": e.is_all_day,
                "recurrence_rule": e.recurrence_rule,
                "content_hash": hash,
                "source": "apple_calendar",
            })
        })
        .collect()
}

fn build_script(calendar: Option<&str>) -> String {
    let calendar_filter = match calendar {
        Some(name) => format!(
            r#"set targetCalendar to calendar "{}"
    set eventList to every event of targetCalendar"#,
            name.replace('"', "\\\"")
        ),
        None => {
            // Collect events from all calendars
            r#"set eventList to {}
    repeat with cal in calendars
        set eventList to eventList & (every event of cal)
    end repeat"#
                .to_string()
        }
    };

    format!(
        r#"tell application "Calendar"
    {calendar_filter}
    set output to ""
    repeat with e in eventList
        set eTitle to summary of e
        set eStart to (start date of e) as string
        set eEnd to (end date of e) as string
        try
            set eLoc to location of e
            if eLoc is missing value then set eLoc to "none"
        on error
            set eLoc to "none"
        end try
        try
            set eNotes to description of e
            if eNotes is missing value then set eNotes to "none"
        on error
            set eNotes to "none"
        end try
        set eCalName to name of calendar of e
        try
            set eAttendees to ""
            set attendeeList to attendees of e
            repeat with a in attendeeList
                if eAttendees is not "" then set eAttendees to eAttendees & ", "
                set eAttendees to eAttendees & (display name of a)
            end repeat
            if eAttendees is "" then set eAttendees to "none"
        on error
            set eAttendees to "none"
        end try
        set eAllDay to allday event of e
        try
            set eRecurrence to recurrence of e
            if eRecurrence is missing value then set eRecurrence to "none"
        on error
            set eRecurrence to "none"
        end try
        set output to output & "<<<EVT_START>>>" & eTitle & "<<<SEP>>>" & eStart & "<<<SEP>>>" & eEnd & "<<<SEP>>>" & eLoc & "<<<SEP>>>" & eNotes & "<<<SEP>>>" & eCalName & "<<<SEP>>>" & eAttendees & "<<<SEP>>>" & eAllDay & "<<<SEP>>>" & eRecurrence & "<<<EVT_END>>>"
    end repeat
    return output
end tell"#
    )
}

fn parse_output(raw: &str) -> Result<Vec<Event>, IngestionError> {
    let re = Regex::new(
        r"<<<EVT_START>>>(.*?)<<<SEP>>>(.*?)<<<SEP>>>(.*?)<<<SEP>>>(.*?)<<<SEP>>>(.*?)<<<SEP>>>(.*?)<<<SEP>>>(.*?)<<<SEP>>>(.*?)<<<SEP>>>(.*?)<<<EVT_END>>>"
    )
    .map_err(|e| IngestionError::Extraction(format!("Regex error: {}", e)))?;

    let mut events = Vec::new();
    for cap in re.captures_iter(raw) {
        let is_all_day = cap[8].trim().to_lowercase() == "true";

        events.push(Event {
            title: cap[1].trim().to_string(),
            start_date: cap[2].trim().to_string(),
            end_date: cap[3].trim().to_string(),
            location: cap[4].trim().to_string(),
            notes: cap[5].trim().to_string(),
            calendar_name: cap[6].trim().to_string(),
            attendees: cap[7].trim().to_string(),
            is_all_day,
            recurrence_rule: cap[9].trim().to_string(),
        });
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_output_basic() {
        let raw = "<<<EVT_START>>>Team Meeting<<<SEP>>>2024-01-20 10:00:00<<<SEP>>>2024-01-20 11:00:00<<<SEP>>>Conference Room<<<SEP>>>Weekly sync<<<SEP>>>Work<<<SEP>>>Alice, Bob<<<SEP>>>false<<<SEP>>>FREQ=WEEKLY<<<EVT_END>>>";
        let events = parse_output(raw).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].title, "Team Meeting");
        assert_eq!(events[0].start_date, "2024-01-20 10:00:00");
        assert_eq!(events[0].end_date, "2024-01-20 11:00:00");
        assert_eq!(events[0].location, "Conference Room");
        assert_eq!(events[0].notes, "Weekly sync");
        assert_eq!(events[0].calendar_name, "Work");
        assert_eq!(events[0].attendees, "Alice, Bob");
        assert!(!events[0].is_all_day);
        assert_eq!(events[0].recurrence_rule, "FREQ=WEEKLY");
    }

    #[test]
    fn parse_output_all_day() {
        let raw = "<<<EVT_START>>>Holiday<<<SEP>>>2024-12-25 00:00:00<<<SEP>>>2024-12-26 00:00:00<<<SEP>>>none<<<SEP>>>none<<<SEP>>>Personal<<<SEP>>>none<<<SEP>>>true<<<SEP>>>none<<<EVT_END>>>";
        let events = parse_output(raw).unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].is_all_day);
        assert_eq!(events[0].location, "none");
    }

    #[test]
    fn parse_output_multiple() {
        let raw = "<<<EVT_START>>>Event 1<<<SEP>>>2024-01-01 09:00:00<<<SEP>>>2024-01-01 10:00:00<<<SEP>>>Room A<<<SEP>>>none<<<SEP>>>Work<<<SEP>>>none<<<SEP>>>false<<<SEP>>>none<<<EVT_END>>><<<EVT_START>>>Event 2<<<SEP>>>2024-01-02 14:00:00<<<SEP>>>2024-01-02 15:00:00<<<SEP>>>none<<<SEP>>>Important<<<SEP>>>Personal<<<SEP>>>Charlie<<<SEP>>>false<<<SEP>>>FREQ=DAILY<<<EVT_END>>>";
        let events = parse_output(raw).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].title, "Event 1");
        assert_eq!(events[1].title, "Event 2");
        assert_eq!(events[1].attendees, "Charlie");
    }

    #[test]
    fn parse_output_empty() {
        let events = parse_output("").unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn build_script_all() {
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
    fn to_json_records_basic() {
        let events = vec![Event {
            title: "Test".to_string(),
            start_date: "2024-01-01 10:00:00".to_string(),
            end_date: "2024-01-01 11:00:00".to_string(),
            location: "none".to_string(),
            notes: "none".to_string(),
            calendar_name: "Work".to_string(),
            attendees: "none".to_string(),
            is_all_day: false,
            recurrence_rule: "none".to_string(),
        }];
        let records = to_json_records(&events);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["source"], "apple_calendar");
        assert_eq!(records[0]["title"], "Test");
        assert!(records[0]["content_hash"].as_str().is_some());
    }
}
