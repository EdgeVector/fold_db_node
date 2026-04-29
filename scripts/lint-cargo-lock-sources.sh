#!/usr/bin/env bash
# lint-cargo-lock-sources.sh
#
# Catch the "missing-source trap" in Cargo.lock: when a `[[package]]` entry
# for a git-sourced dep has lost its `source = "git+..."` line. This happens
# when a developer commits Cargo.lock with `.cargo/config.toml` (the local
# sibling-path patch) active — every cargo invocation rewrites Cargo.lock
# to drop those source lines, and once such a commit lands on main,
# `cargo update -p <pkg>` errors with
#   `package ID specification "<pkg>" did not match any packages`
# from then on, because cargo uses (name, version, source) as the package
# identity. The committed PreToolUse `cargo-lock-guard.sh` hook tries to
# block these commits, but only catches the *purely-noise* case — a commit
# that combines a real bump with the noise still gets through, and once
# landed, the only repair is surgical re-injection of the source line.
#
# This lint runs in CI and on demand. It walks each `[[package]]` block in
# Cargo.lock whose name matches a git-sourced dep declared in our Cargo.toml
# and asserts the block has a `source = "git+..."` line. If any are missing,
# it fails with a clear pointer to the recovery procedure in CLAUDE.md.
#
# Scope: walks both `[dependencies]` and `[dev-dependencies]` for any entry
# of the form `<name> = { ... git = "..." ... }`. Plain registry deps and
# path deps are ignored. Transitive git deps that aren't named directly in
# Cargo.toml are not checked — empirically the missing-source trap only
# affects the deps we patch in `.cargo/config.toml`, which by construction
# are the ones declared at the top level here.
#
# Usage:        bash scripts/lint-cargo-lock-sources.sh
# Exit code:    0 if every git dep's lock entry has a source line, 1 otherwise.

set -euo pipefail

SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
REPO_ROOT="$( cd "$SCRIPT_DIR/.." && pwd )"
cd "$REPO_ROOT"

if [[ ! -f Cargo.toml ]]; then
    echo "lint-cargo-lock-sources: no Cargo.toml at $REPO_ROOT" >&2
    exit 1
fi
if [[ ! -f Cargo.lock ]]; then
    echo "lint-cargo-lock-sources: no Cargo.lock at $REPO_ROOT" >&2
    exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
    echo "lint-cargo-lock-sources: python3 not found in PATH" >&2
    exit 1
fi

python3 - <<'PY'
import re
import sys
from pathlib import Path

cargo_toml = Path("Cargo.toml").read_text()
cargo_lock = Path("Cargo.lock").read_text()

# Pull every dep entry of the form `<name> = { ... git = "..." ... }`
# from any `[dependencies]` / `[dev-dependencies]` / target-specific deps
# table. Cargo.toml in this repo keeps each such dep on a single line, but
# we tolerate either inline or multi-line tables by matching a single
# logical entry: `<name>\s*=\s*\{[^}]*\}`.
git_dep_pattern = re.compile(
    r"^([A-Za-z0-9_\-]+)\s*=\s*\{[^}]*\bgit\s*=\s*\"[^\"]+\"[^}]*\}",
    re.MULTILINE,
)
git_dep_names = set()
for m in git_dep_pattern.finditer(cargo_toml):
    git_dep_names.add(m.group(1).replace("-", "_"))

# Cargo.lock package names use underscores; Cargo.toml dep keys may use either.
# We normalised above. If we found nothing, the lint is a no-op (nothing to
# enforce) — but emit a note so a future refactor that drops all git deps
# doesn't silently turn this into dead code.
if not git_dep_names:
    print("lint-cargo-lock-sources: no git-sourced deps declared in Cargo.toml — nothing to check.")
    sys.exit(0)

# Walk Cargo.lock package blocks. A block starts at `[[package]]` and runs
# until the next `[[package]]` or end-of-file.
blocks = re.split(r"(?m)^\[\[package\]\]\s*$", cargo_lock)
# blocks[0] is the preamble before the first `[[package]]`; skip it.
offenders = []
seen = set()
for block in blocks[1:]:
    name_match = re.search(r'^\s*name\s*=\s*"([^"]+)"', block, re.MULTILINE)
    if not name_match:
        continue
    name = name_match.group(1)
    if name not in git_dep_names:
        continue
    seen.add(name)
    has_git_source = re.search(r'^\s*source\s*=\s*"git\+', block, re.MULTILINE)
    if not has_git_source:
        offenders.append(name)

# Anything declared in Cargo.toml as a git dep but absent from Cargo.lock
# entirely is also a defect — usually means Cargo.lock is stale.
missing = sorted(git_dep_names - seen)

if offenders or missing:
    print("lint-cargo-lock-sources: FAIL", file=sys.stderr)
    if offenders:
        print("", file=sys.stderr)
        print("The following Cargo.lock [[package]] entries are missing their", file=sys.stderr)
        print('`source = "git+..."` line, but Cargo.toml declares them as git deps:', file=sys.stderr)
        for name in sorted(set(offenders)):
            print(f"  - {name}", file=sys.stderr)
    if missing:
        print("", file=sys.stderr)
        print("The following git deps are declared in Cargo.toml but have no", file=sys.stderr)
        print("[[package]] entry in Cargo.lock at all:", file=sys.stderr)
        for name in missing:
            print(f"  - {name}", file=sys.stderr)
    print("", file=sys.stderr)
    print("This usually means Cargo.lock was committed while .cargo/config.toml's", file=sys.stderr)
    print("local sibling-path patch was active. From this state, `cargo update -p`", file=sys.stderr)
    print("on the affected packages will error with", file=sys.stderr)
    print('  `package ID specification "<pkg>" did not match any packages`', file=sys.stderr)
    print("and only surgical re-injection of the source line repairs the file.", file=sys.stderr)
    print("", file=sys.stderr)
    print("Recovery procedure: see CLAUDE.md > 'Schema service > Dep pinning'.", file=sys.stderr)
    sys.exit(1)

print(f"lint-cargo-lock-sources: ok — all {len(seen)} git-sourced dep(s) in Cargo.lock have source lines.")
PY
