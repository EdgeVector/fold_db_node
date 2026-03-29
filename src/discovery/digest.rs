//! Weekly digest generation for social activity.
//!
//! Collects social activity from the past week (discovery matches, connection
//! activity, shared moments, interest fingerprint changes) and generates a
//! human-readable summary via LLM. Digests are stored in Sled and served to the
//! dashboard.

use chrono::{DateTime, Duration, Utc};
use fold_db::storage::traits::KvStore;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const DIGEST_PREFIX: &str = "discovery:digest:";
const DIGEST_LATEST_KEY: &str = "discovery:digest:latest";
const PREVIOUS_PROFILE_KEY: &str = "discovery:digest:prev_profile";

/// A single section of the weekly digest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DigestSection {
    /// Section title (e.g., "New Discovery Matches")
    pub title: String,
    /// Section content lines
    pub items: Vec<String>,
    /// Icon hint for frontend rendering
    pub icon: String,
}

/// A generated weekly digest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeeklyDigest {
    /// Unique digest ID (ISO week string, e.g. "2026-W13")
    pub digest_id: String,
    /// Period start (inclusive)
    pub period_start: DateTime<Utc>,
    /// Period end (exclusive)
    pub period_end: DateTime<Utc>,
    /// Digest sections
    pub sections: Vec<DigestSection>,
    /// LLM-generated natural-language summary
    pub summary: String,
    /// When this digest was generated
    pub generated_at: DateTime<Utc>,
}

/// Raw activity data collected for digest generation.
#[derive(Debug, Clone, Default)]
pub struct DigestActivityData {
    pub new_similar_profiles: usize,
    pub top_shared_categories: Vec<String>,
    pub accepted_connections: Vec<String>,
    pub declined_connections: Vec<String>,
    pub pending_connections: usize,
    pub new_shared_moments: Vec<SharedMomentSummary>,
    pub interest_changes: Vec<InterestChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedMomentSummary {
    pub peer_display_name: Option<String>,
    pub peer_pseudonym: String,
    pub location_name: Option<String>,
    pub time_bucket: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterestChange {
    pub category: String,
    pub change_type: InterestChangeType,
    pub old_rank: Option<usize>,
    pub new_rank: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum InterestChangeType {
    /// New interest appeared in the profile
    New,
    /// Interest disappeared from the profile
    Removed,
    /// Interest moved up in rank
    RankUp,
    /// Interest moved down in rank
    RankDown,
}

/// Collect activity data from the stores for the given time window.
pub async fn collect_activity(
    metadata_store: &dyn KvStore,
    period_start: DateTime<Utc>,
) -> Result<DigestActivityData, String> {
    let mut data = DigestActivityData::default();
    let cutoff = period_start.to_rfc3339();

    // 1. Connection activity: scan received requests
    let received = super::connection::list_received_requests(metadata_store).await?;
    for req in &received {
        if req.created_at >= cutoff || req.responded_at.as_deref().is_some_and(|r| r >= cutoff.as_str()) {
            match req.status.as_str() {
                "accepted" => {
                    data.accepted_connections.push(
                        req.sender_pseudonym
                            .chars()
                            .take(8)
                            .collect::<String>(),
                    );
                }
                "declined" => {
                    data.declined_connections.push(
                        req.sender_pseudonym
                            .chars()
                            .take(8)
                            .collect::<String>(),
                    );
                }
                "pending" => {
                    data.pending_connections += 1;
                }
                _ => {}
            }
        }
    }

    // Also check sent requests for accepted responses
    let sent = super::connection::list_sent_requests(metadata_store).await?;
    for req in &sent {
        if req.created_at >= cutoff && req.status == "accepted" {
            let short = req.target_pseudonym.chars().take(8).collect::<String>();
            if !data.accepted_connections.contains(&short) {
                data.accepted_connections.push(short);
            }
        }
    }

    // 2. Shared moments this week
    let moments = super::moments::list_shared_moments(metadata_store).await?;
    for m in &moments {
        if m.detected_at >= cutoff {
            data.new_shared_moments.push(SharedMomentSummary {
                peer_display_name: m.peer_display_name.clone(),
                peer_pseudonym: m.peer_pseudonym.chars().take(8).collect(),
                location_name: m.location_name.clone(),
                time_bucket: m.time_bucket.clone(),
            });
        }
    }

    // 3. Interest fingerprint changes
    let current_profile = super::interests::load_interest_profile(metadata_store).await?;
    let previous_profile = load_previous_profile(metadata_store).await?;

    if let Some(current) = &current_profile {
        data.interest_changes = compute_interest_changes(
            previous_profile.as_ref().map(|p| &p.categories[..]).unwrap_or(&[]),
            &current.categories,
        );

        // Save current as previous for next digest
        save_previous_profile(metadata_store, current).await?;
    }

    Ok(data)
}

/// Compute interest changes between two profile snapshots.
fn compute_interest_changes(
    old: &[super::interests::InterestCategory],
    new: &[super::interests::InterestCategory],
) -> Vec<InterestChange> {
    let mut changes = Vec::new();

    // Build rank maps (position in sorted-by-count order)
    let old_ranks: std::collections::HashMap<&str, usize> = old
        .iter()
        .enumerate()
        .map(|(i, c)| (c.name.as_str(), i))
        .collect();

    let new_ranks: std::collections::HashMap<&str, usize> = new
        .iter()
        .enumerate()
        .map(|(i, c)| (c.name.as_str(), i))
        .collect();

    // Check for new or moved categories
    for (name, &new_rank) in &new_ranks {
        match old_ranks.get(name) {
            None => {
                changes.push(InterestChange {
                    category: name.to_string(),
                    change_type: InterestChangeType::New,
                    old_rank: None,
                    new_rank: Some(new_rank),
                });
            }
            Some(&old_rank) if new_rank < old_rank => {
                changes.push(InterestChange {
                    category: name.to_string(),
                    change_type: InterestChangeType::RankUp,
                    old_rank: Some(old_rank),
                    new_rank: Some(new_rank),
                });
            }
            Some(&old_rank) if new_rank > old_rank => {
                changes.push(InterestChange {
                    category: name.to_string(),
                    change_type: InterestChangeType::RankDown,
                    old_rank: Some(old_rank),
                    new_rank: Some(new_rank),
                });
            }
            _ => {}
        }
    }

    // Check for removed categories
    for (name, &old_rank) in &old_ranks {
        if !new_ranks.contains_key(name) {
            changes.push(InterestChange {
                category: name.to_string(),
                change_type: InterestChangeType::Removed,
                old_rank: Some(old_rank),
                new_rank: None,
            });
        }
    }

    changes
}

/// Build digest sections from collected activity data.
pub fn build_sections(data: &DigestActivityData) -> Vec<DigestSection> {
    let mut sections = Vec::new();

    // Discovery matches
    if data.new_similar_profiles > 0 || !data.top_shared_categories.is_empty() {
        let mut items = Vec::new();
        if data.new_similar_profiles > 0 {
            items.push(format!(
                "{} new {} share your interests",
                data.new_similar_profiles,
                if data.new_similar_profiles == 1 {
                    "person"
                } else {
                    "people"
                }
            ));
        }
        if !data.top_shared_categories.is_empty() {
            items.push(format!(
                "Top shared interests: {}",
                data.top_shared_categories.join(", ")
            ));
        }
        sections.push(DigestSection {
            title: "New Discovery Matches".to_string(),
            items,
            icon: "search".to_string(),
        });
    }

    // Connection activity
    {
        let mut items = Vec::new();
        for name in &data.accepted_connections {
            items.push(format!("Connection with {}... accepted", name));
        }
        for name in &data.declined_connections {
            items.push(format!("Connection with {}... declined", name));
        }
        if data.pending_connections > 0 {
            items.push(format!(
                "{} pending connection {}",
                data.pending_connections,
                if data.pending_connections == 1 {
                    "request"
                } else {
                    "requests"
                }
            ));
        }
        if !items.is_empty() {
            sections.push(DigestSection {
                title: "Connection Activity".to_string(),
                items,
                icon: "users".to_string(),
            });
        }
    }

    // Shared moments
    if !data.new_shared_moments.is_empty() {
        let items = data
            .new_shared_moments
            .iter()
            .map(|m| {
                let who = m
                    .peer_display_name
                    .as_deref()
                    .map_or_else(|| format!("{}...", m.peer_pseudonym), |n| n.to_string());
                let where_str = m
                    .location_name
                    .as_deref()
                    .unwrap_or("a shared location");
                format!("You and {} were both at {}", who, where_str)
            })
            .collect();

        sections.push(DigestSection {
            title: "Shared Moments".to_string(),
            items,
            icon: "camera".to_string(),
        });
    }

    // Interest fingerprint changes
    if !data.interest_changes.is_empty() {
        let items = data
            .interest_changes
            .iter()
            .filter_map(|c| match c.change_type {
                InterestChangeType::New => {
                    Some(format!("New interest detected: {}", c.category))
                }
                InterestChangeType::Removed => {
                    Some(format!("{} dropped from your interests", c.category))
                }
                InterestChangeType::RankUp => {
                    if c.new_rank == Some(0) {
                        Some(format!(
                            "Your top interest shifted to {}",
                            c.category
                        ))
                    } else {
                        Some(format!("{} moved up in your interests", c.category))
                    }
                }
                InterestChangeType::RankDown => None, // Only highlight positive/notable changes
            })
            .collect::<Vec<_>>();

        if !items.is_empty() {
            sections.push(DigestSection {
                title: "Interest Fingerprint Changes".to_string(),
                items,
                icon: "fingerprint".to_string(),
            });
        }
    }

    sections
}

/// Generate a natural-language summary from sections (without LLM — template-based).
/// This is used as the default; an LLM-enhanced summary can be layered on top.
pub fn generate_template_summary(sections: &[DigestSection]) -> String {
    if sections.is_empty() {
        return "Quiet week! No new social activity to report.".to_string();
    }

    let mut parts = Vec::new();
    for section in sections {
        if !section.items.is_empty() {
            parts.push(format!(
                "{}: {}",
                section.title,
                section.items.join(". ")
            ));
        }
    }

    if parts.is_empty() {
        "Quiet week! No new social activity to report.".to_string()
    } else {
        parts.join(" | ")
    }
}

/// Generate a weekly digest, collecting data and building sections.
pub async fn generate_digest(
    metadata_store: &dyn KvStore,
) -> Result<WeeklyDigest, String> {
    let now = Utc::now();
    let period_end = now;
    let period_start = now - Duration::days(7);

    let data = collect_activity(metadata_store, period_start).await?;
    let sections = build_sections(&data);
    let summary = generate_template_summary(&sections);

    let digest_id = format!("{}", now.format("%G-W%V"));

    let digest = WeeklyDigest {
        digest_id: digest_id.clone(),
        period_start,
        period_end,
        sections,
        summary,
        generated_at: now,
    };

    save_digest(metadata_store, &digest).await?;

    Ok(digest)
}

// === Storage ===

/// Save a digest to the store (both by ID and as "latest").
pub async fn save_digest(store: &dyn KvStore, digest: &WeeklyDigest) -> Result<(), String> {
    let bytes = serde_json::to_vec(digest)
        .map_err(|e| format!("Failed to serialize digest: {}", e))?;

    let key = format!("{}{}", DIGEST_PREFIX, digest.digest_id);
    store
        .put(key.as_bytes(), bytes.clone())
        .await
        .map_err(|e| format!("Failed to save digest: {}", e))?;

    store
        .put(DIGEST_LATEST_KEY.as_bytes(), bytes)
        .await
        .map_err(|e| format!("Failed to save latest digest pointer: {}", e))?;

    Ok(())
}

/// Load the most recent digest.
pub async fn load_latest_digest(store: &dyn KvStore) -> Result<Option<WeeklyDigest>, String> {
    let bytes = store
        .get(DIGEST_LATEST_KEY.as_bytes())
        .await
        .map_err(|e| format!("Failed to load latest digest: {}", e))?;

    match bytes {
        Some(b) => {
            let digest: WeeklyDigest = serde_json::from_slice(&b)
                .map_err(|e| format!("Failed to deserialize digest: {}", e))?;
            Ok(Some(digest))
        }
        None => Ok(None),
    }
}

/// Load all stored digests, sorted newest first.
pub async fn list_digests(store: &dyn KvStore) -> Result<Vec<WeeklyDigest>, String> {
    let entries = store
        .scan_prefix(DIGEST_PREFIX.as_bytes())
        .await
        .map_err(|e| format!("Failed to scan digests: {}", e))?;

    let mut digests = Vec::new();
    for (key, value) in entries {
        // Skip the "latest" pointer
        if key == DIGEST_LATEST_KEY.as_bytes() {
            continue;
        }
        match serde_json::from_slice(&value) {
            Ok(d) => digests.push(d),
            Err(e) => log::warn!("Failed to deserialize digest: {}", e),
        }
    }

    digests.sort_by(|a: &WeeklyDigest, b: &WeeklyDigest| b.generated_at.cmp(&a.generated_at));
    Ok(digests)
}

/// Save the current interest profile as the "previous" snapshot for diff computation.
async fn save_previous_profile(
    store: &dyn KvStore,
    profile: &super::interests::InterestProfile,
) -> Result<(), String> {
    let bytes = serde_json::to_vec(profile)
        .map_err(|e| format!("Failed to serialize previous profile: {}", e))?;
    store
        .put(PREVIOUS_PROFILE_KEY.as_bytes(), bytes)
        .await
        .map_err(|e| format!("Failed to save previous profile: {}", e))
}

/// Load the previous interest profile snapshot.
async fn load_previous_profile(
    store: &dyn KvStore,
) -> Result<Option<super::interests::InterestProfile>, String> {
    let bytes = store
        .get(PREVIOUS_PROFILE_KEY.as_bytes())
        .await
        .map_err(|e| format!("Failed to load previous profile: {}", e))?;
    match bytes {
        Some(b) => {
            let profile = serde_json::from_slice(&b)
                .map_err(|e| format!("Failed to deserialize previous profile: {}", e))?;
            Ok(Some(profile))
        }
        None => Ok(None),
    }
}

/// Spawn a background task that generates a weekly digest on a timer.
/// Runs immediately on startup (if no digest exists for this week), then
/// every 24 hours checks if a new week's digest is needed.
///
/// Uses the NodeManager to get the default user's node. In local mode,
/// there is typically one user.
pub fn spawn_digest_scheduler(
    node_manager: Arc<crate::server::node_manager::NodeManager>,
) {
    tokio::spawn(async move {
        // Initial delay: let the system start up and the first user to log in
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;

        let mut interval = tokio::time::interval(std::time::Duration::from_secs(24 * 60 * 60));
        loop {
            interval.tick().await;

            let current_week = Utc::now().format("%G-W%V").to_string();

            // Get all active user nodes from the manager
            let user_ids = node_manager.list_users().await;
            if user_ids.is_empty() {
                log::debug!("Digest scheduler: no active users, skipping");
                continue;
            }

            for user_id in &user_ids {
                let node = match node_manager.get_node(user_id).await {
                    Ok(n) => n,
                    Err(e) => {
                        log::warn!(
                            "Digest scheduler: failed to get node for user {}: {}",
                            user_id,
                            e
                        );
                        continue;
                    }
                };

                let node_guard = node.read().await;
                let db = match node_guard.get_fold_db().await {
                    Ok(db) => db,
                    Err(e) => {
                        log::warn!("Digest scheduler: failed to access database: {}", e);
                        continue;
                    }
                };

                let metadata_store = db.get_db_ops().metadata_store().inner().clone();
                drop(db);
                drop(node_guard);

                // Check if we already have a digest for this week
                match load_latest_digest(&*metadata_store).await {
                    Ok(Some(d)) if d.digest_id == current_week => {
                        log::debug!(
                            "Digest scheduler: digest for {} already exists for user {}, skipping",
                            current_week,
                            user_id,
                        );
                        continue;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        log::warn!("Digest scheduler: failed to check latest digest: {}", e);
                        continue;
                    }
                }

                log::info!(
                    "Digest scheduler: generating digest {} for user {}",
                    current_week,
                    user_id,
                );
                match generate_digest(&*metadata_store).await {
                    Ok(digest) => {
                        log::info!(
                            "Digest scheduler: generated digest {} with {} sections",
                            digest.digest_id,
                            digest.sections.len()
                        );
                    }
                    Err(e) => {
                        log::error!("Digest scheduler: failed to generate digest: {}", e);
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_interest_changes_new_category() {
        let old = vec![];
        let new = vec![super::super::interests::InterestCategory {
            name: "Photography".to_string(),
            count: 10,
            avg_similarity: 0.5,
            enabled: true,
        }];

        let changes = compute_interest_changes(&old, &new);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].category, "Photography");
        assert_eq!(changes[0].change_type, InterestChangeType::New);
    }

    #[test]
    fn test_compute_interest_changes_removed_category() {
        let old = vec![super::super::interests::InterestCategory {
            name: "Cooking".to_string(),
            count: 5,
            avg_similarity: 0.4,
            enabled: true,
        }];
        let new = vec![];

        let changes = compute_interest_changes(&old, &new);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].category, "Cooking");
        assert_eq!(changes[0].change_type, InterestChangeType::Removed);
    }

    #[test]
    fn test_compute_interest_changes_rank_change() {
        let old = vec![
            super::super::interests::InterestCategory {
                name: "Running".to_string(),
                count: 20,
                avg_similarity: 0.6,
                enabled: true,
            },
            super::super::interests::InterestCategory {
                name: "Photography".to_string(),
                count: 10,
                avg_similarity: 0.5,
                enabled: true,
            },
        ];
        let new = vec![
            super::super::interests::InterestCategory {
                name: "Photography".to_string(),
                count: 25,
                avg_similarity: 0.55,
                enabled: true,
            },
            super::super::interests::InterestCategory {
                name: "Running".to_string(),
                count: 15,
                avg_similarity: 0.6,
                enabled: true,
            },
        ];

        let changes = compute_interest_changes(&old, &new);
        assert_eq!(changes.len(), 2);

        let photo_change = changes.iter().find(|c| c.category == "Photography").unwrap();
        assert_eq!(photo_change.change_type, InterestChangeType::RankUp);
        assert_eq!(photo_change.old_rank, Some(1));
        assert_eq!(photo_change.new_rank, Some(0));

        let running_change = changes.iter().find(|c| c.category == "Running").unwrap();
        assert_eq!(running_change.change_type, InterestChangeType::RankDown);
    }

    #[test]
    fn test_compute_interest_changes_no_changes() {
        let cats = vec![super::super::interests::InterestCategory {
            name: "Music".to_string(),
            count: 15,
            avg_similarity: 0.5,
            enabled: true,
        }];

        let changes = compute_interest_changes(&cats, &cats);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_build_sections_empty() {
        let data = DigestActivityData::default();
        let sections = build_sections(&data);
        assert!(sections.is_empty());
    }

    #[test]
    fn test_build_sections_connections() {
        let data = DigestActivityData {
            accepted_connections: vec!["abc12345".to_string()],
            pending_connections: 2,
            ..Default::default()
        };
        let sections = build_sections(&data);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].title, "Connection Activity");
        assert_eq!(sections[0].items.len(), 2);
    }

    #[test]
    fn test_build_sections_moments() {
        let data = DigestActivityData {
            new_shared_moments: vec![SharedMomentSummary {
                peer_display_name: Some("Alice".to_string()),
                peer_pseudonym: "abc12345".to_string(),
                location_name: Some("the farmers market".to_string()),
                time_bucket: "2026-03-28T10".to_string(),
            }],
            ..Default::default()
        };
        let sections = build_sections(&data);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].title, "Shared Moments");
        assert!(sections[0].items[0].contains("Alice"));
        assert!(sections[0].items[0].contains("farmers market"));
    }

    #[test]
    fn test_build_sections_interest_changes() {
        let data = DigestActivityData {
            interest_changes: vec![
                InterestChange {
                    category: "Photography".to_string(),
                    change_type: InterestChangeType::RankUp,
                    old_rank: Some(2),
                    new_rank: Some(0),
                },
                InterestChange {
                    category: "Cooking".to_string(),
                    change_type: InterestChangeType::New,
                    old_rank: None,
                    new_rank: Some(3),
                },
            ],
            ..Default::default()
        };
        let sections = build_sections(&data);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].title, "Interest Fingerprint Changes");
        assert!(sections[0].items[0].contains("Photography"));
        assert!(sections[0].items[1].contains("Cooking"));
    }

    #[test]
    fn test_generate_template_summary_empty() {
        let summary = generate_template_summary(&[]);
        assert!(summary.contains("Quiet week"));
    }

    #[test]
    fn test_generate_template_summary_with_sections() {
        let sections = vec![DigestSection {
            title: "Connection Activity".to_string(),
            items: vec!["Alice accepted your connection".to_string()],
            icon: "users".to_string(),
        }];
        let summary = generate_template_summary(&sections);
        assert!(summary.contains("Connection Activity"));
        assert!(summary.contains("Alice"));
    }
}
