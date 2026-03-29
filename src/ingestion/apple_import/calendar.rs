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
}

/// Extract all events (or events from a specific calendar) from Apple Calendar.
pub fn extract(calendar: Option<&str>) -> Result<Vec<CalendarEvent>, IngestionError> {
    let script = build_script(calendar);
    let raw = run_osascript(&script)?;
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
                "content_hash": hash,
                "source": "apple_calendar",
            })
        })
        .collect()
}

pub fn build_script(calendar: Option<&str>) -> String {
    let calendar_filter = match calendar {
        Some(name) => format!(
            r#"set targetCalendar to calendar "{}"
    set eventList to every event of targetCalendar whose start date ≥ (current date) - 30 * days and start date ≤ (current date) + 90 * days"#,
            name.replace('"', "\\\"")
        ),
        None => r#"set eventList to {}
    repeat with cal in calendars
        set eventList to eventList & (every event of cal whose start date ≥ (current date) - 30 * days and start date ≤ (current date) + 90 * days)
    end repeat"#
            .to_string(),
    };

    format!(
        r#"tell application "Calendar"
    {calendar_filter}
    set output to ""
    repeat with e in eventList
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
        set eCalName to name of calendar of e
        set output to output & "<<<EVT_START>>>" & eSummary & "<<<SEP>>>" & eStart & "<<<SEP>>>" & eEnd & "<<<SEP>>>" & eLocation & "<<<SEP>>>" & eDescription & "<<<SEP>>>" & eCalName & "<<<SEP>>>" & eAllDay & "<<<SEP>>>" & eRecurring & "<<<EVT_END>>>"
    end repeat
    return output
end tell"#
    )
}

pub fn parse_output(raw: &str) -> Result<Vec<CalendarEvent>, IngestionError> {
    let re = Regex::new(
        r"<<<EVT_START>>>(.*?)<<<SEP>>>(.*?)<<<SEP>>>(.*?)<<<SEP>>>(.*?)<<<SEP>>>(.*?)<<<SEP>>>(.*?)<<<SEP>>>(.*?)<<<SEP>>>(.*?)<<<EVT_END>>>"
    )
    .map_err(|e| IngestionError::Extraction(format!("Regex error: {}", e)))?;

    let mut events = Vec::new();
    for cap in re.captures_iter(raw) {
        let all_day = cap[7].trim().to_lowercase() == "true";
        let recurring = cap[8].trim().to_lowercase() == "true";

        events.push(CalendarEvent {
            summary: cap[1].trim().to_string(),
            start_time: cap[2].trim().to_string(),
            end_time: cap[3].trim().to_string(),
            location: cap[4].trim().to_string(),
            description: cap[5].trim().to_string(),
            calendar: cap[6].trim().to_string(),
            all_day,
            recurring,
        });
    }
    Ok(events)
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
        }];
        let records = to_json_records(&events);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["summary"], "Test Event");
        assert_eq!(records[0]["source"], "apple_calendar");
        assert_eq!(records[0]["all_day"], false);
        assert_eq!(records[0]["recurring"], false);
        assert!(records[0]["content_hash"].is_string());
    }
}
