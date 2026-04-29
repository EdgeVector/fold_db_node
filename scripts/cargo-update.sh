#!/usr/bin/env bash
# cargo-update.sh
#
# Wrapper for `cargo update` that protects Cargo.lock from the
# missing-source trap (see scripts/lint-cargo-lock-sources.sh and
# CLAUDE.md for full context).
#
# Why this exists: when `.cargo/config.toml` patches `fold_db` /
# `schema_service` to sibling local paths, every `cargo update`
# invocation rewrites Cargo.lock without `source = "git+..."` lines
# for those packages, because cargo can't write a source spec for a
# `path = "..."` package. CI then fails (CI strips the patch and
# tries to resolve from the lockfile pin), and from that point on
# `cargo update -p <pkg>` errors with
#   `package ID specification "<pkg>" did not match any packages`
# because cargo identifies packages by (name, version, source).
#
# This script:
#   1. Moves .cargo/config.toml aside if present.
#   2. Runs `cargo update "$@"` against the unpatch-ed configuration
#      (resolves git deps from their declared `git = "..."` sources).
#   3. Restores .cargo/config.toml on EXIT — success, error, or kill.
#   4. Runs scripts/lint-cargo-lock-sources.sh to verify the lockfile
#      didn't drift even with the config moved aside.
#
# Don't run other cargo commands in parallel with this script — while
# config.toml is moved aside, any concurrent `cargo build` will lose
# the local-sibling patches and may hit the dual-`fold_db` trap
# documented in CLAUDE.md.
#
# Usage:
#   bash scripts/cargo-update.sh -p schema_service_core
#   bash scripts/cargo-update.sh -p fold_db --precise <sha>
#   bash scripts/cargo-update.sh                       # update all (rare)
#
# Exit code: cargo's exit code, or 2 if the post-update lint catches
# missing source lines (which would indicate a deeper issue, since
# we ran with the patch moved aside).

set -euo pipefail

SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
REPO_ROOT="$( cd "$SCRIPT_DIR/.." && pwd )"
cd "$REPO_ROOT"

CONFIG_PATH=".cargo/config.toml"
STASH_PATH=""

restore() {
    if [[ -n "$STASH_PATH" && -f "$STASH_PATH" ]]; then
        mv "$STASH_PATH" "$CONFIG_PATH"
        echo "cargo-update: restored $CONFIG_PATH from $STASH_PATH" >&2
    fi
}
trap restore EXIT INT TERM

if [[ -f "$CONFIG_PATH" ]]; then
    STASH_PATH="$(mktemp -t cargo-config.XXXXXXXX)"
    mv "$CONFIG_PATH" "$STASH_PATH"
    echo "cargo-update: moved $CONFIG_PATH aside to $STASH_PATH" >&2
else
    echo "cargo-update: no $CONFIG_PATH present; running cargo update directly" >&2
fi

# Run the update. We don't `set -e` around it because we still want
# the trap to fire and restore config.toml on cargo failure.
set +e
cargo update "$@"
cargo_exit=$?
set -e

if [[ $cargo_exit -ne 0 ]]; then
    echo "cargo-update: cargo update failed (exit $cargo_exit) — config.toml will be restored" >&2
    exit $cargo_exit
fi

# Post-update sanity check. With the patch moved aside, the resolved
# lockfile entries for git deps must carry their `source = "git+..."`
# lines. If they don't, something else is rewriting the lockfile
# (e.g., a global ~/.cargo/config.toml with a [patch] section).
if [[ -x "$SCRIPT_DIR/lint-cargo-lock-sources.sh" ]]; then
    if ! bash "$SCRIPT_DIR/lint-cargo-lock-sources.sh"; then
        echo "" >&2
        echo "cargo-update: lint failed — Cargo.lock has missing source lines" >&2
        echo "  This should not happen with .cargo/config.toml moved aside." >&2
        echo "  Check for a global ~/.cargo/config.toml with [patch] sections," >&2
        echo "  and see CLAUDE.md 'missing-source recovery' for repair steps." >&2
        exit 2
    fi
fi

echo "cargo-update: done. Cargo.lock is clean." >&2
