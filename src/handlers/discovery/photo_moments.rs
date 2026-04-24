//! Photo moment detection handlers, extracted from the discovery module.

use super::get_metadata_store;
use super::MAX_PHOTO_BATCH;
use crate::discovery::moments;
use crate::discovery::types::{
    MomentHashReceiveRequest, MomentOptInRequest, MomentOptOutRequest, PhotoMetadata,
};
use crate::fold_node::node::FoldNode;
use crate::fold_node::OperationProcessor;
use crate::handlers::response::{
    get_db_guard, ApiResponse, HandlerError, HandlerResult, IntoHandlerError,
};
use crate::trust::contact_book::{Contact, ContactBook};
use serde::{Deserialize, Serialize};

/// Outcome of the moment-peer authorization check.
///
/// Pure-function result so the gate can be unit tested without standing up a
/// full `FoldNode`. Matches the pattern established in PR #420 for
/// `authorize_data_share_sender`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MomentPeerAuthz {
    /// Peer pseudonym matches an active (non-revoked) contact.
    Authorized,
    /// Peer pseudonym does not match any contact in the book.
    UnknownPeer,
    /// Peer pseudonym matches a contact whose trust has been revoked.
    RevokedPeer,
}

/// Match a `peer_pseudonym` against any contact in `book`, checking all three
/// pseudonym fields on [`Contact`]: `identity_pseudonym` (stable, preferred),
/// `messaging_pseudonym`, and the legacy discovery `pseudonym`. Includes
/// revoked contacts — callers must inspect `contact.revoked`.
fn find_contact_by_pseudonym<'a>(
    book: &'a ContactBook,
    peer_pseudonym: &str,
) -> Option<&'a Contact> {
    book.contacts.values().find(|c| {
        c.identity_pseudonym.as_deref() == Some(peer_pseudonym)
            || c.messaging_pseudonym.as_deref() == Some(peer_pseudonym)
            || c.pseudonym.as_deref() == Some(peer_pseudonym)
    })
}

/// Pure authorization helper: does `peer_pseudonym` correspond to a known,
/// non-revoked contact in `contact_book`? Matches any of the three pseudonym
/// fields on [`Contact`]. Matches the pattern of `authorize_data_share_sender`
/// in PR #420, but keyed on pseudonym (not pubkey) because the moment opt-in
/// payload carries a pseudonym.
pub(crate) fn authorize_moment_peer(
    contact_book: &ContactBook,
    peer_pseudonym: &str,
) -> MomentPeerAuthz {
    match find_contact_by_pseudonym(contact_book, peer_pseudonym) {
        None => MomentPeerAuthz::UnknownPeer,
        Some(c) if c.revoked => MomentPeerAuthz::RevokedPeer,
        Some(_) => MomentPeerAuthz::Authorized,
    }
}

#[cfg(feature = "ts-bindings")]
use ts_rs::TS;

// === Photo Moment Detection Handlers ===

/// Response for moment opt-in list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(
    feature = "ts-bindings",
    ts(export, export_to = "src/fold_node/static-react/src/types/")
)]
pub struct MomentOptInListResponse {
    pub opt_ins: Vec<moments::MomentOptIn>,
}

/// Response for shared moments list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(
    feature = "ts-bindings",
    ts(export, export_to = "src/fold_node/static-react/src/types/")
)]
pub struct SharedMomentsResponse {
    pub moments: Vec<moments::SharedMoment>,
}

/// Response for moment hash scan.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(
    feature = "ts-bindings",
    ts(export, export_to = "src/fold_node/static-react/src/types/")
)]
pub struct MomentScanResponse {
    pub photos_scanned: usize,
    pub hashes_generated: usize,
    pub peers_processed: usize,
}

/// Response for moment detection run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(
    feature = "ts-bindings",
    ts(export, export_to = "src/fold_node/static-react/src/types/")
)]
pub struct MomentDetectResponse {
    pub new_moments_found: usize,
    pub moments: Vec<moments::SharedMoment>,
}

/// Opt-in to photo moment sharing with a peer.
///
/// Trust-boundary gate: the target `peer_pseudonym` MUST correspond to a
/// known, non-revoked contact in this node's contact book. Without this
/// check an attacker could spray `moment_opt_in` requests for arbitrary
/// pseudonyms and pollute Sled — same class of bug as the `data_share`
/// vulnerability closed by PR #420.
pub async fn moment_opt_in(
    req: &MomentOptInRequest,
    node: &FoldNode,
) -> HandlerResult<MomentOptInListResponse> {
    // Authorization gate: load contact book and match the requested peer
    // pseudonym against known contacts. See `authorize_moment_peer`.
    let op = OperationProcessor::from_ref(node);
    let contact_book = op
        .load_contact_book()
        .await
        .map_err(|e| HandlerError::Internal(format!("load contact book: {}", e)))?;
    match authorize_moment_peer(&contact_book, &req.peer_pseudonym) {
        MomentPeerAuthz::Authorized => {}
        MomentPeerAuthz::UnknownPeer => {
            return Err(HandlerError::Unauthorized(format!(
                "moment_opt_in: peer pseudonym '{}' is not a known contact",
                req.peer_pseudonym
            )));
        }
        MomentPeerAuthz::RevokedPeer => {
            return Err(HandlerError::Unauthorized(format!(
                "moment_opt_in: peer pseudonym '{}' belongs to a revoked contact",
                req.peer_pseudonym
            )));
        }
    }

    let db = get_db_guard(node)?;
    let store = get_metadata_store(&db);

    let opt_in = moments::MomentOptIn {
        peer_pseudonym: req.peer_pseudonym.clone(),
        peer_display_name: req.peer_display_name.clone(),
        opted_in_at: chrono::Utc::now().to_rfc3339(),
    };

    moments::save_moment_opt_in(&*store, &opt_in)
        .await
        .handler_err("save moment opt-in")?;

    let opt_ins = moments::list_moment_opt_ins(&*store)
        .await
        .handler_err("list moment opt-ins")?;

    Ok(ApiResponse::success(MomentOptInListResponse { opt_ins }))
}

/// Opt-out of photo moment sharing with a peer.
pub async fn moment_opt_out(
    req: &MomentOptOutRequest,
    node: &FoldNode,
) -> HandlerResult<MomentOptInListResponse> {
    let db = get_db_guard(node)?;
    let store = get_metadata_store(&db);

    moments::remove_moment_opt_in(&*store, &req.peer_pseudonym)
        .await
        .handler_err("remove moment opt-in")?;

    let opt_ins = moments::list_moment_opt_ins(&*store)
        .await
        .handler_err("list moment opt-ins")?;

    Ok(ApiResponse::success(MomentOptInListResponse { opt_ins }))
}

/// List all moment opt-ins.
pub async fn moment_opt_in_list(node: &FoldNode) -> HandlerResult<MomentOptInListResponse> {
    let db = get_db_guard(node)?;
    let store = get_metadata_store(&db);

    let opt_ins = moments::list_moment_opt_ins(&*store)
        .await
        .handler_err("list moment opt-ins")?;

    Ok(ApiResponse::success(MomentOptInListResponse { opt_ins }))
}

/// Scan local photos and generate moment hashes for all opted-in peers.
pub async fn moment_scan(
    node: &FoldNode,
    master_key: &[u8],
    photo_metadata: &[PhotoMetadata],
) -> HandlerResult<MomentScanResponse> {
    let db = get_db_guard(node)?;
    let store = get_metadata_store(&db);

    if photo_metadata.len() > MAX_PHOTO_BATCH {
        return Err(HandlerError::BadRequest(format!(
            "Too many photos in batch: {} (max {})",
            photo_metadata.len(),
            MAX_PHOTO_BATCH
        )));
    }

    let opt_ins = moments::list_moment_opt_ins(&*store)
        .await
        .handler_err("list moment opt-ins")?;

    if opt_ins.is_empty() {
        return Ok(ApiResponse::success(MomentScanResponse {
            photos_scanned: photo_metadata.len(),
            hashes_generated: 0,
            peers_processed: 0,
        }));
    }

    let our_pseudo_hash = crate::discovery::pseudonym::content_hash("moment-sharing");
    let our_pseudonym = crate::discovery::pseudonym::derive_pseudonym(master_key, &our_pseudo_hash);
    let our_pseudonym_str = our_pseudonym.to_string();

    let mut total_hashes = 0;

    for opt_in in &opt_ins {
        let shared_secret = moments::derive_peer_shared_secret(
            master_key,
            &our_pseudonym_str,
            &opt_in.peer_pseudonym,
        );

        let mut all_hashes = Vec::new();

        for photo in photo_metadata {
            let ts = photo
                .timestamp
                .parse::<chrono::DateTime<chrono::Utc>>()
                .map_err(|e| {
                    HandlerError::BadRequest(format!(
                        "Invalid timestamp '{}': {}",
                        photo.timestamp, e
                    ))
                })?;

            let hashes = moments::generate_moment_hashes(
                &ts,
                photo.latitude,
                photo.longitude,
                &photo.record_id,
                &shared_secret,
            );
            all_hashes.extend(hashes);
        }

        total_hashes += all_hashes.len();
        moments::save_our_moment_hashes(&*store, &opt_in.peer_pseudonym, &all_hashes)
            .await
            .handler_err("save moment hashes")?;
    }

    Ok(ApiResponse::success(MomentScanResponse {
        photos_scanned: photo_metadata.len(),
        hashes_generated: total_hashes,
        peers_processed: opt_ins.len(),
    }))
}

/// Receive moment hashes from a peer (via encrypted exchange).
pub async fn moment_receive_hashes(
    req: &MomentHashReceiveRequest,
    node: &FoldNode,
) -> HandlerResult<()> {
    let db = get_db_guard(node)?;
    let store = get_metadata_store(&db);

    let has_opt_in = moments::has_moment_opt_in(&*store, &req.sender_pseudonym)
        .await
        .handler_err("check moment opt-in")?;

    if !has_opt_in {
        return Err(HandlerError::BadRequest(format!(
            "No moment opt-in for peer {}. Both peers must opt-in first.",
            req.sender_pseudonym
        )));
    }

    let exchange = moments::MomentHashExchange {
        sender_pseudonym: req.sender_pseudonym.clone(),
        hashes: req.hashes.clone(),
        exchanged_at: chrono::Utc::now().to_rfc3339(),
    };

    moments::save_peer_moment_hashes(&*store, &req.sender_pseudonym, &exchange)
        .await
        .handler_err("save peer moment hashes")?;

    Ok(ApiResponse::success(()))
}

/// Detect shared moments by comparing our hashes with received peer hashes.
pub async fn moment_detect(node: &FoldNode) -> HandlerResult<MomentDetectResponse> {
    let db = get_db_guard(node)?;
    let store = get_metadata_store(&db);

    let opt_ins = moments::list_moment_opt_ins(&*store)
        .await
        .handler_err("list moment opt-ins")?;

    let mut all_new_moments = Vec::new();

    for opt_in in &opt_ins {
        let new_moments = moments::detect_shared_moments(
            &*store,
            &opt_in.peer_pseudonym,
            opt_in.peer_display_name.as_deref(),
        )
        .await
        .handler_err("detect shared moments")?;

        all_new_moments.extend(new_moments);
    }

    let count = all_new_moments.len();
    Ok(ApiResponse::success(MomentDetectResponse {
        new_moments_found: count,
        moments: all_new_moments,
    }))
}

/// List all detected shared moments.
pub async fn moment_list(node: &FoldNode) -> HandlerResult<SharedMomentsResponse> {
    let db = get_db_guard(node)?;
    let store = get_metadata_store(&db);

    let shared_moments = moments::list_shared_moments(&*store)
        .await
        .handler_err("list shared moments")?;

    Ok(ApiResponse::success(SharedMomentsResponse {
        moments: shared_moments,
    }))
}

#[cfg(test)]
mod moment_auth_gate_tests {
    use super::*;
    use crate::trust::contact_book::TrustDirection;
    use chrono::Utc;
    use std::collections::HashMap;

    fn insert_contact(book: &mut ContactBook, c: Contact) {
        // Use direct insert to preserve the `revoked` field, which
        // `upsert_contact` would reset to false.
        book.contacts.insert(c.public_key.clone(), c);
    }

    fn contact_with_pseudonyms(
        pk: &str,
        discovery_pseudo: Option<&str>,
        messaging_pseudo: Option<&str>,
        identity_pseudo: Option<&str>,
        revoked: bool,
    ) -> Contact {
        Contact {
            public_key: pk.to_string(),
            display_name: format!("contact-{}", pk),
            contact_hint: None,
            direction: TrustDirection::Mutual,
            connected_at: Utc::now(),
            pseudonym: discovery_pseudo.map(|s| s.to_string()),
            messaging_pseudonym: messaging_pseudo.map(|s| s.to_string()),
            messaging_public_key: None,
            identity_pseudonym: identity_pseudo.map(|s| s.to_string()),
            revoked,
            roles: HashMap::new(),
        }
    }

    #[test]
    fn empty_book_rejects_any_peer() {
        let book = ContactBook::new();
        assert_eq!(
            authorize_moment_peer(&book, "random-pseudonym"),
            MomentPeerAuthz::UnknownPeer
        );
    }

    #[test]
    fn matches_by_identity_pseudonym() {
        let mut book = ContactBook::new();
        insert_contact(
            &mut book,
            contact_with_pseudonyms("pk-alice", None, None, Some("id-alice"), false),
        );
        assert_eq!(
            authorize_moment_peer(&book, "id-alice"),
            MomentPeerAuthz::Authorized
        );
    }

    #[test]
    fn matches_by_messaging_pseudonym() {
        let mut book = ContactBook::new();
        insert_contact(
            &mut book,
            contact_with_pseudonyms("pk-bob", None, Some("msg-bob"), None, false),
        );
        assert_eq!(
            authorize_moment_peer(&book, "msg-bob"),
            MomentPeerAuthz::Authorized
        );
    }

    #[test]
    fn matches_by_discovery_pseudonym() {
        let mut book = ContactBook::new();
        insert_contact(
            &mut book,
            contact_with_pseudonyms("pk-carol", Some("disc-carol"), None, None, false),
        );
        assert_eq!(
            authorize_moment_peer(&book, "disc-carol"),
            MomentPeerAuthz::Authorized
        );
    }

    #[test]
    fn rejects_revoked_contact() {
        let mut book = ContactBook::new();
        insert_contact(
            &mut book,
            contact_with_pseudonyms("pk-dave", None, None, Some("id-dave"), true),
        );
        assert_eq!(
            authorize_moment_peer(&book, "id-dave"),
            MomentPeerAuthz::RevokedPeer
        );
    }

    #[test]
    fn rejects_unknown_pseudonym_when_others_exist() {
        let mut book = ContactBook::new();
        insert_contact(
            &mut book,
            contact_with_pseudonyms("pk-alice", None, None, Some("id-alice"), false),
        );
        assert_eq!(
            authorize_moment_peer(&book, "id-eve"),
            MomentPeerAuthz::UnknownPeer
        );
    }
}
