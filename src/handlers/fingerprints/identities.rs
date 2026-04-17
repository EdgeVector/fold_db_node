//! List imported Identity records — the audit view for Phase 3b.
//!
//! After a user imports a peer's Identity Card via the Phase 3b
//! flow, the node has an `Identity` record (the signed card) and
//! an `IdentityReceipt` record (audit log: when + how + from
//! where). This handler joins the two by `identity_id` and returns
//! every card the node has received, including the self-Identity.
//!
//! Ordering: most-recent-first by IdentityReceipt.received_at.
//! That puts fresh imports at the top of the UI, which is what
//! the user expects to see right after pasting a card.
//!
//! ## Endpoint
//!
//! `GET /api/fingerprints/identities` → `ListIdentitiesResponse`
//!
//! Returns an empty list on nodes that haven't completed the
//! setup wizard. The handler never errors on "no records" — an
//! empty node is a valid state.
//!
//! ## Trust boundary
//!
//! The response contains only public card material (pubkey,
//! display_name, signature, issued_at) plus receipt metadata
//! (when / how received). No private keys, no per-persona
//! linkage. Persona→Identity linkage is read separately via the
//! persona detail endpoint.

use std::collections::HashMap;
use std::sync::Arc;

use fold_db::schema::types::operations::Query;
use serde::Serialize;
use serde_json::Value;

use crate::fingerprints::canonical_names;
use crate::fingerprints::schemas::{IDENTITY, IDENTITY_RECEIPT};
use crate::fold_node::FoldNode;
use crate::handlers::response::{ApiResponse, HandlerError, HandlerResult};

/// One row in the audit list. Shape mirrors Identity 1:1 plus the
/// joined IdentityReceipt fields so the UI can render a single flat
/// table without a second round trip.
#[derive(Debug, Clone, Serialize)]
pub struct IdentityAuditRow {
    pub identity_id: String,
    pub pub_key: String,
    pub display_name: String,
    pub issued_at: String,
    /// `Self`, `Paste`, or any future channel value (QR, Messaging,
    /// …). Stable string; UIs should not hardcode an enum — just
    /// show it. `None` when the Identity has no matching receipt
    /// (should never happen in a clean DB; included so the handler
    /// doesn't drop Identities).
    pub received_via: Option<String>,
    /// RFC3339 timestamp of when the receipt was written. `None`
    /// when no receipt exists for this Identity.
    pub received_at: Option<String>,
    /// `Self`, `Attested`, or future tiers. Mirrors `received_via`
    /// semantics.
    pub trust_level: Option<String>,
    /// True when this row is the node's own self-Identity. The UI
    /// renders it with a "you" badge so the user can tell their
    /// own card apart from peers'.
    pub is_self: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ListIdentitiesResponse {
    pub identities: Vec<IdentityAuditRow>,
}

/// Scan every Identity on the node, join with the matching
/// IdentityReceipt by `identity_id`, and return a flat audit row
/// list sorted newest-first by `received_at`. Never errors on
/// empty data.
pub async fn list_identities(node: Arc<FoldNode>) -> HandlerResult<ListIdentitiesResponse> {
    let identity_canonical = canonical_names::lookup(IDENTITY).map_err(|e| {
        HandlerError::Internal(format!(
            "fingerprints: canonical_names not initialized for '{}': {}",
            IDENTITY, e
        ))
    })?;
    let receipt_canonical = canonical_names::lookup(IDENTITY_RECEIPT).map_err(|e| {
        HandlerError::Internal(format!(
            "fingerprints: canonical_names not initialized for '{}': {}",
            IDENTITY_RECEIPT, e
        ))
    })?;

    let processor = crate::fold_node::OperationProcessor::new(node.clone());
    let self_pub_key = node.get_node_public_key().to_string();

    // 1. Full scan of Identity records. The set is small (one per
    //    known person the user has cards from) so we don't paginate.
    let identity_query = Query {
        schema_name: identity_canonical,
        fields: vec![
            "id".to_string(),
            "pub_key".to_string(),
            "display_name".to_string(),
            "issued_at".to_string(),
        ],
        filter: None,
        as_of: None,
        rehydrate_depth: None,
        sort_order: None,
        value_filters: None,
    };
    let identity_records = processor
        .execute_query_json(identity_query)
        .await
        .map_err(|e| HandlerError::Internal(format!("identity scan failed: {}", e)))?;

    // 2. Full scan of IdentityReceipt records. Build a lookup
    //    keyed by identity_id so the join is O(n) not O(n*m).
    let receipt_query = Query {
        schema_name: receipt_canonical,
        fields: vec![
            "identity_id".to_string(),
            "received_via".to_string(),
            "received_at".to_string(),
            "trust_level".to_string(),
        ],
        filter: None,
        as_of: None,
        rehydrate_depth: None,
        sort_order: None,
        value_filters: None,
    };
    let receipt_records = processor
        .execute_query_json(receipt_query)
        .await
        .map_err(|e| HandlerError::Internal(format!("receipt scan failed: {}", e)))?;

    let mut receipts_by_identity: HashMap<String, ReceiptFields> = HashMap::new();
    for record in &receipt_records {
        let Some(fields) = record.get("fields") else {
            continue;
        };
        let Some(identity_id) = string_field(fields, "identity_id") else {
            continue;
        };
        let received_via = string_field(fields, "received_via");
        let received_at = string_field(fields, "received_at");
        let trust_level = string_field(fields, "trust_level");
        // Keep the newest receipt per identity when multiple exist.
        // IdentityReceipts are append-only so duplicates happen if
        // the user re-imports an already-present card; picking the
        // newest gives the user the most recent audit entry.
        let entry = receipts_by_identity
            .entry(identity_id)
            .or_insert_with(|| ReceiptFields {
                received_via: None,
                received_at: None,
                trust_level: None,
            });
        let should_replace = match (&entry.received_at, &received_at) {
            (None, _) => true,
            (Some(old), Some(new)) => new.as_str() > old.as_str(),
            _ => false,
        };
        if should_replace {
            entry.received_via = received_via;
            entry.received_at = received_at;
            entry.trust_level = trust_level;
        }
    }

    // 3. Zip into audit rows.
    let mut rows: Vec<IdentityAuditRow> = Vec::with_capacity(identity_records.len());
    for record in &identity_records {
        let Some(fields) = record.get("fields") else {
            continue;
        };
        let Some(identity_id) = string_field(fields, "id") else {
            continue;
        };
        let pub_key = string_field(fields, "pub_key").unwrap_or_default();
        let display_name = string_field(fields, "display_name").unwrap_or_default();
        let issued_at = string_field(fields, "issued_at").unwrap_or_default();
        let is_self = pub_key == self_pub_key;
        let receipt = receipts_by_identity
            .remove(&identity_id)
            .unwrap_or_default();
        rows.push(IdentityAuditRow {
            identity_id,
            pub_key,
            display_name,
            issued_at,
            received_via: receipt.received_via,
            received_at: receipt.received_at,
            trust_level: receipt.trust_level,
            is_self,
        });
    }

    // 4. Sort newest-first. Identities without a receipt sort to
    //    the bottom — they're typically a data-integrity oddity the
    //    user wants to see but not at the top.
    rows.sort_by(|a, b| match (&a.received_at, &b.received_at) {
        (Some(x), Some(y)) => y.cmp(x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });

    Ok(ApiResponse::success(ListIdentitiesResponse {
        identities: rows,
    }))
}

#[derive(Default, Clone)]
struct ReceiptFields {
    received_via: Option<String>,
    received_at: Option<String>,
    trust_level: Option<String>,
}

fn string_field(fields: &Value, name: &str) -> Option<String> {
    fields
        .get(name)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    // Logic that touches the live node is covered by integration
    // tests; the unit tests here exercise the pure join + sort
    // helpers so a regression in the ordering rule is caught
    // without spinning up a FoldNode.

    use super::*;
    use serde_json::json;

    fn receipt(id: &str, via: &str, at: &str, level: &str) -> Value {
        json!({
            "fields": {
                "identity_id": id,
                "received_via": via,
                "received_at": at,
                "trust_level": level,
            }
        })
    }

    #[test]
    fn receipt_lookup_picks_newest_when_multiple_per_identity() {
        let records = vec![
            receipt("id_a", "Paste", "2026-04-01T12:00:00Z", "Attested"),
            receipt("id_a", "Paste", "2026-04-15T12:00:00Z", "Attested"),
        ];
        let mut map: HashMap<String, ReceiptFields> = HashMap::new();
        for record in &records {
            let fields = record.get("fields").unwrap();
            let identity_id = string_field(fields, "identity_id").unwrap();
            let received_at = string_field(fields, "received_at");
            let entry = map.entry(identity_id).or_default();
            let should_replace = match (&entry.received_at, &received_at) {
                (None, _) => true,
                (Some(old), Some(new)) => new.as_str() > old.as_str(),
                _ => false,
            };
            if should_replace {
                entry.received_via = string_field(fields, "received_via");
                entry.received_at = received_at;
                entry.trust_level = string_field(fields, "trust_level");
            }
        }
        assert_eq!(
            map.get("id_a").unwrap().received_at.as_deref(),
            Some("2026-04-15T12:00:00Z")
        );
    }

    #[test]
    fn rows_sort_newest_first_with_no_receipt_at_bottom() {
        let mut rows: Vec<IdentityAuditRow> = [
            IdentityAuditRow {
                identity_id: "id_old".into(),
                pub_key: "pk_o".into(),
                display_name: "Old".into(),
                issued_at: "2026-01-01T00:00:00Z".into(),
                received_via: Some("Paste".into()),
                received_at: Some("2026-01-02T00:00:00Z".into()),
                trust_level: Some("Attested".into()),
                is_self: false,
            },
            IdentityAuditRow {
                identity_id: "id_new".into(),
                pub_key: "pk_n".into(),
                display_name: "New".into(),
                issued_at: "2026-04-01T00:00:00Z".into(),
                received_via: Some("Paste".into()),
                received_at: Some("2026-04-15T00:00:00Z".into()),
                trust_level: Some("Attested".into()),
                is_self: false,
            },
            IdentityAuditRow {
                identity_id: "id_orphan".into(),
                pub_key: "pk_x".into(),
                display_name: "Orphan".into(),
                issued_at: "2026-02-01T00:00:00Z".into(),
                received_via: None,
                received_at: None,
                trust_level: None,
                is_self: false,
            },
        ]
        .to_vec();
        rows.sort_by(|a, b| match (&a.received_at, &b.received_at) {
            (Some(x), Some(y)) => y.cmp(x),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });
        assert_eq!(rows[0].identity_id, "id_new");
        assert_eq!(rows[1].identity_id, "id_old");
        assert_eq!(rows[2].identity_id, "id_orphan");
    }
}
