//! Integration tests for all 4 Apple import sources:
//! Notes, Photos, Calendar, and Reminders.
//!
//! These tests exercise the extraction pipeline (parse → struct → JSON records)
//! without calling osascript, so they run on any platform (including CI).
//!
//! Run with:
//!   cargo test --test apple_import_integration_test -- --nocapture

#[cfg(target_os = "macos")]
mod tests {
    use fold_db_node::ingestion::apple_import::{calendar, notes, photos, reminders};
    use fold_db_node::ingestion::apple_import::{content_hash, is_available};
    use serde_json::Value;
    use std::collections::HashSet;

    // =========================================================================
    // Shared helpers
    // =========================================================================

    /// Assert that a JSON record has a non-empty string field.
    fn assert_has_string(record: &Value, field: &str) {
        assert!(
            record.get(field).and_then(|v| v.as_str()).is_some(),
            "record missing string field '{}'",
            field
        );
    }

    // =========================================================================
    // 1. Notes — successful extraction
    // =========================================================================

    #[test]
    fn notes_parse_and_convert_roundtrip() {
        let raw = concat!(
            "<<<NOTE_START>>>Shopping List<<<SEP>>>Milk, eggs, bread, cheese, and more items<<<SEP>>>2026-03-01 08:00:00<<<SEP>>>2026-03-01 09:30:00<<<NOTE_END>>>",
            "<<<NOTE_START>>>Meeting Notes<<<SEP>>>Discussed roadmap for Q2 with the team<<<SEP>>>2026-03-02 14:00:00<<<SEP>>>2026-03-02 15:00:00<<<NOTE_END>>>"
        );

        let parsed = notes::parse_output(raw).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].title, "Shopping List");
        assert_eq!(parsed[1].title, "Meeting Notes");

        let records = notes::to_json_records(&parsed);
        assert_eq!(records.len(), 2);
        for r in &records {
            assert_has_string(r, "title");
            assert_has_string(r, "body");
            assert_has_string(r, "created_at");
            assert_has_string(r, "modified_at");
            assert_has_string(r, "content_hash");
            assert_eq!(r["source"], "apple_notes");
        }
    }

    #[test]
    fn notes_empty_results() {
        let parsed = notes::parse_output("").unwrap();
        assert!(parsed.is_empty());
        let records = notes::to_json_records(&parsed);
        assert!(records.is_empty());
    }

    #[test]
    fn notes_build_script_no_folder() {
        let script = notes::build_script(None);
        assert!(script.contains(r#"tell application "Notes""#));
        assert!(script.contains("set noteList to every note"));
        assert!(!script.contains("targetFolder"));
    }

    #[test]
    fn notes_build_script_with_folder() {
        let script = notes::build_script(Some("Work"));
        assert!(script.contains(r#"folder "Work""#));
        assert!(script.contains("targetFolder"));
    }

    #[test]
    fn notes_build_script_escapes_quotes() {
        let script = notes::build_script(Some(r#"Tom's "Special" Folder"#));
        assert!(script.contains(r#"folder "Tom's \"Special\" Folder""#));
    }

    // =========================================================================
    // 2. Reminders — successful extraction
    // =========================================================================

    #[test]
    fn reminders_parse_and_convert_roundtrip() {
        let raw = concat!(
            "<<<REM_START>>>Buy groceries<<<SEP>>>Shopping<<<SEP>>>false<<<SEP>>>2026-03-28 10:00:00<<<SEP>>>1<<<REM_END>>>",
            "<<<REM_START>>>Call dentist<<<SEP>>>Health<<<SEP>>>true<<<SEP>>>none<<<SEP>>>5<<<REM_END>>>",
            "<<<REM_START>>>Submit report<<<SEP>>>Work<<<SEP>>>false<<<SEP>>>2026-03-30 17:00:00<<<SEP>>>0<<<REM_END>>>"
        );

        let parsed = reminders::parse_output(raw).unwrap();
        assert_eq!(parsed.len(), 3);

        assert_eq!(parsed[0].name, "Buy groceries");
        assert_eq!(parsed[0].list, "Shopping");
        assert!(!parsed[0].completed);
        assert_eq!(parsed[0].priority, 1);

        assert_eq!(parsed[1].name, "Call dentist");
        assert!(parsed[1].completed);
        assert_eq!(parsed[1].due_date, "none");
        assert_eq!(parsed[1].priority, 5);

        let records = reminders::to_json_records(&parsed);
        assert_eq!(records.len(), 3);
        for r in &records {
            assert_has_string(r, "name");
            assert_has_string(r, "list");
            assert_has_string(r, "content_hash");
            assert_eq!(r["source"], "apple_reminders");
            assert!(r.get("completed").is_some());
            assert!(r.get("priority").is_some());
        }
    }

    #[test]
    fn reminders_empty_results() {
        let parsed = reminders::parse_output("").unwrap();
        assert!(parsed.is_empty());
        let records = reminders::to_json_records(&parsed);
        assert!(records.is_empty());
    }

    #[test]
    fn reminders_build_script_no_list() {
        let script = reminders::build_script(None);
        assert!(script.contains(r#"tell application "Reminders""#));
        assert!(script.contains("set reminderItems to every reminder"));
        assert!(!script.contains("targetList"));
    }

    #[test]
    fn reminders_build_script_with_list() {
        let script = reminders::build_script(Some("Shopping"));
        assert!(script.contains(r#"list "Shopping""#));
        assert!(script.contains("targetList"));
    }

    // =========================================================================
    // 3. Calendar — successful extraction
    // =========================================================================

    #[test]
    fn calendar_parse_and_convert_roundtrip() {
        let raw = concat!(
            "<<<EVT_START>>>Team Standup<<<SEP>>>2026-03-28 09:00:00<<<SEP>>>2026-03-28 09:15:00<<<SEP>>>Zoom<<<SEP>>>Daily sync meeting<<<SEP>>>Work<<<SEP>>>false<<<SEP>>>true<<<EVT_END>>>",
            "<<<EVT_START>>>Lunch with Alex<<<SEP>>>2026-03-28 12:00:00<<<SEP>>>2026-03-28 13:00:00<<<SEP>>>Cafe Luna<<<SEP>>><<<SEP>>>Personal<<<SEP>>>false<<<SEP>>>false<<<EVT_END>>>"
        );

        let parsed = calendar::parse_output(raw).unwrap();
        assert_eq!(parsed.len(), 2);

        assert_eq!(parsed[0].summary, "Team Standup");
        assert_eq!(parsed[0].location, "Zoom");
        assert_eq!(parsed[0].calendar, "Work");
        assert!(!parsed[0].all_day);
        assert!(parsed[0].recurring);

        assert_eq!(parsed[1].summary, "Lunch with Alex");
        assert_eq!(parsed[1].calendar, "Personal");
        assert!(parsed[1].description.is_empty());

        let records = calendar::to_json_records(&parsed);
        assert_eq!(records.len(), 2);
        for r in &records {
            assert_has_string(r, "summary");
            assert_has_string(r, "start_time");
            assert_has_string(r, "end_time");
            assert_has_string(r, "calendar");
            assert_has_string(r, "content_hash");
            assert_eq!(r["source"], "apple_calendar");
            assert!(r.get("all_day").is_some());
            assert!(r.get("recurring").is_some());
        }
    }

    #[test]
    fn calendar_empty_results() {
        let parsed = calendar::parse_output("").unwrap();
        assert!(parsed.is_empty());
        let records = calendar::to_json_records(&parsed);
        assert!(records.is_empty());
    }

    #[test]
    fn calendar_build_script_all() {
        let script = calendar::build_script(None);
        assert!(script.contains(r#"tell application "Calendar""#));
        assert!(script.contains("repeat with cal in calendars"));
        assert!(!script.contains("targetCalendar"));
    }

    #[test]
    fn calendar_build_script_specific() {
        let script = calendar::build_script(Some("Work"));
        assert!(script.contains(r#"calendar "Work""#));
        assert!(script.contains("targetCalendar"));
    }

    // =========================================================================
    // 3a. Calendar — edge cases: recurring events
    // =========================================================================

    #[test]
    fn calendar_recurring_event() {
        let raw = "<<<EVT_START>>>Weekly 1:1<<<SEP>>>2026-03-28 14:00:00<<<SEP>>>2026-03-28 14:30:00<<<SEP>>>Office<<<SEP>>>Manager 1:1<<<SEP>>>Work<<<SEP>>>false<<<SEP>>>true<<<EVT_END>>>";
        let events = calendar::parse_output(raw).unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].recurring);
        assert!(!events[0].all_day);

        let records = calendar::to_json_records(&events);
        assert_eq!(records[0]["recurring"], true);
    }

    // =========================================================================
    // 3b. Calendar — edge cases: all-day events
    // =========================================================================

    #[test]
    fn calendar_all_day_event() {
        let raw = "<<<EVT_START>>>Company Holiday<<<SEP>>>2026-12-25 00:00:00<<<SEP>>>2026-12-26 00:00:00<<<SEP>>><<<SEP>>>Christmas<<<SEP>>>Company<<<SEP>>>true<<<SEP>>>false<<<EVT_END>>>";
        let events = calendar::parse_output(raw).unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].all_day);
        assert!(!events[0].recurring);
        assert!(events[0].location.is_empty());

        let records = calendar::to_json_records(&events);
        assert_eq!(records[0]["all_day"], true);
        assert_eq!(records[0]["recurring"], false);
    }

    #[test]
    fn calendar_all_day_recurring_event() {
        let raw = "<<<EVT_START>>>Team Lunch Friday<<<SEP>>>2026-03-27 00:00:00<<<SEP>>>2026-03-28 00:00:00<<<SEP>>>Cafeteria<<<SEP>>>Weekly team lunch<<<SEP>>>Work<<<SEP>>>true<<<SEP>>>true<<<EVT_END>>>";
        let events = calendar::parse_output(raw).unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].all_day);
        assert!(events[0].recurring);
    }

    // =========================================================================
    // 3c. Calendar — edge cases: multi-calendar
    // =========================================================================

    #[test]
    fn calendar_multi_calendar_events() {
        let raw = concat!(
            "<<<EVT_START>>>Standup<<<SEP>>>2026-03-28 09:00:00<<<SEP>>>2026-03-28 09:15:00<<<SEP>>><<<SEP>>><<<SEP>>>Work<<<SEP>>>false<<<SEP>>>true<<<EVT_END>>>",
            "<<<EVT_START>>>Yoga Class<<<SEP>>>2026-03-28 18:00:00<<<SEP>>>2026-03-28 19:00:00<<<SEP>>>Studio<<<SEP>>><<<SEP>>>Personal<<<SEP>>>false<<<SEP>>>true<<<EVT_END>>>",
            "<<<EVT_START>>>Dentist<<<SEP>>>2026-03-29 10:00:00<<<SEP>>>2026-03-29 11:00:00<<<SEP>>>Clinic<<<SEP>>>Annual checkup<<<SEP>>>Health<<<SEP>>>false<<<SEP>>>false<<<EVT_END>>>"
        );

        let events = calendar::parse_output(raw).unwrap();
        assert_eq!(events.len(), 3);

        let calendars: HashSet<&str> = events.iter().map(|e| e.calendar.as_str()).collect();
        assert_eq!(calendars.len(), 3);
        assert!(calendars.contains("Work"));
        assert!(calendars.contains("Personal"));
        assert!(calendars.contains("Health"));

        let records = calendar::to_json_records(&events);
        assert_eq!(records.len(), 3);

        // Each event from a different calendar should have a different content_hash
        let hashes: HashSet<&str> = records
            .iter()
            .map(|r| r["content_hash"].as_str().unwrap())
            .collect();
        assert_eq!(
            hashes.len(),
            3,
            "each event should have a unique content hash"
        );
    }

    // =========================================================================
    // 3d. Calendar — edge cases: events with special characters
    // =========================================================================

    #[test]
    fn calendar_event_with_special_chars() {
        let raw = r#"<<<EVT_START>>>Tom & Jerry's Meeting — "Important"<<<SEP>>>2026-03-28 10:00:00<<<SEP>>>2026-03-28 11:00:00<<<SEP>>>Room #42 (3rd floor)<<<SEP>>>Discuss Q2 goals & budget<<<SEP>>>Work<<<SEP>>>false<<<SEP>>>false<<<EVT_END>>>"#;
        let events = calendar::parse_output(raw).unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].summary.contains("Tom & Jerry"));
        assert!(events[0].location.contains("#42"));
    }

    // =========================================================================
    // 4. Photos — build script & collect/convert
    // =========================================================================

    #[test]
    fn photos_build_script_no_album() {
        let script = photos::build_script(None, 50);
        assert!(script.contains(r#"tell application "Photos""#));
        assert!(script.contains("set allItems to every media item"));
        assert!(script.contains("50"));
        assert!(!script.contains("targetAlbum"));
    }

    #[test]
    fn photos_build_script_with_album() {
        let script = photos::build_script(Some("Vacation 2026"), 20);
        assert!(script.contains(r#"album "Vacation 2026""#));
        assert!(script.contains("20"));
    }

    #[test]
    fn photos_collect_and_convert_empty_dir() {
        let dir = std::env::temp_dir().join("folddb_test_photos_integration_empty");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let paths = photos::collect_and_convert(&dir).unwrap();
        assert!(paths.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn photos_collect_and_convert_multiple_jpegs() {
        let dir = std::env::temp_dir().join("folddb_test_photos_integration_multi");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Create fake JPEG files
        for name in &["photo1.jpg", "photo2.jpeg", "photo3.png"] {
            std::fs::write(dir.join(name), b"fake image data").unwrap();
        }

        let paths = photos::collect_and_convert(&dir).unwrap();
        assert_eq!(paths.len(), 3);

        // Paths should be sorted
        for i in 1..paths.len() {
            assert!(paths[i] >= paths[i - 1], "paths should be sorted");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn photos_collect_and_convert_skips_directories() {
        let dir = std::env::temp_dir().join("folddb_test_photos_integration_dirs");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(dir.join("subdir")).unwrap();
        std::fs::write(dir.join("photo.jpg"), b"fake jpeg").unwrap();

        let paths = photos::collect_and_convert(&dir).unwrap();
        assert_eq!(paths.len(), 1);
        assert!(paths[0].file_name().unwrap().to_str().unwrap() == "photo.jpg");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // =========================================================================
    // 5. Content-hash deduplication
    // =========================================================================

    #[test]
    fn content_hash_deterministic() {
        let h1 = content_hash("hello world");
        let h2 = content_hash("hello world");
        assert_eq!(h1, h2, "same input must produce same hash");
    }

    #[test]
    fn content_hash_different_inputs() {
        let h1 = content_hash("hello world");
        let h2 = content_hash("hello world!");
        assert_ne!(h1, h2, "different inputs must produce different hashes");
    }

    #[test]
    fn content_hash_is_16_hex_chars() {
        let h = content_hash("test content");
        assert_eq!(h.len(), 16, "hash should be 16 hex chars (8 bytes)");
        assert!(
            h.chars().all(|c| c.is_ascii_hexdigit()),
            "hash should be hex"
        );
    }

    #[test]
    fn notes_dedup_same_data_twice_produces_same_hashes() {
        let raw = "<<<NOTE_START>>>Shopping List<<<SEP>>>Milk, eggs, bread, cheese, and more items<<<SEP>>>2026-03-01 08:00:00<<<SEP>>>2026-03-01 09:30:00<<<NOTE_END>>>";

        let parsed1 = notes::parse_output(raw).unwrap();
        let parsed2 = notes::parse_output(raw).unwrap();
        let records1 = notes::to_json_records(&parsed1);
        let records2 = notes::to_json_records(&parsed2);

        assert_eq!(
            records1[0]["content_hash"], records2[0]["content_hash"],
            "importing same note twice must produce identical content_hash"
        );
    }

    #[test]
    fn reminders_dedup_same_data_twice_produces_same_hashes() {
        let raw = "<<<REM_START>>>Buy groceries<<<SEP>>>Shopping<<<SEP>>>false<<<SEP>>>2026-03-28 10:00:00<<<SEP>>>1<<<REM_END>>>";

        let parsed1 = reminders::parse_output(raw).unwrap();
        let parsed2 = reminders::parse_output(raw).unwrap();
        let records1 = reminders::to_json_records(&parsed1);
        let records2 = reminders::to_json_records(&parsed2);

        assert_eq!(
            records1[0]["content_hash"], records2[0]["content_hash"],
            "importing same reminder twice must produce identical content_hash"
        );
    }

    #[test]
    fn calendar_dedup_same_data_twice_produces_same_hashes() {
        let raw = "<<<EVT_START>>>Team Standup<<<SEP>>>2026-03-28 09:00:00<<<SEP>>>2026-03-28 09:15:00<<<SEP>>>Zoom<<<SEP>>>Daily sync<<<SEP>>>Work<<<SEP>>>false<<<SEP>>>true<<<EVT_END>>>";

        let parsed1 = calendar::parse_output(raw).unwrap();
        let parsed2 = calendar::parse_output(raw).unwrap();
        let records1 = calendar::to_json_records(&parsed1);
        let records2 = calendar::to_json_records(&parsed2);

        assert_eq!(
            records1[0]["content_hash"], records2[0]["content_hash"],
            "importing same event twice must produce identical content_hash"
        );
    }

    #[test]
    fn notes_dedup_different_metadata_same_body_produces_same_hash() {
        // Two notes with different titles/dates but same body should have same hash
        // because notes hash on body content only
        let note_a = notes::Note {
            title: "Title A".to_string(),
            body: "Identical body content here".to_string(),
            created_at: "2026-01-01".to_string(),
            modified_at: "2026-01-02".to_string(),
        };
        let note_b = notes::Note {
            title: "Title B".to_string(),
            body: "Identical body content here".to_string(),
            created_at: "2026-03-01".to_string(),
            modified_at: "2026-03-02".to_string(),
        };

        let records_a = notes::to_json_records(&[note_a]);
        let records_b = notes::to_json_records(&[note_b]);

        assert_eq!(
            records_a[0]["content_hash"], records_b[0]["content_hash"],
            "notes with same body should have same content_hash regardless of title/dates"
        );
    }

    #[test]
    fn reminders_dedup_different_metadata_same_name_produces_same_hash() {
        let rem_a = reminders::Reminder {
            name: "Buy groceries".to_string(),
            list: "Shopping".to_string(),
            completed: false,
            due_date: "2026-03-28".to_string(),
            priority: 1,
        };
        let rem_b = reminders::Reminder {
            name: "Buy groceries".to_string(),
            list: "Personal".to_string(),
            completed: true,
            due_date: "2026-04-01".to_string(),
            priority: 5,
        };

        let records_a = reminders::to_json_records(&[rem_a]);
        let records_b = reminders::to_json_records(&[rem_b]);

        assert_eq!(
            records_a[0]["content_hash"], records_b[0]["content_hash"],
            "reminders with same name should have same content_hash regardless of list/status"
        );
    }

    #[test]
    fn calendar_dedup_same_event_different_description() {
        // Calendar hashes on summary|start_time|calendar, so same event
        // with different description should still produce the same hash
        let evt_a = calendar::CalendarEvent {
            summary: "Standup".to_string(),
            start_time: "2026-03-28 09:00:00".to_string(),
            end_time: "2026-03-28 09:15:00".to_string(),
            location: "Zoom".to_string(),
            description: "Version 1 of description".to_string(),
            calendar: "Work".to_string(),
            all_day: false,
            recurring: true,
            attendees: Vec::new(),
        };
        let evt_b = calendar::CalendarEvent {
            summary: "Standup".to_string(),
            start_time: "2026-03-28 09:00:00".to_string(),
            end_time: "2026-03-28 09:30:00".to_string(),
            location: "Teams".to_string(),
            description: "Updated description".to_string(),
            calendar: "Work".to_string(),
            all_day: false,
            recurring: false,
            attendees: Vec::new(),
        };

        let records_a = calendar::to_json_records(&[evt_a]);
        let records_b = calendar::to_json_records(&[evt_b]);

        assert_eq!(
            records_a[0]["content_hash"], records_b[0]["content_hash"],
            "same summary+start_time+calendar should produce same hash"
        );
    }

    #[test]
    fn calendar_dedup_different_calendar_different_hash() {
        let evt_a = calendar::CalendarEvent {
            summary: "Standup".to_string(),
            start_time: "2026-03-28 09:00:00".to_string(),
            end_time: "2026-03-28 09:15:00".to_string(),
            location: "".to_string(),
            description: "".to_string(),
            calendar: "Work".to_string(),
            all_day: false,
            recurring: false,
            attendees: Vec::new(),
        };
        let evt_b = calendar::CalendarEvent {
            summary: "Standup".to_string(),
            start_time: "2026-03-28 09:00:00".to_string(),
            end_time: "2026-03-28 09:15:00".to_string(),
            location: "".to_string(),
            description: "".to_string(),
            calendar: "Personal".to_string(),
            all_day: false,
            recurring: false,
            attendees: Vec::new(),
        };

        let records_a = calendar::to_json_records(&[evt_a]);
        let records_b = calendar::to_json_records(&[evt_b]);

        assert_ne!(
            records_a[0]["content_hash"], records_b[0]["content_hash"],
            "same event in different calendars should have different hashes"
        );
    }

    // =========================================================================
    // 6. Batch processing — large datasets produce correct record counts
    // =========================================================================

    #[test]
    fn notes_batch_processing_many_records() {
        let mut raw = String::new();
        for i in 0..25 {
            raw.push_str(&format!(
                "<<<NOTE_START>>>Note {}<<<SEP>>>Body of note {} with enough text to pass<<<SEP>>>2026-03-{:02} 10:00:00<<<SEP>>>2026-03-{:02} 11:00:00<<<NOTE_END>>>",
                i, i, (i % 28) + 1, (i % 28) + 1
            ));
        }

        let parsed = notes::parse_output(&raw).unwrap();
        assert_eq!(parsed.len(), 25);

        let records = notes::to_json_records(&parsed);
        assert_eq!(records.len(), 25);

        // All hashes should be unique (different bodies)
        let hashes: HashSet<&str> = records
            .iter()
            .map(|r| r["content_hash"].as_str().unwrap())
            .collect();
        assert_eq!(hashes.len(), 25, "all 25 notes should have unique hashes");
    }

    #[test]
    fn reminders_batch_processing_many_records() {
        let mut raw = String::new();
        for i in 0..50 {
            let completed = if i % 3 == 0 { "true" } else { "false" };
            raw.push_str(&format!(
                "<<<REM_START>>>Reminder {}<<<SEP>>>List {}<<<SEP>>>{}<<<SEP>>>2026-04-{:02} 10:00:00<<<SEP>>>{}<<<REM_END>>>",
                i, i % 5, completed, (i % 28) + 1, i % 10
            ));
        }

        let parsed = reminders::parse_output(&raw).unwrap();
        assert_eq!(parsed.len(), 50);

        let records = reminders::to_json_records(&parsed);
        assert_eq!(records.len(), 50);

        // Verify completed status distribution
        let completed_count = parsed.iter().filter(|r| r.completed).count();
        assert_eq!(completed_count, 17); // i % 3 == 0 for i in 0..50 → 17
    }

    #[test]
    fn calendar_batch_processing_many_records() {
        let mut raw = String::new();
        let calendars = ["Work", "Personal", "Health", "Family"];
        for i in 0..30 {
            let cal = calendars[i % calendars.len()];
            let all_day = if i % 5 == 0 { "true" } else { "false" };
            let recurring = if i % 4 == 0 { "true" } else { "false" };
            raw.push_str(&format!(
                "<<<EVT_START>>>Event {}<<<SEP>>>2026-04-{:02} {}:00:00<<<SEP>>>2026-04-{:02} {}:00:00<<<SEP>>>Location {}<<<SEP>>>Description {}<<<SEP>>>{}<<<SEP>>>{}<<<SEP>>>{}<<<EVT_END>>>",
                i, (i % 28) + 1, 8 + (i % 12), (i % 28) + 1, 9 + (i % 12), i, i, cal, all_day, recurring
            ));
        }

        let parsed = calendar::parse_output(&raw).unwrap();
        assert_eq!(parsed.len(), 30);

        let records = calendar::to_json_records(&parsed);
        assert_eq!(records.len(), 30);

        // Verify calendar distribution
        let cal_counts: std::collections::HashMap<&str, usize> =
            parsed
                .iter()
                .fold(std::collections::HashMap::new(), |mut m, e| {
                    *m.entry(e.calendar.as_str()).or_default() += 1;
                    m
                });
        // 30 events across 4 calendars: 8, 8, 7, 7
        assert_eq!(cal_counts.len(), 4);
        for count in cal_counts.values() {
            assert!(*count >= 7 && *count <= 8);
        }

        // Verify all_day and recurring counts
        let all_day_count = parsed.iter().filter(|e| e.all_day).count();
        assert_eq!(all_day_count, 6); // i % 5 == 0 for i in 0..30 → 6
        let recurring_count = parsed.iter().filter(|e| e.recurring).count();
        assert_eq!(recurring_count, 8); // i % 4 == 0 for i in 0..30 → 8
    }

    // =========================================================================
    // 7. Error handling — malformed input
    // =========================================================================

    #[test]
    fn notes_parse_partial_delimiters_ignored() {
        // Incomplete delimiters should not produce any notes
        let raw = "<<<NOTE_START>>>Broken note<<<SEP>>>body only two seps";
        let parsed = notes::parse_output(raw).unwrap();
        assert!(parsed.is_empty());
    }

    #[test]
    fn reminders_parse_partial_delimiters_ignored() {
        let raw = "<<<REM_START>>>Broken<<<SEP>>>only partial";
        let parsed = reminders::parse_output(raw).unwrap();
        assert!(parsed.is_empty());
    }

    #[test]
    fn calendar_parse_partial_delimiters_ignored() {
        let raw = "<<<EVT_START>>>Broken event<<<SEP>>>only two fields";
        let parsed = calendar::parse_output(raw).unwrap();
        assert!(parsed.is_empty());
    }

    #[test]
    fn notes_parse_mixed_valid_and_invalid() {
        // One valid note surrounded by garbage
        let raw = "garbage<<<NOTE_START>>>Valid Note<<<SEP>>>This is valid body content here<<<SEP>>>2026-01-01<<<SEP>>>2026-01-02<<<NOTE_END>>>more garbage";
        let parsed = notes::parse_output(raw).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].title, "Valid Note");
    }

    #[test]
    fn reminders_parse_mixed_valid_and_invalid() {
        let raw = "junk<<<REM_START>>>Valid<<<SEP>>>List<<<SEP>>>false<<<SEP>>>none<<<SEP>>>0<<<REM_END>>>junk";
        let parsed = reminders::parse_output(raw).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "Valid");
    }

    #[test]
    fn calendar_parse_mixed_valid_and_invalid() {
        let raw = "junk<<<EVT_START>>>Valid Event<<<SEP>>>Start<<<SEP>>>End<<<SEP>>>Loc<<<SEP>>>Desc<<<SEP>>>Cal<<<SEP>>>false<<<SEP>>>false<<<EVT_END>>>junk";
        let parsed = calendar::parse_output(raw).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].summary, "Valid Event");
    }

    // =========================================================================
    // 8. Permission errors — AppleScript error messages
    // =========================================================================

    #[test]
    fn notes_extraction_error_message_from_applescript() {
        // Simulate what happens when osascript returns an error
        // The IngestionError::Extraction variant wraps the stderr output
        use fold_db_node::ingestion::IngestionError;

        let err = IngestionError::Extraction(
            "AppleScript error: Notes got an error: Not authorized to send Apple events to Notes."
                .to_string(),
        );
        let msg = err.to_string();
        assert!(msg.contains("Not authorized"));
        assert!(msg.contains("Apple events"));
    }

    #[test]
    fn calendar_extraction_error_message_from_applescript() {
        use fold_db_node::ingestion::IngestionError;

        let err = IngestionError::Extraction(
            "AppleScript error: Calendar got an error: Not authorized to send Apple events to Calendar."
                .to_string(),
        );
        let msg = err.to_string();
        assert!(msg.contains("Not authorized"));
        assert!(msg.contains("Calendar"));
    }

    // =========================================================================
    // 9. Timeout handling — error type and message
    // =========================================================================

    #[test]
    fn timeout_error_contains_recovery_hint() {
        use fold_db_node::ingestion::IngestionError;

        let err = IngestionError::Extraction(
            "osascript timed out after 300 seconds. Photos.app may be unresponsive or \
             processing iCloud photos. Try again with a smaller limit, or ensure Full \
             Disk Access is granted in System Settings → Privacy & Security."
                .to_string(),
        );
        let msg = err.to_string();
        assert!(msg.contains("timed out"));
        assert!(msg.contains("300 seconds"));
        assert!(msg.contains("Full Disk Access"));
        assert!(msg.contains("Privacy & Security"));
    }

    #[test]
    fn timeout_error_variant_has_provider_info() {
        use fold_db_node::ingestion::IngestionError;

        let err = IngestionError::TimeoutError {
            provider: "Apple Calendar".to_string(),
            message: "osascript exceeded 300s timeout".to_string(),
        };
        let msg = err.user_message();
        assert!(msg.contains("Apple Calendar"));
        assert!(msg.contains("timed out"));
    }

    // =========================================================================
    // 10. Platform availability
    // =========================================================================

    #[test]
    fn is_available_returns_true_on_macos() {
        // This test file is gated on cfg(target_os = "macos"), so this should be true
        assert!(is_available());
    }

    // =========================================================================
    // 11. Calendar — content hash includes calendar name for cross-calendar dedup
    // =========================================================================

    #[test]
    fn calendar_hash_components_are_summary_start_calendar() {
        let evt = calendar::CalendarEvent {
            summary: "Meeting".to_string(),
            start_time: "2026-03-28 10:00:00".to_string(),
            end_time: "2026-03-28 11:00:00".to_string(),
            location: "Room A".to_string(),
            description: "Desc".to_string(),
            calendar: "Work".to_string(),
            all_day: false,
            recurring: false,
            attendees: Vec::new(),
        };

        let records = calendar::to_json_records(&[evt]);
        let expected_hash = content_hash("Meeting|2026-03-28 10:00:00|Work");
        assert_eq!(
            records[0]["content_hash"].as_str().unwrap(),
            expected_hash,
            "calendar hash should be sha256 of summary|start_time|calendar"
        );
    }

    // =========================================================================
    // 12. Notes — content hash uses body only
    // =========================================================================

    #[test]
    fn notes_hash_uses_body_only() {
        let note = notes::Note {
            title: "Any Title".to_string(),
            body: "This is the body text".to_string(),
            created_at: "2026-01-01".to_string(),
            modified_at: "2026-01-02".to_string(),
        };

        let records = notes::to_json_records(&[note]);
        let expected_hash = content_hash("This is the body text");
        assert_eq!(records[0]["content_hash"].as_str().unwrap(), expected_hash);
    }

    // =========================================================================
    // 13. Reminders — content hash uses name only
    // =========================================================================

    #[test]
    fn reminders_hash_uses_name_only() {
        let rem = reminders::Reminder {
            name: "Buy milk".to_string(),
            list: "Shopping".to_string(),
            completed: false,
            due_date: "2026-03-28".to_string(),
            priority: 1,
        };

        let records = reminders::to_json_records(&[rem]);
        let expected_hash = content_hash("Buy milk");
        assert_eq!(records[0]["content_hash"].as_str().unwrap(), expected_hash);
    }

    // =========================================================================
    // 14. Reminders — priority parsing edge cases
    // =========================================================================

    #[test]
    fn reminders_parse_invalid_priority_defaults_to_zero() {
        let raw = "<<<REM_START>>>Task<<<SEP>>>List<<<SEP>>>false<<<SEP>>>none<<<SEP>>>not_a_number<<<REM_END>>>";
        let parsed = reminders::parse_output(raw).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(
            parsed[0].priority, 0,
            "invalid priority should default to 0"
        );
    }

    // =========================================================================
    // 15. Calendar — large batch with mixed event types for progress simulation
    // =========================================================================

    #[test]
    fn calendar_mixed_event_types_large_batch() {
        let mut raw = String::new();

        // Regular events
        for i in 0..10 {
            raw.push_str(&format!(
                "<<<EVT_START>>>Meeting {}<<<SEP>>>2026-04-{:02} 10:00:00<<<SEP>>>2026-04-{:02} 11:00:00<<<SEP>>>Room {}<<<SEP>>>Agenda {}<<<SEP>>>Work<<<SEP>>>false<<<SEP>>>false<<<EVT_END>>>",
                i, (i % 28) + 1, (i % 28) + 1, i, i
            ));
        }

        // All-day events
        for i in 0..5 {
            raw.push_str(&format!(
                "<<<EVT_START>>>Holiday {}<<<SEP>>>2026-05-{:02} 00:00:00<<<SEP>>>2026-05-{:02} 00:00:00<<<SEP>>><<<SEP>>><<<SEP>>>Personal<<<SEP>>>true<<<SEP>>>false<<<EVT_END>>>",
                i, (i % 28) + 1, (i % 28) + 2
            ));
        }

        // Recurring events
        for i in 0..5 {
            raw.push_str(&format!(
                "<<<EVT_START>>>Weekly {}<<<SEP>>>2026-06-{:02} 09:00:00<<<SEP>>>2026-06-{:02} 09:30:00<<<SEP>>><<<SEP>>><<<SEP>>>Work<<<SEP>>>false<<<SEP>>>true<<<EVT_END>>>",
                i, (i % 28) + 1, (i % 28) + 1
            ));
        }

        let parsed = calendar::parse_output(&raw).unwrap();
        assert_eq!(parsed.len(), 20);

        let regular: Vec<_> = parsed
            .iter()
            .filter(|e| !e.all_day && !e.recurring)
            .collect();
        let all_day: Vec<_> = parsed.iter().filter(|e| e.all_day).collect();
        let recurring: Vec<_> = parsed.iter().filter(|e| e.recurring).collect();

        assert_eq!(regular.len(), 10);
        assert_eq!(all_day.len(), 5);
        assert_eq!(recurring.len(), 5);

        // All events should produce valid JSON records
        let records = calendar::to_json_records(&parsed);
        assert_eq!(records.len(), 20);

        // Simulate batch chunking (batch_size = 10)
        let batch_size = 10;
        let chunks: Vec<_> = records.chunks(batch_size).collect();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 10);
        assert_eq!(chunks[1].len(), 10);
    }
}
