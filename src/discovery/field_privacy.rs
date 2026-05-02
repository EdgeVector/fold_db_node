//! Per-field user consent for discovery publishing.
//!
//! This is NOT anonymity gating — it's user consent, expressed at the
//! level of individual schema fields. A user can declare that their
//! `email` or `ssn` field should never be published to the discovery
//! index even when the schema as a whole has opt-in enabled. See
//! `preferences/no-discovery-anonymity-gating` in gbrain for the
//! distinction between gating (removed) and consent (preserved).
//!
//! ## History
//!
//! This module lived in `fold_db::db_operations::native_index::anonymity`
//! until 2026-04-18, when fold_db PR #555 deleted the whole module as
//! part of the anonymity-gate rip-out. That deletion was overscoped —
//! it removed the user-consent `FieldPrivacyClass::NeverPublish`
//! capability along with the actual gate helpers (NER, entropy,
//! `PublishIfAnonymous`).
//!
//! The consent half is restored here, at the app layer where it
//! belongs. fold_db is the database; per-field consent for discovery
//! is a fold_db_node concern.
//!
//! ## What's different from the pre-rip-out enum
//!
//! The original had three variants: `NeverPublish`, `PublishIfAnonymous`,
//! `AlwaysPublish`. `PublishIfAnonymous` was the gate — "publish if the
//! fragment content passes NER + entropy." With the gate removed, there
//! is no distinction between "publish if anonymous" and "always
//! publish"; both collapse to `AlwaysPublish`. The two-variant enum
//! here reflects that.

use serde::{Deserialize, Serialize};

/// Per-field consent decision for discovery publishing.
#[cfg_attr(feature = "ts-bindings", derive(ts_rs::TS))]
#[cfg_attr(
    feature = "ts-bindings",
    ts(export, export_to = "src/fold_node/static-react/src/types/")
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldPrivacyClass {
    /// Fragments from this field are never published. User-level opt-out.
    NeverPublish,
    /// Fragments from this field are published when the schema's
    /// `discover_enabled` / `publish_faces` toggle is on. Default for
    /// fields that don't match the PII heuristic.
    AlwaysPublish,
}

/// Infer a default privacy class from a field name.
///
/// Fields whose names suggest PII default to `NeverPublish` so a user
/// who turns a schema's `discover_enabled` toggle on doesn't
/// accidentally publish their email / SSN / address / etc. The lists
/// here are conservative — when in doubt, mark as PII. Users can
/// override per-field via the `field_privacy` map on
/// [`crate::discovery::config::DiscoveryConfig`].
///
/// Not covered by this heuristic: schema-specific sensitive fields
/// that aren't on the common-noun list. Users should set those
/// explicitly.
pub fn default_privacy_class(field_name: &str) -> FieldPrivacyClass {
    let lower = field_name.to_lowercase();

    // NeverPublish: fields that inherently contain PII. Preserved
    // verbatim from the pre-rip-out fold_db module so user-visible
    // behavior doesn't change on fields like `email`, `ssn`, etc.
    const NEVER_PUBLISH: &[&str] = &[
        "name",
        "first_name",
        "last_name",
        "full_name",
        "email",
        "phone",
        "telephone",
        "mobile",
        "ssn",
        "social_security",
        "address",
        "street",
        "zip",
        "zipcode",
        "zip_code",
        "postal_code",
        "city",
        "state",
        "country",
        "dob",
        "date_of_birth",
        "birthday",
        "passport",
        "driver_license",
        "license_number",
        "credit_card",
        "card_number",
        "account_number",
        "ip_address",
        "mac_address",
        "username",
        "user_name",
        "password",
        "secret",
    ];

    for &pattern in NEVER_PUBLISH {
        if lower == pattern || lower.contains(pattern) {
            return FieldPrivacyClass::NeverPublish;
        }
    }

    FieldPrivacyClass::AlwaysPublish
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_defaults_to_never_publish() {
        assert_eq!(
            default_privacy_class("email"),
            FieldPrivacyClass::NeverPublish
        );
    }

    #[test]
    fn user_email_defaults_to_never_publish_via_substring() {
        // The substring check on "email" catches compound field names.
        assert_eq!(
            default_privacy_class("user_email"),
            FieldPrivacyClass::NeverPublish
        );
        assert_eq!(
            default_privacy_class("workEmail"),
            FieldPrivacyClass::NeverPublish
        );
    }

    #[test]
    fn ssn_and_password_default_to_never_publish() {
        assert_eq!(
            default_privacy_class("ssn"),
            FieldPrivacyClass::NeverPublish
        );
        assert_eq!(
            default_privacy_class("password"),
            FieldPrivacyClass::NeverPublish
        );
    }

    #[test]
    fn non_pii_field_defaults_to_always_publish() {
        // With the anonymity gate gone, anything not on the PII list
        // publishes by default — there's no "check if anonymous" middle
        // ground anymore.
        assert_eq!(
            default_privacy_class("title"),
            FieldPrivacyClass::AlwaysPublish
        );
        assert_eq!(
            default_privacy_class("body"),
            FieldPrivacyClass::AlwaysPublish
        );
        assert_eq!(
            default_privacy_class("tags"),
            FieldPrivacyClass::AlwaysPublish
        );
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(
            default_privacy_class("Email"),
            FieldPrivacyClass::NeverPublish
        );
        assert_eq!(
            default_privacy_class("SSN"),
            FieldPrivacyClass::NeverPublish
        );
    }
}
