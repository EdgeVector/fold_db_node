//! Per-schema policy decisions for the generic-ingest fingerprint
//! hook. Today the only decision is which [`SignalBinding`] to use
//! when running the text extractor — the rest of the audit trail
//! (whether to extract at all, which extractor to run, etc.) is
//! handled at higher layers.
//!
//! ## Strong-binding allowlist
//!
//! Most schemas in a node's data are observational: a Note that
//! happens to mention an email is a co-mention, not an assertion of
//! identity. Those default to [`SignalBinding::CoOccurrence`].
//!
//! A small set of schemas are *structurally* identity claims — every
//! record on the schema names a single entity by listing its
//! signals. A Contact card lists `display_name`, `email`, and `phone`
//! for one person. A Calendar attendee record names one attendee.
//! For records on those schemas, intra-record edges should be
//! [`SignalBinding::Strong`] (weight 0.95, kind `StrongMatch`) so the
//! cluster crosses the auto-sweep `MIN_EDGE_WEIGHT` (0.85) floor and
//! produces a tentative Persona on a single record's worth of data.
//!
//! Adding a schema to the allowlist is a deliberate design call.
//! When in doubt: leave it on CoOccurrence. The worst case there is
//! "no auto-Persona for this record," which is recoverable. Wrong
//! Strong tagging produces false merges that the user has to clean
//! up by hand — much harder to undo.
//!
//! Match is case-insensitive on the schema's descriptive name (the
//! human-readable label, not the runtime identity-hash). The hook
//! resolves the descriptive name from the schema manager before
//! calling here.

use crate::fingerprints::extractors::text::SignalBinding;

/// Schemas whose records are structural claims about a single
/// entity. Match is case-insensitive on the descriptive name.
const STRONG_BINDING_SCHEMAS: &[&str] = &[
    "Contacts",
    "AddressBook",
    "AppleContacts",
    "CalendarEvent",
    "AppleCalendarEvent",
    "EmailHeader",
];

/// Decide which [`SignalBinding`] the fingerprint hook should use for
/// records on `descriptive_schema_name`.
pub fn binding_for_schema(descriptive_schema_name: &str) -> SignalBinding {
    if STRONG_BINDING_SCHEMAS
        .iter()
        .any(|s| s.eq_ignore_ascii_case(descriptive_schema_name))
    {
        SignalBinding::Strong
    } else {
        SignalBinding::CoOccurrence
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contacts_get_strong_binding() {
        assert_eq!(binding_for_schema("Contacts"), SignalBinding::Strong);
        assert_eq!(binding_for_schema("AddressBook"), SignalBinding::Strong);
        assert_eq!(binding_for_schema("AppleContacts"), SignalBinding::Strong);
    }

    #[test]
    fn calendar_attendees_get_strong_binding() {
        assert_eq!(binding_for_schema("CalendarEvent"), SignalBinding::Strong);
        assert_eq!(
            binding_for_schema("AppleCalendarEvent"),
            SignalBinding::Strong
        );
    }

    #[test]
    fn email_header_gets_strong_binding() {
        assert_eq!(binding_for_schema("EmailHeader"), SignalBinding::Strong);
    }

    #[test]
    fn observational_schemas_default_to_co_occurrence() {
        assert_eq!(binding_for_schema("Notes"), SignalBinding::CoOccurrence);
        assert_eq!(binding_for_schema("Journal"), SignalBinding::CoOccurrence);
        assert_eq!(binding_for_schema("Recipes"), SignalBinding::CoOccurrence);
        assert_eq!(binding_for_schema("Photos"), SignalBinding::CoOccurrence);
    }

    #[test]
    fn unknown_schemas_default_to_co_occurrence() {
        assert_eq!(
            binding_for_schema("SomeRandomSchema"),
            SignalBinding::CoOccurrence
        );
        assert_eq!(binding_for_schema(""), SignalBinding::CoOccurrence);
    }

    #[test]
    fn match_is_case_insensitive() {
        assert_eq!(binding_for_schema("contacts"), SignalBinding::Strong);
        assert_eq!(binding_for_schema("CONTACTS"), SignalBinding::Strong);
        assert_eq!(binding_for_schema("cOnTaCtS"), SignalBinding::Strong);
    }
}
