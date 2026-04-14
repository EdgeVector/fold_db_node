//! Canonical schema-name lookup for the fingerprints subsystem.
//!
//! ## The naming problem
//!
//! fold_db's schema service canonicalizes every submitted schema by
//! renaming it to its `identity_hash`. A schema proposed with
//! `name = "Fingerprint"` and `descriptive_name = "Fingerprint"` comes
//! back from the service with `name = "<sha256-based hash>"`. The
//! descriptive_name is preserved; the `name` field is replaced.
//!
//! This matters because every subsequent mutation and query call
//! needs the **runtime** name (the canonical hash), not the
//! **semantic** name we proposed. Hard-coding `"Fingerprint"` in a
//! call to `execute_mutation` would fail — no schema exists on the
//! node under that name; only under the canonical hash.
//!
//! ## The invariant this enforces
//!
//! "All schemas must come from the schema service" — per the
//! architectural correction in
//! `exemem-workspace/docs/designs/fingerprints.md`. The fingerprints
//! subsystem never creates a local schema directly. It proposes a
//! schema to the service, loads the canonical version returned, and
//! then uses the canonical name from then on.
//!
//! ## The mapping
//!
//! ```text
//!     descriptive_name                canonical_name (runtime)
//!     ─────────────────────────       ─────────────────────────────
//!     "Fingerprint"         →         "sh_abc…" (some identity hash)
//!     "Mention"             →         "sh_def…"
//!     "Edge"                →         "sh_ghi…"
//!     "Identity"            →         "sh_jkl…"
//!     ...
//! ```
//!
//! The mapping is populated exactly once, at subsystem startup, by
//! `registration::register_phase_1_schemas()`. The rest of the
//! fingerprints code looks up a runtime name via `get()` before any
//! mutation or query.

use once_cell::sync::OnceCell;
use std::collections::HashMap;
use std::sync::RwLock;

use fold_db::error::{FoldDbError, FoldDbResult};

/// Process-wide lookup from descriptive_name → canonical runtime name.
///
/// Populated by `register_phase_1_schemas()` on subsystem startup.
/// Callers must not depend on any specific ordering or lazy
/// initialization — if a caller tries to look up a name before
/// registration has run, it gets a clear error rather than a silent
/// wrong value.
static REGISTRY: OnceCell<RwLock<CanonicalNames>> = OnceCell::new();

#[derive(Debug, Default, Clone)]
pub struct CanonicalNames {
    by_descriptive: HashMap<String, String>,
}

impl CanonicalNames {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `descriptive_name` resolves to `canonical_name`.
    ///
    /// If the descriptive_name was previously mapped to a different
    /// canonical_name, this returns an error. Re-registration of the
    /// same mapping is a no-op (idempotent).
    pub fn insert(&mut self, descriptive_name: &str, canonical_name: &str) -> FoldDbResult<()> {
        if let Some(existing) = self.by_descriptive.get(descriptive_name) {
            if existing == canonical_name {
                return Ok(());
            }
            return Err(FoldDbError::Config(format!(
                "canonical_names: conflicting entries for '{}': existing='{}', new='{}'",
                descriptive_name, existing, canonical_name
            )));
        }
        self.by_descriptive
            .insert(descriptive_name.to_string(), canonical_name.to_string());
        Ok(())
    }

    /// Look up the canonical runtime name for a descriptive name.
    /// Returns an error if the name has not been registered — this
    /// is a loud-failure path so callers cannot silently fall back
    /// to the semantic label as a runtime name.
    pub fn get(&self, descriptive_name: &str) -> FoldDbResult<String> {
        self.by_descriptive
            .get(descriptive_name)
            .cloned()
            .ok_or_else(|| {
                FoldDbError::Config(format!(
                    "canonical_names: no canonical name registered for '{}' — \
                     did register_phase_1_schemas() run at subsystem startup?",
                    descriptive_name
                ))
            })
    }

    pub fn len(&self) -> usize {
        self.by_descriptive.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_descriptive.is_empty()
    }
}

/// Install a populated mapping as the process-wide registry.
///
/// Fails if the registry was already installed with a different mapping.
/// Re-installing an identical mapping is a no-op. Tests can reset the
/// global state via `reset_for_tests()`.
pub fn install(mapping: CanonicalNames) -> FoldDbResult<()> {
    let rw = REGISTRY.get_or_init(|| RwLock::new(CanonicalNames::new()));
    let mut guard = rw
        .write()
        .map_err(|e| FoldDbError::Config(format!("canonical_names: RwLock poisoned: {}", e)))?;
    if guard.is_empty() {
        *guard = mapping;
        return Ok(());
    }
    // Already populated — verify it matches before silently succeeding.
    for (descriptive, canonical) in &mapping.by_descriptive {
        guard.insert(descriptive, canonical)?;
    }
    Ok(())
}

/// Look up a canonical runtime name from the global registry.
///
/// Returns an error if the registry has not been populated or if the
/// descriptive_name is unknown. Both cases are loud-failure.
pub fn lookup(descriptive_name: &str) -> FoldDbResult<String> {
    let rw = REGISTRY.get().ok_or_else(|| {
        FoldDbError::Config(format!(
            "canonical_names: registry not initialized — register_phase_1_schemas() \
             must run at subsystem startup before any lookup. Attempted lookup: '{}'",
            descriptive_name
        ))
    })?;
    let guard = rw
        .read()
        .map_err(|e| FoldDbError::Config(format!("canonical_names: RwLock poisoned: {}", e)))?;
    guard.get(descriptive_name)
}

/// Clear the global registry. Intended for test isolation only —
/// integration tests cannot use `#[cfg(test)]` gating to hide this
/// function because integration tests see the library as an external
/// crate with `cfg(test) == false`. Production code must never call
/// this; there is no scenario in which the subsystem needs to forget
/// canonical names mid-run.
#[doc(hidden)]
pub fn reset_for_tests() {
    if let Some(rw) = REGISTRY.get() {
        if let Ok(mut guard) = rw.write() {
            *guard = CanonicalNames::new();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_get_returns_registered_name() {
        let mut names = CanonicalNames::new();
        names.insert("Fingerprint", "sh_abc123").unwrap();
        assert_eq!(names.get("Fingerprint").unwrap(), "sh_abc123");
    }

    #[test]
    fn get_returns_error_for_unknown_name() {
        let names = CanonicalNames::new();
        let err = names.get("Fingerprint").unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("no canonical name registered"));
    }

    #[test]
    fn insert_is_idempotent_for_same_mapping() {
        let mut names = CanonicalNames::new();
        names.insert("Fingerprint", "sh_abc").unwrap();
        names.insert("Fingerprint", "sh_abc").unwrap();
        assert_eq!(names.get("Fingerprint").unwrap(), "sh_abc");
    }

    #[test]
    fn insert_rejects_conflicting_mapping() {
        let mut names = CanonicalNames::new();
        names.insert("Fingerprint", "sh_abc").unwrap();
        let err = names.insert("Fingerprint", "sh_xyz").unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("conflicting entries"));
    }

    #[test]
    fn len_and_is_empty_track_registered_count() {
        let mut names = CanonicalNames::new();
        assert_eq!(names.len(), 0);
        assert!(names.is_empty());
        names.insert("Fingerprint", "sh_a").unwrap();
        names.insert("Mention", "sh_b").unwrap();
        assert_eq!(names.len(), 2);
        assert!(!names.is_empty());
    }

    #[test]
    fn global_lookup_before_install_returns_error() {
        reset_for_tests();
        let err = lookup("Fingerprint").unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("registry not initialized") || msg.contains("no canonical name"));
    }
}
