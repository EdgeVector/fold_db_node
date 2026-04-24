//! Architectural fence: Persona and Identity sharing must NEVER reach
//! the discovery transport.
//!
//! This test is the structural defense for TODO-3 in
//! `exemem-workspace/TODOS.md`. The Privacy Principle in
//! `docs/designs/fingerprints.md` says Personas and Identities never
//! publish to discovery — direct peer sharing and Identity Card
//! exchange go through the existing E2E messaging layer. Without an
//! enforced grep, a future refactor could silently reach for a
//! discovery primitive ("it has presigned URLs, let me just use it")
//! and quietly violate the invariant.
//!
//! The test walks every `.rs` file under the Phase 3 sharing
//! codepath and asserts none of them import from
//! `crate::handlers::discovery` / `crate::server::routes::discovery`
//! or hardcode an `/api/discovery/` URL.
//!
//! If this test starts failing, the right move is almost never to
//! loosen it. Route the new capability through the E2E messaging
//! layer instead, or — if a discovery primitive is genuinely
//! dual-use — refactor it out of `handlers::discovery` so the import
//! is honest about what it's doing.

use std::fs;
use std::path::{Path, PathBuf};

/// Crate-relative directories that form the Phase 3 sharing /
/// exchange codepath. Every Rust source file under these roots is
/// audited.
const FENCED_ROOTS: &[&str] = &[
    "src/fingerprints",
    "src/handlers/fingerprints",
    "src/server/routes/fingerprints",
];

/// Import paths and URL fragments that must not appear in fenced
/// code. Paired with a human-readable label for the failure report.
const FORBIDDEN_PATTERNS: &[(&str, &str)] = &[
    (
        "use crate::handlers::discovery",
        "import from crate::handlers::discovery::*",
    ),
    (
        "use crate::server::routes::discovery",
        "import from crate::server::routes::discovery::*",
    ),
    ("/api/discovery/", "hardcoded /api/discovery/* URL"),
];

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn rust_files_under(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    walk(root, &mut files);
    files.sort();
    files
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

#[test]
fn fingerprint_sharing_codepath_never_imports_discovery() {
    let root = crate_root();
    let mut violations: Vec<String> = Vec::new();

    for fenced in FENCED_ROOTS {
        let fenced_path = root.join(fenced);
        assert!(
            fenced_path.exists(),
            "fenced root {} not found — did the module move? Update FENCED_ROOTS.",
            fenced_path.display()
        );

        for file in rust_files_under(&fenced_path) {
            let rel = file
                .strip_prefix(&root)
                .expect("file is under crate root")
                .to_string_lossy()
                .replace('\\', "/");

            let contents = fs::read_to_string(&file)
                .unwrap_or_else(|e| panic!("read {}: {e}", file.display()));

            for (pattern, label) in FORBIDDEN_PATTERNS {
                for (line_idx, line) in contents.lines().enumerate() {
                    // Skip line comments — the fence is about runtime
                    // behavior, not the words we use to document it.
                    // `//` catches `//` and `//!`; no-op on code that
                    // has a trailing `//` comment, which is fine
                    // because we still scan the code part.
                    if line.trim_start().starts_with("//") {
                        continue;
                    }
                    if line.contains(pattern) {
                        violations.push(format!(
                            "{rel}:{lineno}: {label} — `{line}`",
                            lineno = line_idx + 1,
                            line = line.trim(),
                        ));
                    }
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "Persona / Identity sharing codepath reaches discovery — \
         Privacy Principle violated. Offending lines:\n  {}\n\n\
         The fix is almost never to widen the exemption. Route the \
         capability through the E2E messaging layer instead. See \
         TODO-3 in exemem-workspace/TODOS.md and the Privacy \
         Principle in docs/designs/fingerprints.md.",
        violations.join("\n  "),
    );
}

#[test]
fn fingerprints_module_documents_the_fence() {
    // The fence is structural, but if the explanatory doc comment at
    // the top of `src/fingerprints/mod.rs` ever goes missing, future
    // contributors have no in-code pointer to what this test is
    // protecting or why. Guard the discoverability of the convention.
    let root = crate_root();
    let contents = fs::read_to_string(root.join("src/fingerprints/mod.rs"))
        .expect("src/fingerprints/mod.rs must exist");
    assert!(
        contents.contains("identity_sharing_fence_test"),
        "src/fingerprints/mod.rs must reference identity_sharing_fence_test.rs \
         so future readers can find the structural rule it documents."
    );
    assert!(
        contents.contains("Privacy principle") || contents.contains("privacy principle"),
        "src/fingerprints/mod.rs must document the Privacy Principle so the \
         fence test's failure message has an in-repo narrative to point at."
    );
}
