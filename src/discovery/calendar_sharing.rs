//! Shared calendar event detection for connected peers.
//!
//! Compares calendar events between mutually opted-in peers to detect overlaps
//! (same conference, meetup, or event). Only reveals overlap existence — never
//! full calendar details.
//!
//! Comparison uses: date range overlap + location string similarity + title similarity.

use chrono::{NaiveDateTime, ParseError};
use fold_db::storage::traits::KvStore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

const CALENDAR_SHARING_OPTED_IN_KEY: &str = "discovery:calendar_sharing:opted_in";
const CALENDAR_EVENTS_PREFIX: &str = "discovery:calendar_events:";
const PEER_EVENTS_PREFIX: &str = "discovery:peer_events:";

/// Minimum combined similarity score to consider events overlapping.
const OVERLAP_THRESHOLD: f64 = 0.5;

/// Weight for date overlap in combined score.
const DATE_WEIGHT: f64 = 0.4;
/// Weight for title similarity in combined score.
const TITLE_WEIGHT: f64 = 0.35;
/// Weight for location similarity in combined score.
const LOCATION_WEIGHT: f64 = 0.25;

/// A normalized calendar event fingerprint for privacy-preserving comparison.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventFingerprint {
    /// SHA-256 hash of the original event (summary|start_time|calendar) for dedup.
    pub event_hash: String,
    /// Lowercased, whitespace-normalized title tokens.
    pub title_tokens: Vec<String>,
    /// Lowercased, whitespace-normalized location tokens.
    pub location_tokens: Vec<String>,
    /// Start time as ISO 8601 string.
    pub start_time: String,
    /// End time as ISO 8601 string.
    pub end_time: String,
    /// Original event title (stored locally, shared only as tokens to peers).
    pub display_title: String,
}

/// A set of event fingerprints from a peer, stored locally.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerEventSet {
    pub peer_pseudonym: String,
    pub fingerprints: Vec<EventFingerprint>,
    pub updated_at: String,
}

/// A detected overlap between the user's event and peers' events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedEvent {
    /// The user's event title.
    pub event_title: String,
    /// Start time of the user's event.
    pub start_time: String,
    /// End time of the user's event.
    pub end_time: String,
    /// Location of the user's event.
    pub location: String,
    /// Number of connections also attending.
    pub connection_count: usize,
    /// Pseudonyms of connections with overlapping events.
    pub connection_pseudonyms: Vec<String>,
    /// Combined similarity score (0.0–1.0).
    pub match_score: f64,
}

// === Opt-in management ===

/// Check if the user has opted in to calendar sharing.
pub async fn is_opted_in(store: &dyn KvStore) -> Result<bool, String> {
    let val = store
        .get(CALENDAR_SHARING_OPTED_IN_KEY.as_bytes())
        .await
        .map_err(|e| format!("Failed to read calendar sharing opt-in: {}", e))?;
    Ok(val.is_some_and(|v| v == b"true"))
}

/// Set calendar sharing opt-in status.
pub async fn set_opt_in(store: &dyn KvStore, opted_in: bool) -> Result<(), String> {
    let value = if opted_in {
        b"true".to_vec()
    } else {
        b"false".to_vec()
    };
    store
        .put(CALENDAR_SHARING_OPTED_IN_KEY.as_bytes(), value)
        .await
        .map_err(|e| format!("Failed to save calendar sharing opt-in: {}", e))
}

// === Event fingerprinting ===

/// Tokenize a string: lowercase, split on whitespace and punctuation, remove empties.
fn tokenize(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| {
            c.is_whitespace()
                || c == ','
                || c == '-'
                || c == '/'
                || c == '('
                || c == ')'
                || c == ':'
        })
        .filter(|t| !t.is_empty() && t.len() > 1)
        .map(String::from)
        .collect()
}

/// Create a fingerprint from a calendar event's fields.
pub fn fingerprint_event(
    summary: &str,
    start_time: &str,
    end_time: &str,
    location: &str,
    calendar: &str,
) -> EventFingerprint {
    let hash_input = format!("{}|{}|{}", summary, start_time, calendar);
    let mut hasher = Sha256::new();
    hasher.update(hash_input.as_bytes());
    let event_hash = hex::encode(hasher.finalize());

    EventFingerprint {
        event_hash,
        title_tokens: tokenize(summary),
        location_tokens: tokenize(location),
        start_time: start_time.to_string(),
        end_time: end_time.to_string(),
        display_title: summary.to_string(),
    }
}

// === Local event storage ===

/// Save the user's event fingerprints to the local store.
pub async fn save_local_events(
    store: &dyn KvStore,
    fingerprints: &[EventFingerprint],
) -> Result<usize, String> {
    let count = fingerprints.len();
    for fp in fingerprints {
        let key = format!("{}{}", CALENDAR_EVENTS_PREFIX, fp.event_hash);
        let value = serde_json::to_vec(fp)
            .map_err(|e| format!("Failed to serialize fingerprint: {}", e))?;
        store
            .put(key.as_bytes(), value)
            .await
            .map_err(|e| format!("Failed to save fingerprint: {}", e))?;
    }
    Ok(count)
}

/// Load all local event fingerprints.
pub async fn load_local_events(store: &dyn KvStore) -> Result<Vec<EventFingerprint>, String> {
    let entries = store
        .scan_prefix(CALENDAR_EVENTS_PREFIX.as_bytes())
        .await
        .map_err(|e| format!("Failed to scan local events: {}", e))?;

    let mut events = Vec::new();
    for (_key, value) in entries {
        match serde_json::from_slice(&value) {
            Ok(fp) => events.push(fp),
            Err(e) => log::warn!("Failed to deserialize event fingerprint: {}", e),
        }
    }
    Ok(events)
}

// === Peer event storage ===

/// Save event fingerprints received from a peer.
pub async fn save_peer_events(store: &dyn KvStore, peer_set: &PeerEventSet) -> Result<(), String> {
    let key = format!("{}{}", PEER_EVENTS_PREFIX, peer_set.peer_pseudonym);
    let value = serde_json::to_vec(peer_set)
        .map_err(|e| format!("Failed to serialize peer events: {}", e))?;
    store
        .put(key.as_bytes(), value)
        .await
        .map_err(|e| format!("Failed to save peer events: {}", e))
}

/// Load all peer event sets.
pub async fn load_all_peer_events(store: &dyn KvStore) -> Result<Vec<PeerEventSet>, String> {
    let entries = store
        .scan_prefix(PEER_EVENTS_PREFIX.as_bytes())
        .await
        .map_err(|e| format!("Failed to scan peer events: {}", e))?;

    let mut peer_sets = Vec::new();
    for (_key, value) in entries {
        match serde_json::from_slice(&value) {
            Ok(ps) => peer_sets.push(ps),
            Err(e) => log::warn!("Failed to deserialize peer event set: {}", e),
        }
    }
    Ok(peer_sets)
}

// === Overlap detection ===

/// Parse a datetime string, trying multiple formats.
fn parse_datetime(s: &str) -> Result<NaiveDateTime, ParseError> {
    // Try ISO 8601 first
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Ok(dt);
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Ok(dt);
    }
    // Apple Calendar format: "Saturday, March 28, 2026 at 9:00:00 AM"
    // Fall back to treating as a date-only
    NaiveDateTime::parse_from_str(&format!("{} 00:00:00", s), "%Y-%m-%d %H:%M:%S")
}

/// Check if two date ranges overlap and return overlap ratio (0.0–1.0).
/// Returns 1.0 for perfect overlap, 0.0 for no overlap.
fn date_overlap_score(start_a: &str, end_a: &str, start_b: &str, end_b: &str) -> f64 {
    let (Ok(sa), Ok(ea), Ok(sb), Ok(eb)) = (
        parse_datetime(start_a),
        parse_datetime(end_a),
        parse_datetime(start_b),
        parse_datetime(end_b),
    ) else {
        // If we can't parse, fall back to exact string match on date portion
        let date_a = start_a.split_whitespace().next().unwrap_or(start_a);
        let date_b = start_b.split_whitespace().next().unwrap_or(start_b);
        return if date_a == date_b { 0.8 } else { 0.0 };
    };

    let overlap_start = sa.max(sb);
    let overlap_end = ea.min(eb);

    if overlap_start >= overlap_end {
        return 0.0;
    }

    let overlap_duration = (overlap_end - overlap_start).num_seconds() as f64;
    let duration_a = (ea - sa).num_seconds().max(1) as f64;
    let duration_b = (eb - sb).num_seconds().max(1) as f64;
    let max_duration = duration_a.max(duration_b);

    (overlap_duration / max_duration).min(1.0)
}

/// Jaccard similarity between two token sets (0.0–1.0).
fn jaccard_similarity(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }

    let set_a: HashSet<&str> = a.iter().map(|s| s.as_str()).collect();
    let set_b: HashSet<&str> = b.iter().map(|s| s.as_str()).collect();

    let intersection = set_a.intersection(&set_b).count() as f64;
    let union = set_a.union(&set_b).count() as f64;

    if union == 0.0 {
        return 0.0;
    }

    intersection / union
}

/// Compute combined similarity score between two event fingerprints.
pub fn event_similarity(a: &EventFingerprint, b: &EventFingerprint) -> f64 {
    let date_score = date_overlap_score(&a.start_time, &a.end_time, &b.start_time, &b.end_time);
    let title_score = jaccard_similarity(&a.title_tokens, &b.title_tokens);
    let location_score = jaccard_similarity(&a.location_tokens, &b.location_tokens);

    // If dates don't overlap at all, no match regardless of title/location
    if date_score == 0.0 {
        return 0.0;
    }

    DATE_WEIGHT * date_score + TITLE_WEIGHT * title_score + LOCATION_WEIGHT * location_score
}

/// Detect shared events between local events and all peer events.
pub fn detect_shared_events(
    local_events: &[EventFingerprint],
    peer_event_sets: &[PeerEventSet],
) -> Vec<SharedEvent> {
    // For each local event, find which peers have overlapping events
    let mut shared: HashMap<String, SharedEvent> = HashMap::new();

    for local_fp in local_events {
        for peer_set in peer_event_sets {
            for peer_fp in &peer_set.fingerprints {
                let score = event_similarity(local_fp, peer_fp);
                if score >= OVERLAP_THRESHOLD {
                    let entry = shared
                        .entry(local_fp.event_hash.clone())
                        .or_insert_with(|| SharedEvent {
                            event_title: local_fp.display_title.clone(),
                            start_time: local_fp.start_time.clone(),
                            end_time: local_fp.end_time.clone(),
                            location: local_fp.location_tokens.join(" "),
                            connection_count: 0,
                            connection_pseudonyms: Vec::new(),
                            match_score: 0.0,
                        });

                    if !entry
                        .connection_pseudonyms
                        .contains(&peer_set.peer_pseudonym)
                    {
                        entry.connection_count += 1;
                        entry
                            .connection_pseudonyms
                            .push(peer_set.peer_pseudonym.clone());
                        // Keep highest match score
                        if score > entry.match_score {
                            entry.match_score = score;
                        }
                    }
                }
            }
        }
    }

    let mut result: Vec<SharedEvent> = shared.into_values().collect();
    result.sort_by(|a, b| b.connection_count.cmp(&a.connection_count));
    result
}

/// Get the list of accepted connection pseudonyms (mutual connections).
/// Returns pseudonyms from both received-accepted and sent-accepted requests.
pub async fn get_accepted_connections(store: &dyn KvStore) -> Result<Vec<String>, String> {
    let mut pseudonyms = Vec::new();

    // Received requests that were accepted
    let received = super::connection::list_received_requests(store).await?;
    for req in received {
        if req.status == "accepted" {
            pseudonyms.push(req.sender_pseudonym);
        }
    }

    // Sent requests that were accepted
    let sent = super::connection::list_sent_requests(store).await?;
    for req in sent {
        if req.status == "accepted" {
            pseudonyms.push(req.target_pseudonym);
        }
    }

    Ok(pseudonyms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize() {
        let tokens = tokenize("RustConf 2026 - Portland, OR");
        assert!(tokens.contains(&"rustconf".to_string()));
        assert!(tokens.contains(&"2026".to_string()));
        assert!(tokens.contains(&"portland".to_string()));
        assert!(tokens.contains(&"or".to_string()));
    }

    #[test]
    fn test_tokenize_empty() {
        assert!(tokenize("").is_empty());
        assert!(tokenize("   ").is_empty());
    }

    #[test]
    fn test_fingerprint_event() {
        let fp = fingerprint_event(
            "RustConf 2026",
            "2026-06-15 09:00:00",
            "2026-06-17 17:00:00",
            "Portland, OR",
            "Conferences",
        );
        assert!(!fp.event_hash.is_empty());
        assert!(fp.title_tokens.contains(&"rustconf".to_string()));
        assert!(fp.location_tokens.contains(&"portland".to_string()));
    }

    #[test]
    fn test_fingerprint_deterministic() {
        let fp1 = fingerprint_event(
            "Event",
            "2026-01-01 00:00:00",
            "2026-01-02 00:00:00",
            "",
            "Cal",
        );
        let fp2 = fingerprint_event(
            "Event",
            "2026-01-01 00:00:00",
            "2026-01-02 00:00:00",
            "",
            "Cal",
        );
        assert_eq!(fp1.event_hash, fp2.event_hash);
    }

    #[test]
    fn test_jaccard_similarity_identical() {
        let a = vec!["rust".to_string(), "conf".to_string()];
        let b = vec!["rust".to_string(), "conf".to_string()];
        assert!((jaccard_similarity(&a, &b) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_jaccard_similarity_disjoint() {
        let a = vec!["rust".to_string()];
        let b = vec!["python".to_string()];
        assert!((jaccard_similarity(&a, &b)).abs() < f64::EPSILON);
    }

    #[test]
    fn test_jaccard_similarity_partial() {
        let a = vec!["rust".to_string(), "conf".to_string(), "2026".to_string()];
        let b = vec!["rust".to_string(), "conf".to_string(), "2025".to_string()];
        // intersection = 2, union = 4 → 0.5
        assert!((jaccard_similarity(&a, &b) - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_jaccard_empty_sets() {
        let empty: Vec<String> = vec![];
        assert!((jaccard_similarity(&empty, &empty)).abs() < f64::EPSILON);
    }

    #[test]
    fn test_date_overlap_full() {
        let score = date_overlap_score(
            "2026-06-15 09:00:00",
            "2026-06-15 17:00:00",
            "2026-06-15 09:00:00",
            "2026-06-15 17:00:00",
        );
        assert!((score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_date_overlap_partial() {
        let score = date_overlap_score(
            "2026-06-15 09:00:00",
            "2026-06-15 17:00:00",
            "2026-06-15 13:00:00",
            "2026-06-15 21:00:00",
        );
        // overlap: 13:00–17:00 = 4h, max duration = 8h → 0.5
        assert!((score - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_date_no_overlap() {
        let score = date_overlap_score(
            "2026-06-15 09:00:00",
            "2026-06-15 17:00:00",
            "2026-06-16 09:00:00",
            "2026-06-16 17:00:00",
        );
        assert!((score).abs() < f64::EPSILON);
    }

    #[test]
    fn test_event_similarity_same_event() {
        let a = fingerprint_event(
            "RustConf 2026",
            "2026-06-15 09:00:00",
            "2026-06-17 17:00:00",
            "Portland Convention Center",
            "Cal",
        );
        let b = fingerprint_event(
            "RustConf 2026",
            "2026-06-15 09:00:00",
            "2026-06-17 17:00:00",
            "Portland Convention Center",
            "Work",
        );
        let score = event_similarity(&a, &b);
        assert!(
            score > OVERLAP_THRESHOLD,
            "Same event should exceed threshold: {}",
            score
        );
    }

    #[test]
    fn test_event_similarity_different_events() {
        let a = fingerprint_event(
            "RustConf 2026",
            "2026-06-15 09:00:00",
            "2026-06-17 17:00:00",
            "Portland, OR",
            "Cal",
        );
        let b = fingerprint_event(
            "PyCon 2026",
            "2026-09-20 09:00:00",
            "2026-09-22 17:00:00",
            "Pittsburgh, PA",
            "Cal",
        );
        let score = event_similarity(&a, &b);
        assert!(
            score < OVERLAP_THRESHOLD,
            "Different events should be below threshold: {}",
            score
        );
    }

    #[test]
    fn test_event_similarity_same_time_different_title() {
        let a = fingerprint_event(
            "Team Standup",
            "2026-06-15 09:00:00",
            "2026-06-15 09:15:00",
            "Zoom",
            "Work",
        );
        let b = fingerprint_event(
            "Board Meeting",
            "2026-06-15 09:00:00",
            "2026-06-15 10:00:00",
            "Conference Room",
            "Work",
        );
        let score = event_similarity(&a, &b);
        // Same time but different event — should be below threshold
        assert!(
            score < OVERLAP_THRESHOLD,
            "Different events at same time: {}",
            score
        );
    }

    #[test]
    fn test_detect_shared_events() {
        let local = vec![
            fingerprint_event(
                "RustConf 2026",
                "2026-06-15 09:00:00",
                "2026-06-17 17:00:00",
                "Portland Convention Center",
                "Cal",
            ),
            fingerprint_event(
                "Daily Standup",
                "2026-06-15 09:00:00",
                "2026-06-15 09:15:00",
                "Zoom",
                "Work",
            ),
        ];

        let peer_sets = vec![
            PeerEventSet {
                peer_pseudonym: "peer-a".to_string(),
                fingerprints: vec![fingerprint_event(
                    "RustConf 2026",
                    "2026-06-15 09:00:00",
                    "2026-06-17 17:00:00",
                    "Portland Convention Center",
                    "Conferences",
                )],
                updated_at: "2026-03-29T00:00:00Z".to_string(),
            },
            PeerEventSet {
                peer_pseudonym: "peer-b".to_string(),
                fingerprints: vec![fingerprint_event(
                    "RustConf 2026",
                    "2026-06-14 18:00:00",
                    "2026-06-17 17:00:00",
                    "Portland OR",
                    "Events",
                )],
                updated_at: "2026-03-29T00:00:00Z".to_string(),
            },
        ];

        let shared = detect_shared_events(&local, &peer_sets);
        assert_eq!(
            shared.len(),
            1,
            "Should detect exactly one shared event (RustConf)"
        );
        assert_eq!(shared[0].connection_count, 2);
        assert!(shared[0].event_title.contains("RustConf"));
    }

    #[test]
    fn test_detect_shared_events_no_matches() {
        let local = vec![fingerprint_event(
            "Personal Dentist",
            "2026-06-15 14:00:00",
            "2026-06-15 15:00:00",
            "123 Main St",
            "Personal",
        )];
        let peer_sets = vec![PeerEventSet {
            peer_pseudonym: "peer-a".to_string(),
            fingerprints: vec![fingerprint_event(
                "RustConf 2026",
                "2026-06-15 09:00:00",
                "2026-06-17 17:00:00",
                "Portland",
                "Cal",
            )],
            updated_at: "2026-03-29T00:00:00Z".to_string(),
        }];

        let shared = detect_shared_events(&local, &peer_sets);
        assert!(shared.is_empty(), "Should not match unrelated events");
    }
}
