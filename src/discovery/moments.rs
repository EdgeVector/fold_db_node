//! Photo moment detection for connected peers.
//!
//! Detects photos taken at similar times (1-hour window) and locations (500m radius)
//! between mutually opted-in peers. Uses metadata hashing for privacy:
//! - Timestamps rounded to 1-hour buckets
//! - GPS coordinates encoded as geohash precision 6 (~1.2km x 0.6km cells)
//! - Only hashes are exchanged; raw metadata never leaves the device
//! - Overlap revealed only after mutual opt-in

use chrono::{DateTime, Datelike, Timelike, Utc};
use fold_db::storage::traits::KvStore;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use uuid::Uuid;

const MOMENT_OPTIN_PREFIX: &str = "discovery:moment_optin:";
const MOMENT_HASH_PREFIX: &str = "discovery:moment_hash:";
const SHARED_MOMENT_PREFIX: &str = "discovery:shared_moment:";
const PEER_MOMENT_PREFIX: &str = "discovery:peer_moment:";

type HmacSha256 = Hmac<Sha256>;

/// Opt-in record for photo moment sharing with a specific peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MomentOptIn {
    /// Pseudonym of the peer we opted in to share with
    pub peer_pseudonym: String,
    /// Display name for the peer (if known)
    pub peer_display_name: Option<String>,
    /// When we opted in
    pub opted_in_at: String,
}

/// A hashed photo moment fingerprint.
/// Generated from EXIF timestamp (1-hour bucket) + GPS (geohash precision 6).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct PhotoMomentHash {
    /// HMAC-SHA256 of (time_bucket || "|" || geohash), hex-encoded
    pub hash: String,
    /// The time bucket (ISO date + hour), e.g. "2026-03-15T14"
    /// Only stored locally, never sent to peers
    pub time_bucket: String,
    /// Geohash at precision 6, e.g. "9q8yyk"
    /// Only stored locally, never sent to peers
    pub geohash: String,
    /// Original photo timestamp (full precision)
    pub timestamp: String,
    /// Source molecule/record UUID
    pub record_id: String,
}

/// A detected shared moment between the local user and a peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedMoment {
    /// Unique ID for this shared moment
    pub moment_id: String,
    /// Peer's pseudonym
    pub peer_pseudonym: String,
    /// Peer's display name (if known)
    pub peer_display_name: Option<String>,
    /// The matching time bucket
    pub time_bucket: String,
    /// The matching geohash (rough location)
    pub geohash: String,
    /// Human-readable location name (reverse-geocoded, if available)
    pub location_name: Option<String>,
    /// Our photo record ID
    pub our_record_id: String,
    /// Our photo timestamp
    pub our_timestamp: String,
    /// Peer's photo timestamp (from their hash exchange)
    pub peer_timestamp: Option<String>,
    /// When this shared moment was detected
    pub detected_at: String,
}

/// Hashes exchanged with a peer (encrypted via bulletin board).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MomentHashExchange {
    /// The sender's pseudonym
    pub sender_pseudonym: String,
    /// List of moment hashes (only the hash field, not local metadata)
    pub hashes: Vec<String>,
    /// Timestamp of the exchange
    pub exchanged_at: String,
}

/// Generate a time bucket string from a timestamp.
/// Rounds down to the nearest hour: "2026-03-15T14"
pub fn time_bucket(timestamp: &DateTime<Utc>) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}",
        timestamp.year(),
        timestamp.month(),
        timestamp.day(),
        timestamp.hour()
    )
}

/// Generate adjacent time buckets for a timestamp (the hour before and after).
/// This handles the edge case where two photos are taken 30 minutes apart
/// but fall into different hour buckets.
pub fn adjacent_time_buckets(timestamp: &DateTime<Utc>) -> Vec<String> {
    let prev = *timestamp - chrono::Duration::hours(1);
    let next = *timestamp + chrono::Duration::hours(1);
    vec![time_bucket(&prev), time_bucket(timestamp), time_bucket(&next)]
}

/// Encode GPS coordinates as a geohash at precision 6 (~1.2km x 0.6km).
/// Returns None if coordinates are invalid.
pub fn encode_location(latitude: f64, longitude: f64) -> Option<String> {
    if !(-90.0..=90.0).contains(&latitude) || !(-180.0..=180.0).contains(&longitude) {
        return None;
    }
    // Precision 6 gives ~1.2km x 0.6km cells — close to 500m target
    geohash::encode(geohash::Coord { x: longitude, y: latitude }, 6).ok()
}

/// Get neighboring geohash cells (for 500m radius matching).
/// A geohash-6 cell is ~1.2km x 0.6km, so checking neighbors covers ~500m overlap.
pub fn neighboring_geohashes(gh: &str) -> Vec<String> {
    let mut result = vec![gh.to_string()];
    if let Ok(neighbors) = geohash::neighbors(gh) {
        result.push(neighbors.n);
        result.push(neighbors.ne);
        result.push(neighbors.e);
        result.push(neighbors.se);
        result.push(neighbors.s);
        result.push(neighbors.sw);
        result.push(neighbors.w);
        result.push(neighbors.nw);
    }
    result
}

/// Generate a moment hash using HMAC-SHA256.
/// The shared_secret is derived per-peer-pair to prevent cross-peer correlation.
pub fn compute_moment_hash(time_bucket: &str, geohash: &str, shared_secret: &[u8]) -> String {
    let input = format!("{}|{}", time_bucket, geohash);
    let mut mac =
        HmacSha256::new_from_slice(shared_secret).expect("HMAC accepts any key length");
    mac.update(input.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Derive a shared secret between two peers for moment hash computation.
/// Uses HKDF to combine the master key with both pseudonyms (sorted for consistency).
pub fn derive_peer_shared_secret(
    master_key: &[u8],
    our_pseudonym: &str,
    peer_pseudonym: &str,
) -> Vec<u8> {
    use hkdf::Hkdf;

    // Sort pseudonyms so both sides derive the same secret
    let (first, second) = if our_pseudonym < peer_pseudonym {
        (our_pseudonym, peer_pseudonym)
    } else {
        (peer_pseudonym, our_pseudonym)
    };

    let salt = format!("moment-shared-secret:{}:{}", first, second);
    let hk = Hkdf::<Sha256>::new(Some(salt.as_bytes()), master_key);
    let mut output = [0u8; 32];
    hk.expand(b"photo-moment-hmac", &mut output)
        .expect("32 bytes is a valid HKDF output length");
    output.to_vec()
}

/// Generate all moment hashes for a photo given its metadata.
/// Produces hashes for the primary time bucket + adjacent hours,
/// combined with the primary geohash + neighboring cells.
/// This ensures photos within 1 hour and ~500m will share at least one hash.
pub fn generate_moment_hashes(
    timestamp: &DateTime<Utc>,
    latitude: f64,
    longitude: f64,
    record_id: &str,
    shared_secret: &[u8],
) -> Vec<PhotoMomentHash> {
    let primary_gh = match encode_location(latitude, longitude) {
        Some(gh) => gh,
        None => return Vec::new(),
    };

    let time_buckets = adjacent_time_buckets(timestamp);
    let geohashes = neighboring_geohashes(&primary_gh);
    let primary_bucket = time_bucket(timestamp);

    let mut hashes = Vec::new();
    for tb in &time_buckets {
        for gh in &geohashes {
            let hash = compute_moment_hash(tb, gh, shared_secret);
            hashes.push(PhotoMomentHash {
                hash,
                time_bucket: primary_bucket.clone(),
                geohash: primary_gh.clone(),
                timestamp: timestamp.to_rfc3339(),
                record_id: record_id.to_string(),
            });
        }
    }

    hashes
}

/// Find matching hashes between our set and a peer's set.
/// Returns (our_hash, peer_hash_string) pairs for each match.
pub fn find_hash_overlaps(
    our_hashes: &[PhotoMomentHash],
    peer_hashes: &[String],
) -> Vec<PhotoMomentHash> {
    let peer_set: std::collections::HashSet<&str> =
        peer_hashes.iter().map(|s| s.as_str()).collect();

    our_hashes
        .iter()
        .filter(|h| peer_set.contains(h.hash.as_str()))
        .cloned()
        .collect()
}

// === Sled Storage ===

/// Save a moment opt-in for a specific peer.
pub async fn save_moment_opt_in(
    store: &dyn KvStore,
    opt_in: &MomentOptIn,
) -> Result<(), String> {
    let key = format!("{}{}", MOMENT_OPTIN_PREFIX, opt_in.peer_pseudonym);
    let value =
        serde_json::to_vec(opt_in).map_err(|e| format!("Failed to serialize opt-in: {}", e))?;
    store
        .put(key.as_bytes(), value)
        .await
        .map_err(|e| format!("Failed to save opt-in: {}", e))
}

/// Remove a moment opt-in for a specific peer.
pub async fn remove_moment_opt_in(
    store: &dyn KvStore,
    peer_pseudonym: &str,
) -> Result<(), String> {
    let key = format!("{}{}", MOMENT_OPTIN_PREFIX, peer_pseudonym);
    store
        .delete(key.as_bytes())
        .await
        .map(|_| ())
        .map_err(|e| format!("Failed to remove opt-in: {}", e))
}

/// List all moment opt-ins.
pub async fn list_moment_opt_ins(store: &dyn KvStore) -> Result<Vec<MomentOptIn>, String> {
    let entries = store
        .scan_prefix(MOMENT_OPTIN_PREFIX.as_bytes())
        .await
        .map_err(|e| format!("Failed to scan opt-ins: {}", e))?;

    let mut opt_ins = Vec::new();
    for (_key, value) in entries {
        match serde_json::from_slice(&value) {
            Ok(opt_in) => opt_ins.push(opt_in),
            Err(e) => log::warn!("Failed to deserialize moment opt-in: {}", e),
        }
    }
    Ok(opt_ins)
}

/// Check if we have a moment opt-in for a specific peer.
pub async fn has_moment_opt_in(
    store: &dyn KvStore,
    peer_pseudonym: &str,
) -> Result<bool, String> {
    let key = format!("{}{}", MOMENT_OPTIN_PREFIX, peer_pseudonym);
    let value = store
        .get(key.as_bytes())
        .await
        .map_err(|e| format!("Failed to check opt-in: {}", e))?;
    Ok(value.is_some())
}

/// Save our computed moment hashes for a peer (for later comparison).
pub async fn save_our_moment_hashes(
    store: &dyn KvStore,
    peer_pseudonym: &str,
    hashes: &[PhotoMomentHash],
) -> Result<(), String> {
    let key = format!("{}{}", MOMENT_HASH_PREFIX, peer_pseudonym);
    let value = serde_json::to_vec(hashes)
        .map_err(|e| format!("Failed to serialize moment hashes: {}", e))?;
    store
        .put(key.as_bytes(), value)
        .await
        .map_err(|e| format!("Failed to save moment hashes: {}", e))
}

/// Load our computed moment hashes for a peer.
pub async fn load_our_moment_hashes(
    store: &dyn KvStore,
    peer_pseudonym: &str,
) -> Result<Vec<PhotoMomentHash>, String> {
    let key = format!("{}{}", MOMENT_HASH_PREFIX, peer_pseudonym);
    let value = store
        .get(key.as_bytes())
        .await
        .map_err(|e| format!("Failed to load moment hashes: {}", e))?;
    match value {
        Some(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| format!("Failed to deserialize moment hashes: {}", e)),
        None => Ok(Vec::new()),
    }
}

/// Save peer's exchanged moment hashes.
pub async fn save_peer_moment_hashes(
    store: &dyn KvStore,
    peer_pseudonym: &str,
    exchange: &MomentHashExchange,
) -> Result<(), String> {
    let key = format!("{}{}", PEER_MOMENT_PREFIX, peer_pseudonym);
    let value = serde_json::to_vec(exchange)
        .map_err(|e| format!("Failed to serialize peer exchange: {}", e))?;
    store
        .put(key.as_bytes(), value)
        .await
        .map_err(|e| format!("Failed to save peer exchange: {}", e))
}

/// Load peer's exchanged moment hashes.
pub async fn load_peer_moment_hashes(
    store: &dyn KvStore,
    peer_pseudonym: &str,
) -> Result<Option<MomentHashExchange>, String> {
    let key = format!("{}{}", PEER_MOMENT_PREFIX, peer_pseudonym);
    let value = store
        .get(key.as_bytes())
        .await
        .map_err(|e| format!("Failed to load peer exchange: {}", e))?;
    match value {
        Some(bytes) => {
            let exchange = serde_json::from_slice(&bytes)
                .map_err(|e| format!("Failed to deserialize peer exchange: {}", e))?;
            Ok(Some(exchange))
        }
        None => Ok(None),
    }
}

/// Save a detected shared moment.
pub async fn save_shared_moment(
    store: &dyn KvStore,
    moment: &SharedMoment,
) -> Result<(), String> {
    let key = format!("{}{}", SHARED_MOMENT_PREFIX, moment.moment_id);
    let value = serde_json::to_vec(moment)
        .map_err(|e| format!("Failed to serialize shared moment: {}", e))?;
    store
        .put(key.as_bytes(), value)
        .await
        .map_err(|e| format!("Failed to save shared moment: {}", e))
}

/// List all detected shared moments, sorted by detection time (newest first).
pub async fn list_shared_moments(store: &dyn KvStore) -> Result<Vec<SharedMoment>, String> {
    let entries = store
        .scan_prefix(SHARED_MOMENT_PREFIX.as_bytes())
        .await
        .map_err(|e| format!("Failed to scan shared moments: {}", e))?;

    let mut moments = Vec::new();
    for (_key, value) in entries {
        match serde_json::from_slice(&value) {
            Ok(moment) => moments.push(moment),
            Err(e) => log::warn!("Failed to deserialize shared moment: {}", e),
        }
    }

    moments.sort_by(|a: &SharedMoment, b: &SharedMoment| b.detected_at.cmp(&a.detected_at));
    Ok(moments)
}

/// Detect shared moments between our hashes and a peer's exchanged hashes.
/// Deduplicates by (time_bucket, geohash) to avoid duplicates from adjacent-cell matching.
pub async fn detect_shared_moments(
    store: &dyn KvStore,
    peer_pseudonym: &str,
    peer_display_name: Option<&str>,
) -> Result<Vec<SharedMoment>, String> {
    let our_hashes = load_our_moment_hashes(store, peer_pseudonym).await?;
    let peer_exchange = match load_peer_moment_hashes(store, peer_pseudonym).await? {
        Some(e) => e,
        None => return Ok(Vec::new()),
    };

    let matches = find_hash_overlaps(&our_hashes, &peer_exchange.hashes);

    // Deduplicate by (time_bucket, geohash) — adjacent cell/hour matches
    // produce the same underlying moment
    let mut seen = std::collections::HashSet::new();
    let mut new_moments = Vec::new();
    let now = Utc::now().to_rfc3339();

    for matched in &matches {
        let dedup_key = format!("{}|{}", matched.time_bucket, matched.geohash);
        if !seen.insert(dedup_key) {
            continue;
        }

        let moment = SharedMoment {
            moment_id: Uuid::new_v4().to_string(),
            peer_pseudonym: peer_pseudonym.to_string(),
            peer_display_name: peer_display_name.map(|s| s.to_string()),
            time_bucket: matched.time_bucket.clone(),
            geohash: matched.geohash.clone(),
            location_name: None, // Reverse geocoding is a future enhancement
            our_record_id: matched.record_id.clone(),
            our_timestamp: matched.timestamp.clone(),
            peer_timestamp: None,
            detected_at: now.clone(),
        };

        save_shared_moment(store, &moment).await?;
        new_moments.push(moment);
    }

    Ok(new_moments)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_time_bucket() {
        let ts = "2026-03-15T14:32:00Z".parse::<DateTime<Utc>>().unwrap();
        assert_eq!(time_bucket(&ts), "2026-03-15T14");
    }

    #[test]
    fn test_adjacent_time_buckets() {
        let ts = "2026-03-15T14:32:00Z".parse::<DateTime<Utc>>().unwrap();
        let buckets = adjacent_time_buckets(&ts);
        assert_eq!(buckets.len(), 3);
        assert_eq!(buckets[0], "2026-03-15T13");
        assert_eq!(buckets[1], "2026-03-15T14");
        assert_eq!(buckets[2], "2026-03-15T15");
    }

    #[test]
    fn test_encode_location() {
        // Golden Gate Park, SF
        let gh = encode_location(37.7694, -122.4862).unwrap();
        assert_eq!(gh.len(), 6);

        // Invalid coordinates
        assert!(encode_location(91.0, 0.0).is_none());
        assert!(encode_location(0.0, 181.0).is_none());
    }

    #[test]
    fn test_neighboring_geohashes() {
        let gh = encode_location(37.7694, -122.4862).unwrap();
        let neighbors = neighboring_geohashes(&gh);
        assert_eq!(neighbors.len(), 9); // center + 8 neighbors
        assert_eq!(neighbors[0], gh);
    }

    #[test]
    fn test_moment_hash_deterministic() {
        let secret = b"test-secret-key-for-moments";
        let h1 = compute_moment_hash("2026-03-15T14", "9q8yyk", secret);
        let h2 = compute_moment_hash("2026-03-15T14", "9q8yyk", secret);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_moment_hash_different_inputs() {
        let secret = b"test-secret-key-for-moments";
        let h1 = compute_moment_hash("2026-03-15T14", "9q8yyk", secret);
        let h2 = compute_moment_hash("2026-03-15T15", "9q8yyk", secret);
        assert_ne!(h1, h2);

        let h3 = compute_moment_hash("2026-03-15T14", "9q8yym", secret);
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_different_secrets_different_hashes() {
        let h1 = compute_moment_hash("2026-03-15T14", "9q8yyk", b"secret-a");
        let h2 = compute_moment_hash("2026-03-15T14", "9q8yyk", b"secret-b");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_derive_peer_shared_secret_symmetric() {
        let master = [0x42u8; 32];
        let s1 = derive_peer_shared_secret(&master, "alice", "bob");
        let s2 = derive_peer_shared_secret(&master, "bob", "alice");
        assert_eq!(s1, s2);
    }

    #[test]
    fn test_derive_peer_shared_secret_unique() {
        let master = [0x42u8; 32];
        let s1 = derive_peer_shared_secret(&master, "alice", "bob");
        let s2 = derive_peer_shared_secret(&master, "alice", "charlie");
        assert_ne!(s1, s2);
    }

    #[test]
    fn test_generate_moment_hashes() {
        let ts = "2026-03-15T14:32:00Z".parse::<DateTime<Utc>>().unwrap();
        let secret = b"test-secret";
        let hashes = generate_moment_hashes(&ts, 37.7694, -122.4862, "record-1", secret);

        // 3 time buckets * 9 geohash cells = 27 hashes
        assert_eq!(hashes.len(), 27);

        // All should have the primary time bucket and geohash stored locally
        for h in &hashes {
            assert_eq!(h.time_bucket, "2026-03-15T14");
            assert_eq!(h.record_id, "record-1");
        }
    }

    #[test]
    fn test_generate_moment_hashes_invalid_gps() {
        let ts = "2026-03-15T14:32:00Z".parse::<DateTime<Utc>>().unwrap();
        let hashes = generate_moment_hashes(&ts, 91.0, 0.0, "record-1", b"secret");
        assert!(hashes.is_empty());
    }

    #[test]
    fn test_find_hash_overlaps() {
        let secret = b"shared-secret";
        let ts1 = "2026-03-15T14:32:00Z".parse::<DateTime<Utc>>().unwrap();
        let ts2 = "2026-03-15T14:45:00Z".parse::<DateTime<Utc>>().unwrap();

        // Same location, same hour — should overlap
        let our = generate_moment_hashes(&ts1, 37.7694, -122.4862, "r1", secret);
        let peer = generate_moment_hashes(&ts2, 37.7694, -122.4862, "r2", secret);
        let peer_hash_strings: Vec<String> = peer.iter().map(|h| h.hash.clone()).collect();

        let overlaps = find_hash_overlaps(&our, &peer_hash_strings);
        assert!(!overlaps.is_empty(), "Same place+hour should have overlapping hashes");
    }

    #[test]
    fn test_find_hash_overlaps_no_match() {
        let secret = b"shared-secret";
        let ts1 = "2026-03-15T14:32:00Z".parse::<DateTime<Utc>>().unwrap();
        let ts2 = "2026-03-15T20:00:00Z".parse::<DateTime<Utc>>().unwrap();

        // Same location but 6 hours apart — should NOT overlap
        let our = generate_moment_hashes(&ts1, 37.7694, -122.4862, "r1", secret);
        let peer = generate_moment_hashes(&ts2, 37.7694, -122.4862, "r2", secret);
        let peer_hash_strings: Vec<String> = peer.iter().map(|h| h.hash.clone()).collect();

        let overlaps = find_hash_overlaps(&our, &peer_hash_strings);
        assert!(overlaps.is_empty(), "6 hours apart should not match");
    }

    #[test]
    fn test_find_hash_overlaps_nearby_location() {
        let secret = b"shared-secret";
        let ts = "2026-03-15T14:32:00Z".parse::<DateTime<Utc>>().unwrap();

        // Two points ~200m apart in Golden Gate Park
        let our = generate_moment_hashes(&ts, 37.7694, -122.4862, "r1", secret);
        let peer = generate_moment_hashes(&ts, 37.7696, -122.4842, "r2", secret);
        let peer_hash_strings: Vec<String> = peer.iter().map(|h| h.hash.clone()).collect();

        let overlaps = find_hash_overlaps(&our, &peer_hash_strings);
        assert!(!overlaps.is_empty(), "~200m apart should have overlapping hashes via neighbors");
    }
}
