//! Pure key derivation for the fingerprint substrate.
//!
//! These functions compute stable, content-derived primary keys for
//! the Fingerprint, Edge, Mention, and Identity schemas. They have no
//! I/O side effects and are cheap to test.
//!
//! ## Key conventions
//!
//! ```text
//!   Fingerprint     fp_<sha256("Fingerprint" | kind | canonical_value)>
//!   Edge            eg_<sha256("Edge"        | a | b | kind)>
//!                       where (a, b) is canonicalized to ascending
//!                       order so (a=X, b=Y) and (a=Y, b=X) produce
//!                       the same key
//!   Identity        id_<pub_key>
//!                       pubkey is already globally unique, so it is
//!                       used directly (prefixed for schema-namespace
//!                       readability)
//!   Mention         mn_<uuid>              ← per-instance, not content
//!   IdentityReceipt ir_<uuid>              ← per-instance, not content
//!   Persona         ps_<uuid>              ← per-instance, not content
//! ```
//!
//! Content-keyed schemas (Fingerprint, Edge, Identity) dedupe via the
//! schema layer's upsert semantics when two ingests produce the same
//! derived key. Per-instance keys (Mention, IdentityReceipt, Persona)
//! do not dedupe and are always fresh.
//!
//! ## Canonicalization rules
//!
//! * **Email** — lowercased, trimmed. No Gmail-dot-stripping (that
//!   would silently collapse distinct addresses like `alice.b@` and
//!   `aliceb@`).
//! * **Phone** — E.164 format assumed upstream; this module does not
//!   attempt to canonicalize internally because country code
//!   handling requires libphonenumber-grade logic.
//! * **Full name** — lowercased, whitespace-collapsed. No
//!   transliteration or diacritic folding.
//! * **Face embedding** — bytes of the raw `Vec<f32>`. ArcFace outputs
//!   are deterministic on a given platform, so the bytes are stable
//!   for the same image. Two photos of the same person produce
//!   different embeddings → different fingerprint_ids → the graph
//!   connects them via StrongMatch edges instead of merging them.
//!
//! The canonicalization rules here are intentionally thin. The
//! extractor layer is responsible for upstream canonicalization
//! (e.g. Apple Contacts phone-number parsing). These helpers only
//! produce the final hash from an already-canonical value.

use sha2::{Digest, Sha256};

/// Fingerprint kinds. Kept here as `&'static str` constants so every
/// extractor names the same thing and typos fail to compile.
pub mod kind {
    pub const EMAIL: &str = "email";
    pub const PHONE: &str = "phone";
    pub const FACE_EMBEDDING: &str = "face_embedding";
    pub const FULL_NAME: &str = "full_name";
    pub const FIRST_NAME: &str = "first_name";
    pub const HANDLE: &str = "handle";
    pub const NODE_PUB_KEY: &str = "node_pub_key";
}

/// Edge kinds. Same rationale as fingerprint kind constants.
pub mod edge_kind {
    pub const STRONG_MATCH: &str = "StrongMatch";
    /// Between `MIN_SIMILARITY_EDGE` and `STRONG_MATCH_CUTOFF` in
    /// the face ingest path. Represents "plausibly the same face,
    /// but close enough to be a sibling" — clusters at the default
    /// threshold but splits at a tight threshold.
    pub const MEDIUM_MATCH: &str = "MediumMatch";
    pub const CO_OCCURRENCE: &str = "CoOccurrence";
    pub const USER_ASSERTED: &str = "UserAsserted";
    pub const TEMPORAL_COINCIDENCE: &str = "TemporalCoincidence";
    pub const USER_FORBIDDEN: &str = "UserForbidden";
}

/// Compute a Fingerprint primary key from (kind, canonical_value_bytes).
///
/// The caller is responsible for producing the canonical byte
/// representation. For strings, use `kind_and_string_fingerprint_id`.
/// For face embeddings, pass the raw `Vec<f32>` as bytes.
pub fn fingerprint_id_from_bytes(kind: &str, canonical_bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"Fingerprint");
    hasher.update(kind.as_bytes());
    hasher.update(canonical_bytes);
    format!("fp_{:x}", hasher.finalize())
}

/// Compute a Fingerprint primary key from a string value with explicit
/// canonicalization. Used for email, phone, name, handle.
pub fn fingerprint_id_for_string(kind: &str, value: &str) -> String {
    let canonical = canonicalize_string(kind, value);
    fingerprint_id_from_bytes(kind, canonical.as_bytes())
}

/// Compute a Fingerprint primary key from a 512-float face embedding.
///
/// Uses the raw IEEE-754 bytes so the key is stable on the same
/// platform for the same image. Cross-platform reproducibility is
/// an explicit non-goal — if two devices on different hardware
/// produce slightly different embeddings for the same image, they
/// become two fingerprints connected by a high-similarity edge
/// rather than a single canonical fingerprint. That is the correct
/// behavior: the graph captures the fact that the two observations
/// came from different devices.
pub fn fingerprint_id_for_face_embedding(embedding: &[f32]) -> String {
    // SAFETY: &[f32] → &[u8] is sound for hashing purposes; we only
    // read the bytes, never interpret them as f32 after the cast.
    let byte_len = std::mem::size_of_val(embedding);
    let bytes = unsafe { std::slice::from_raw_parts(embedding.as_ptr() as *const u8, byte_len) };
    fingerprint_id_from_bytes(kind::FACE_EMBEDDING, bytes)
}

/// Compute an Edge primary key from (a, b, kind). Canonicalizes the
/// endpoint order so (a=X, b=Y) and (a=Y, b=X) produce the same key.
pub fn edge_id(a: &str, b: &str, kind: &str) -> String {
    let (first, second) = if a <= b { (a, b) } else { (b, a) };
    let mut hasher = Sha256::new();
    hasher.update(b"Edge");
    hasher.update(first.as_bytes());
    hasher.update(second.as_bytes());
    hasher.update(kind.as_bytes());
    format!("eg_{:x}", hasher.finalize())
}

/// Compute an Identity primary key from a public key. The pubkey is
/// already globally unique, so the key is just a prefixed version of
/// the pubkey string.
pub fn identity_id(pub_key: &str) -> String {
    format!("id_{}", pub_key)
}

/// Compute the `source_composite` hash key used by the
/// `MentionBySource` junction.
pub fn mention_source_composite(source_schema: &str, source_key: &str) -> String {
    format!("{}:{}", source_schema, source_key)
}

fn canonicalize_string(kind: &str, value: &str) -> String {
    match kind {
        kind::EMAIL => value.trim().to_ascii_lowercase(),
        kind::FULL_NAME | kind::FIRST_NAME => value
            .trim()
            .to_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
        kind::PHONE => value.trim().to_string(),
        kind::HANDLE => value.trim().trim_start_matches('@').to_ascii_lowercase(),
        _ => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── fingerprint_id for strings ─────────────────────────────

    #[test]
    fn email_fingerprint_id_is_deterministic() {
        let a = fingerprint_id_for_string(kind::EMAIL, "tom@acme.com");
        let b = fingerprint_id_for_string(kind::EMAIL, "tom@acme.com");
        assert_eq!(a, b);
        assert!(a.starts_with("fp_"));
    }

    #[test]
    fn email_fingerprint_id_is_case_insensitive() {
        let a = fingerprint_id_for_string(kind::EMAIL, "tom@acme.com");
        let b = fingerprint_id_for_string(kind::EMAIL, "Tom@Acme.COM");
        assert_eq!(a, b);
    }

    #[test]
    fn email_fingerprint_id_trims_whitespace() {
        let a = fingerprint_id_for_string(kind::EMAIL, "tom@acme.com");
        let b = fingerprint_id_for_string(kind::EMAIL, "  tom@acme.com  ");
        assert_eq!(a, b);
    }

    #[test]
    fn different_kinds_produce_different_ids_for_same_string() {
        let email_id = fingerprint_id_for_string(kind::EMAIL, "tom");
        let name_id = fingerprint_id_for_string(kind::FIRST_NAME, "tom");
        assert_ne!(email_id, name_id);
    }

    #[test]
    fn full_name_collapses_whitespace() {
        let a = fingerprint_id_for_string(kind::FULL_NAME, "Tom Tang");
        let b = fingerprint_id_for_string(kind::FULL_NAME, "  tom    tang  ");
        assert_eq!(a, b);
    }

    #[test]
    fn handle_strips_at_prefix() {
        let a = fingerprint_id_for_string(kind::HANDLE, "tomtang");
        let b = fingerprint_id_for_string(kind::HANDLE, "@tomtang");
        assert_eq!(a, b);
    }

    // ── fingerprint_id for face embeddings ─────────────────────

    #[test]
    fn face_fingerprint_id_is_deterministic() {
        let embedding = vec![0.1_f32; 512];
        let a = fingerprint_id_for_face_embedding(&embedding);
        let b = fingerprint_id_for_face_embedding(&embedding);
        assert_eq!(a, b);
        assert!(a.starts_with("fp_"));
    }

    #[test]
    fn different_face_embeddings_produce_different_ids() {
        let a = fingerprint_id_for_face_embedding(&vec![0.1_f32; 512]);
        let b = fingerprint_id_for_face_embedding(&vec![0.2_f32; 512]);
        assert_ne!(a, b);
    }

    #[test]
    fn face_fingerprint_id_independent_of_embedding_length() {
        // Even if somehow we had a shorter embedding, the id would still
        // compute. It just wouldn't be useful.
        let a = fingerprint_id_for_face_embedding(&[0.1, 0.2, 0.3]);
        let b = fingerprint_id_for_face_embedding(&[0.1, 0.2, 0.3]);
        assert_eq!(a, b);
    }

    // ── edge_id canonicalization ───────────────────────────────

    #[test]
    fn edge_id_is_order_independent() {
        let ab = edge_id("fp_A", "fp_B", edge_kind::STRONG_MATCH);
        let ba = edge_id("fp_B", "fp_A", edge_kind::STRONG_MATCH);
        assert_eq!(ab, ba);
    }

    #[test]
    fn edge_id_depends_on_kind() {
        let strong = edge_id("fp_A", "fp_B", edge_kind::STRONG_MATCH);
        let coocc = edge_id("fp_A", "fp_B", edge_kind::CO_OCCURRENCE);
        assert_ne!(strong, coocc);
    }

    #[test]
    fn edge_id_starts_with_prefix() {
        let id = edge_id("fp_A", "fp_B", edge_kind::STRONG_MATCH);
        assert!(id.starts_with("eg_"));
    }

    #[test]
    fn edge_id_different_endpoints_produce_different_ids() {
        let ab = edge_id("fp_A", "fp_B", edge_kind::STRONG_MATCH);
        let ac = edge_id("fp_A", "fp_C", edge_kind::STRONG_MATCH);
        assert_ne!(ab, ac);
    }

    // ── identity_id ─────────────────────────────────────────────

    #[test]
    fn identity_id_prefixes_pubkey() {
        let pk = "ed25519:abcdef";
        let id = identity_id(pk);
        assert_eq!(id, "id_ed25519:abcdef");
    }

    // ── mention_source_composite ───────────────────────────────

    #[test]
    fn source_composite_format_matches_junction_schema() {
        let c = mention_source_composite("Photos", "IMG_1234");
        assert_eq!(c, "Photos:IMG_1234");
    }
}
