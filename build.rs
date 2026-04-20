//! Stamp `FOLDDB_BUILD_VERSION` into every binary at compile time.
//!
//! Resolution order:
//! 1. `$GITHUB_REF_NAME` when it looks like a release tag (`v<semver...>`).
//!    This is how the release workflow pins binaries to the pushed tag —
//!    without this, clap's `version` reads `CARGO_PKG_VERSION` and the
//!    binary reports the stale manifest version regardless of the tag.
//! 2. `git describe --tags --always --dirty` so local dev builds reflect
//!    real git state (e.g. `v0.3.1-5-ge1f2a` or `e1f2a-dirty`).
//! 3. `CARGO_PKG_VERSION` fallback when neither is available (e.g. source
//!    tarball builds without git metadata).
//!
//! Keep this small and panic-free — build scripts run on every compile.
use std::process::Command;

fn main() {
    // Re-run when GITHUB_REF_NAME changes (release builds are driven by the
    // tag) or when the build script itself is edited. We intentionally do
    // NOT track `.git/HEAD` / `.git/refs/tags` because this package is a
    // git-submodule worktree: `.git` is a *file* pointing at the real
    // gitdir, so the usual trick silently no-ops. Dev builds with a stale
    // git-describe stamp are a mild annoyance; `cargo clean` refreshes.
    println!("cargo:rerun-if-env-changed=GITHUB_REF_NAME");
    println!("cargo:rerun-if-env-changed=FOLDDB_BUILD_VERSION_OVERRIDE");
    println!("cargo:rerun-if-changed=build.rs");

    let version = resolve_version();
    println!("cargo:rustc-env=FOLDDB_BUILD_VERSION={version}");
}

fn resolve_version() -> String {
    if let Ok(override_val) = std::env::var("FOLDDB_BUILD_VERSION_OVERRIDE") {
        let trimmed = override_val.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    if let Ok(ref_name) = std::env::var("GITHUB_REF_NAME") {
        if let Some(stripped) = strip_tag_prefix(&ref_name) {
            return stripped;
        }
    }

    if let Some(described) = git_describe() {
        return described;
    }

    env!("CARGO_PKG_VERSION").to_string()
}

/// Strip the leading `v` from `v0.3.1`-style tags. Returns `None` for refs
/// that do not look like semver-ish release tags (e.g. branch names in
/// GitHub Actions branch-push workflows), so we fall through to git describe.
fn strip_tag_prefix(ref_name: &str) -> Option<String> {
    let trimmed = ref_name.trim();
    let rest = trimmed.strip_prefix('v')?;
    let first = rest.chars().next()?;
    if first.is_ascii_digit() {
        Some(rest.to_string())
    } else {
        None
    }
}

fn git_describe() -> Option<String> {
    let output = Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8(output.stdout).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Keep `v` prefix stripped for consistency with the tag branch.
    Some(strip_tag_prefix(trimmed).unwrap_or_else(|| trimmed.to_string()))
}
