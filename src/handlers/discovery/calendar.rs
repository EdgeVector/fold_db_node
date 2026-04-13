//! Calendar sharing handlers, extracted from the discovery module.

use super::{get_metadata_store, MAX_CALENDAR_BATCH};
use crate::discovery::calendar_sharing::{self, EventFingerprint, PeerEventSet, SharedEvent};
use crate::fold_node::node::FoldNode;
use crate::handlers::response::{ApiResponse, HandlerError, HandlerResult, IntoHandlerError};
use serde::{Deserialize, Serialize};

#[cfg(feature = "ts-bindings")]
use ts_rs::TS;

/// Request to sync calendar events for sharing.
#[derive(Debug, Clone, Deserialize)]
pub struct SyncCalendarEventsRequest {
    pub events: Vec<CalendarEventInput>,
}

/// A single calendar event from the client.
#[derive(Debug, Clone, Deserialize)]
pub struct CalendarEventInput {
    pub summary: String,
    pub start_time: String,
    pub end_time: String,
    pub location: String,
    pub calendar: String,
}

/// Request to store peer event fingerprints (received from a connection).
#[derive(Debug, Clone, Deserialize)]
pub struct StorePeerEventsRequest {
    pub peer_pseudonym: String,
    pub fingerprints: Vec<EventFingerprint>,
}

/// Response for calendar sharing status.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(
    feature = "ts-bindings",
    ts(export, export_to = "src/fold_node/static-react/src/types/")
)]
pub struct CalendarSharingStatusResponse {
    pub opted_in: bool,
    pub local_event_count: usize,
    pub peer_count: usize,
}

/// Response for syncing calendar events.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(
    feature = "ts-bindings",
    ts(export, export_to = "src/fold_node/static-react/src/types/")
)]
pub struct SyncCalendarEventsResponse {
    pub synced_count: usize,
}

/// Response for shared events detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-bindings", derive(TS))]
#[cfg_attr(
    feature = "ts-bindings",
    ts(export, export_to = "src/fold_node/static-react/src/types/")
)]
pub struct SharedEventsResponse {
    pub shared_events: Vec<SharedEvent>,
    pub connection_count: usize,
}

/// Get calendar sharing opt-in status.
pub async fn calendar_sharing_status(
    node: &FoldNode,
) -> HandlerResult<CalendarSharingStatusResponse> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    let opted_in = calendar_sharing::is_opted_in(&*store)
        .await
        .handler_err("check calendar sharing opt-in")?;

    let local_events = calendar_sharing::load_local_events(&*store)
        .await
        .handler_err("load local events")?;

    let peer_sets = calendar_sharing::load_all_peer_events(&*store)
        .await
        .handler_err("load peer events")?;

    Ok(ApiResponse::success(CalendarSharingStatusResponse {
        opted_in,
        local_event_count: local_events.len(),
        peer_count: peer_sets.len(),
    }))
}

/// Opt in to calendar sharing.
pub async fn calendar_sharing_opt_in(
    node: &FoldNode,
) -> HandlerResult<CalendarSharingStatusResponse> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    calendar_sharing::set_opt_in(&*store, true)
        .await
        .handler_err("opt in to calendar sharing")?;

    let local_events = calendar_sharing::load_local_events(&*store)
        .await
        .handler_err("load local events")?;

    let peer_sets = calendar_sharing::load_all_peer_events(&*store)
        .await
        .handler_err("load peer events")?;

    Ok(ApiResponse::success(CalendarSharingStatusResponse {
        opted_in: true,
        local_event_count: local_events.len(),
        peer_count: peer_sets.len(),
    }))
}

/// Opt out of calendar sharing.
pub async fn calendar_sharing_opt_out(
    node: &FoldNode,
) -> HandlerResult<CalendarSharingStatusResponse> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    calendar_sharing::set_opt_in(&*store, false)
        .await
        .handler_err("opt out of calendar sharing")?;

    // Purge all locally-stored calendar data on opt-out. Leaving stale entries
    // in Sled is a real privacy issue for a feature whose whole pitch is
    // "only reveal overlap existence."
    calendar_sharing::clear_all_calendar_data(&*store)
        .await
        .handler_err("purge calendar data on opt-out")?;

    Ok(ApiResponse::success(CalendarSharingStatusResponse {
        opted_in: false,
        local_event_count: 0,
        peer_count: 0,
    }))
}

/// Sync calendar events — fingerprint and store locally.
pub async fn sync_calendar_events(
    req: &SyncCalendarEventsRequest,
    node: &FoldNode,
) -> HandlerResult<SyncCalendarEventsResponse> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    if req.events.len() > MAX_CALENDAR_BATCH {
        return Err(HandlerError::BadRequest(format!(
            "Too many calendar events in batch: {} (max {})",
            req.events.len(),
            MAX_CALENDAR_BATCH
        )));
    }

    let opted_in = calendar_sharing::is_opted_in(&*store)
        .await
        .handler_err("check calendar sharing opt-in")?;

    if !opted_in {
        return Err(HandlerError::BadRequest(
            "Calendar sharing is not enabled. Opt in first.".to_string(),
        ));
    }

    let fingerprints: Vec<EventFingerprint> = req
        .events
        .iter()
        .map(|e| {
            calendar_sharing::fingerprint_event(
                &e.summary,
                &e.start_time,
                &e.end_time,
                &e.location,
                &e.calendar,
            )
        })
        .collect();

    let count = calendar_sharing::save_local_events(&*store, &fingerprints)
        .await
        .handler_err("save local events")?;

    Ok(ApiResponse::success(SyncCalendarEventsResponse {
        synced_count: count,
    }))
}

/// Store event fingerprints received from a peer connection.
pub async fn store_peer_events(
    req: &StorePeerEventsRequest,
    node: &FoldNode,
) -> HandlerResult<SyncCalendarEventsResponse> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    if req.fingerprints.len() > MAX_CALENDAR_BATCH {
        return Err(HandlerError::BadRequest(format!(
            "Too many peer event fingerprints in batch: {} (max {})",
            req.fingerprints.len(),
            MAX_CALENDAR_BATCH
        )));
    }

    let opted_in = calendar_sharing::is_opted_in(&*store)
        .await
        .handler_err("check calendar sharing opt-in")?;

    if !opted_in {
        return Err(HandlerError::BadRequest(
            "Calendar sharing is not enabled. Opt in first.".to_string(),
        ));
    }

    // Verify the peer is an accepted connection
    let connections = calendar_sharing::get_accepted_connections(&*store)
        .await
        .handler_err("load accepted connections")?;

    if !connections.contains(&req.peer_pseudonym) {
        return Err(HandlerError::BadRequest(
            "Peer is not an accepted connection.".to_string(),
        ));
    }

    let peer_set = PeerEventSet {
        peer_pseudonym: req.peer_pseudonym.clone(),
        fingerprints: req.fingerprints.clone(),
        updated_at: chrono::Utc::now().to_rfc3339(),
    };

    calendar_sharing::save_peer_events(&*store, &peer_set)
        .await
        .handler_err("save peer events")?;

    Ok(ApiResponse::success(SyncCalendarEventsResponse {
        synced_count: peer_set.fingerprints.len(),
    }))
}

/// Detect shared events between local calendar and peer calendars.
pub async fn get_shared_events(node: &FoldNode) -> HandlerResult<SharedEventsResponse> {
    let db = node
        .get_fold_db()
        .map_err(|e| HandlerError::Internal(format!("Failed to access database: {}", e)))?;
    let store = get_metadata_store(&db);

    let opted_in = calendar_sharing::is_opted_in(&*store)
        .await
        .handler_err("check calendar sharing opt-in")?;

    if !opted_in {
        return Ok(ApiResponse::success(SharedEventsResponse {
            shared_events: Vec::new(),
            connection_count: 0,
        }));
    }

    let local_events = calendar_sharing::load_local_events(&*store)
        .await
        .handler_err("load local events")?;

    let peer_sets = calendar_sharing::load_all_peer_events(&*store)
        .await
        .handler_err("load peer events")?;

    let connection_count = peer_sets.len();
    let shared_events = calendar_sharing::detect_shared_events(&local_events, &peer_sets);

    Ok(ApiResponse::success(SharedEventsResponse {
        shared_events,
        connection_count,
    }))
}
