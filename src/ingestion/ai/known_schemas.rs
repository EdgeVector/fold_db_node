//! Curated list of "anchor" schemas the AI proposer should match against
//! by their exact descriptive_name when the input data is a reasonable fit.
//!
//! ## Why this list exists
//!
//! The schema service has cosine-similarity matching as a fallback when an
//! AI proposal doesn't exactly match a registered descriptive_name (see
//! `state_matching::find_matching_descriptive_name`). In practice that
//! fallback is unreliable for the load-bearing case — `Contact Records`
//! ↔ `Contacts` doesn't always cross the 0.8 threshold even though the
//! pair is semantically obvious to a person. (Empirically: `Calendar` →
//! `CalendarEvent` works, but `Contact` / `ContactList` / `AddressBook`
//! / `PersonalContacts` all fail to match `Contacts`.)
//!
//! Inlining the list of canonical descriptive_names directly into the
//! prompt routes around the cosine reliability problem entirely: when
//! the AI sees `Contacts` in "schemas you should reuse if applicable,"
//! it picks that exact name and the schema service's exact-match path
//! is hit on the first try.
//!
//! ## Drift policy
//!
//! This list mirrors the persona seeds shipped by the schema service in
//! [`crates/core/data/persona_schemas.json`]. When new anchors are added
//! to that seed file, add them here too — the test in this module pins
//! the descriptive_names to the persona-binding allowlist in
//! [`crate::fingerprints::schema_policy`] so the two sets stay in step.
//!
//! Long-term the right shape is to fetch this list from the schema
//! service at IngestionService construction (cached, refreshed on
//! demand). Hardcoding for now keeps the change small and the prompt
//! deterministic across nodes.

/// One anchor schema for the AI to consider matching.
#[derive(Debug, Clone, Copy)]
pub struct KnownSchema {
    /// The descriptive_name registered with the schema service. The AI
    /// should propose this exact string verbatim when the data fits.
    pub descriptive_name: &'static str,
    /// One-line summary of what this schema is for. Helps the AI decide
    /// whether the input data fits.
    pub summary: &'static str,
    /// Field names on the schema. Used both for the prompt summary and
    /// to give the AI a concrete signal of "data shape" — e.g. an input
    /// with `{name, email, phone}` should match `Contacts`.
    pub fields: &'static [&'static str],
}

/// Anchor schemas the AI proposer should prefer when input data fits.
/// Mirrors the seeds in
/// `schema_service/crates/core/data/persona_schemas.json`.
pub const KNOWN_SCHEMAS: &[KnownSchema] = &[
    KnownSchema {
        descriptive_name: "Contacts",
        summary:
            "A single contact card — one person or organization with their direct identifiers.",
        fields: &[
            "name",
            "email",
            "phone",
            "address",
            "birthday",
            "relationship",
            "notes",
        ],
    },
    KnownSchema {
        descriptive_name: "CalendarEvent",
        summary: "A calendar event or meeting with attendees.",
        fields: &[
            "title",
            "start_at",
            "end_at",
            "attendees",
            "location",
            "description",
        ],
    },
    KnownSchema {
        descriptive_name: "EmailHeader",
        summary: "Metadata for a single email — sender, recipients, subject, timestamps.",
        fields: &[
            "message_id",
            "from",
            "to",
            "cc",
            "subject",
            "sent_at",
            "body_preview",
        ],
    },
];

/// Render the known-schemas block for inclusion in an AI prompt.
/// Returns an empty string when the list is empty so the caller can
/// concatenate unconditionally.
pub fn render_for_prompt() -> String {
    if KNOWN_SCHEMAS.is_empty() {
        return String::new();
    }

    let mut out = String::from(
        "\n\nEXISTING SCHEMAS — match against these by exact descriptive_name when the data fits:\n",
    );
    for s in KNOWN_SCHEMAS {
        let fields = s.fields.join(", ");
        out.push_str(&format!(
            "  - {name}: {summary} (fields: {fields})\n",
            name = s.descriptive_name,
            summary = s.summary,
            fields = fields,
        ));
    }
    out.push_str(
        "\nIf the input data is a reasonable fit for one of the schemas above, you MUST use that EXACT descriptive_name (case-sensitive). Only propose a NEW descriptive_name when none of the above is a fit.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_schemas_are_non_empty_and_unique() {
        assert!(!KNOWN_SCHEMAS.is_empty());
        let names: Vec<_> = KNOWN_SCHEMAS.iter().map(|s| s.descriptive_name).collect();
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            names.len(),
            "KNOWN_SCHEMAS contains duplicate descriptive_names"
        );
    }

    #[test]
    fn every_known_schema_has_at_least_one_field() {
        for s in KNOWN_SCHEMAS {
            assert!(
                !s.fields.is_empty(),
                "schema '{}' must declare at least one field",
                s.descriptive_name
            );
        }
    }

    #[test]
    fn render_includes_every_descriptive_name() {
        let rendered = render_for_prompt();
        for s in KNOWN_SCHEMAS {
            assert!(
                rendered.contains(s.descriptive_name),
                "rendered prompt block missing '{}'",
                s.descriptive_name
            );
        }
        // The exact-match instruction is what makes this useful — pin it.
        assert!(rendered.contains("EXACT descriptive_name"));
    }

    #[test]
    fn descriptive_names_match_persona_binding_allowlist() {
        // The Strong-binding policy in `schema_policy::binding_for_schema`
        // only fires for descriptive_names in its hardcoded allowlist.
        // Every KNOWN_SCHEMAS entry should also be in that allowlist —
        // otherwise this prompt anchor wouldn't trigger Strong binding
        // at hook time, defeating the point.
        use crate::fingerprints::extractors::text::SignalBinding;
        use crate::fingerprints::schema_policy::binding_for_schema;
        for s in KNOWN_SCHEMAS {
            assert_eq!(
                binding_for_schema(s.descriptive_name),
                SignalBinding::Strong,
                "KNOWN_SCHEMAS entry '{}' is not in the Strong-binding allowlist — \
                 anchoring the AI to it would skip the persona pipeline",
                s.descriptive_name
            );
        }
    }
}
