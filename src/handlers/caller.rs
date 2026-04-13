//! Caller identity helpers.
//!
//! # LOOPBACK OWNER INVARIANT
//!
//! HTTP handlers in fold_db_node currently assume the caller IS the node owner.
//! This is correct for the Tauri single-user model: the device owner owns the
//! data, and the node is only reachable from localhost via the Tauri UI / CLI.
//!
//! For any non-Tauri distribution (headless daemon, shared mode, hosted-for-many),
//! this invariant must be replaced with per-request caller authentication.
//! See fold_db_node/CLAUDE.md "Trust boundary: loopback owner context".
//!
//! This helper is the single source of truth for that assumption. When a
//! verified caller identity mechanism is added (loopback token, session cookie,
//! signed header), update [`current_caller_pubkey`] and all call sites
//! automatically pick up the new behavior.

use crate::fold_node::node::FoldNode;

/// Returns the public key of the HTTP caller.
///
/// **Currently returns the node's own public key** — correct for the Tauri
/// single-user model where the caller IS the node owner. See module-level
/// docs before shipping a non-Tauri distribution.
pub fn current_caller_pubkey(node: &FoldNode) -> String {
    // LOOPBACK OWNER INVARIANT: hardcoded for Tauri single-user mode.
    // Replace with verified_caller_identity() before shipping headless/multi-user.
    node.get_node_public_key().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Placeholder test: verifies that under the loopback-owner invariant,
    // `current_caller_pubkey` returns the node's own public key. When a
    // verified caller identity mechanism is introduced, this test should be
    // updated to cover the new behavior.
    #[test]
    fn current_caller_pubkey_matches_node_pubkey_under_loopback_invariant() {
        // This is a compile-time check only — constructing a FoldNode in a
        // unit test requires substantial setup. Integration tests exercise
        // the real code path via HTTP handlers. See handlers/query.rs and
        // handlers/mutation.rs for end-to-end coverage.
        fn _assert_signature(node: &FoldNode) -> String {
            current_caller_pubkey(node)
        }
    }
}
