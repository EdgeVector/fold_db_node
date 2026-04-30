#!/usr/bin/env bash
# lint-no-hardcoded-urls.sh
#
# Enforces the cross-environment URL registry: all gateway hostnames must
# live in environments.json (the single source of truth). Any hardcoded
# occurrence elsewhere — Rust, shell, JSON, Markdown — is drift waiting to
# happen. build.rs generates the per-(env, key) constants in OUT_DIR;
# Rust callers go through `endpoints::*`. Shell scripts go through
# `scripts/get-env-url.sh`.
#
# Failure exit: 1.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# The four current API Gateway IDs. Update by editing environments.json
# (which this lint reads back); never edit this list by hand.
HOSTNAMES=$(
    jq -r '.environments | to_entries[] | .value | to_entries[] | select(.key != "region") | .value' \
        environments.json \
    | sed -E 's|^https?://||; s|/.*$||' \
    | sort -u
)

# Allowlist: where these hostnames are allowed to appear.
#   environments.json    — the registry itself
#   target/              — generated artifacts (cargo build output)
#   scripts/lint-no-hardcoded-urls.sh — this file (allowed because it
#                                        derives the list dynamically; no
#                                        literal hostname appears here)
#   .git/                — git internals
#   .claude/worktrees/   — sibling worktrees of this same repo (other
#                          checkouts of the same files)
#   docs/dogfood/*.md    — historical run reports; immutable record of
#                          past sessions, not live config

ALLOW_PATHS=(
    './environments.json'
    './target/'
    './scripts/lint-no-hardcoded-urls.sh'
    './.git/'
    './.claude/worktrees/'
    './docs/dogfood/'
    './snapshots/'
)

errors=0
for host in $HOSTNAMES; do
    # `git ls-files` so we only scan tracked files, regardless of cwd noise.
    # Fall back to `find` when not in a git repo (e.g. fresh tarball).
    if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
        candidates=$(git ls-files -z | xargs -0 grep -lF "$host" 2>/dev/null || true)
    else
        candidates=$(grep -rlF "$host" . 2>/dev/null \
            | grep -v '^\./target/' \
            | grep -v '^\./.git/' || true)
    fi

    while IFS= read -r f; do
        [ -z "$f" ] && continue
        # Normalize to leading ./ for allow-list matching.
        path="./${f#./}"
        skip=0
        for allow in "${ALLOW_PATHS[@]}"; do
            if [[ "$path" == "$allow"* ]]; then
                skip=1
                break
            fi
        done
        [ "$skip" -eq 1 ] && continue

        echo "lint-no-hardcoded-urls: FAIL" >&2
        echo "  $path contains hardcoded gateway hostname '$host'." >&2
        # Show the offending lines so the fixer doesn't have to grep.
        grep -nF "$host" "$f" | sed 's/^/    /' >&2
        errors=$((errors + 1))
    done <<< "$candidates"
done

if [ "$errors" -gt 0 ]; then
    cat >&2 <<'EOF'

The cross-environment URLs are owned by environments.json (the single
source of truth). Do not hardcode them anywhere else.

  Rust:  use fold_db_node::endpoints::{schema_service_url, ...}
         or, for env-pinned access, schema_service_url_for(Environment::Dev).
  Shell: use $(scripts/get-env-url.sh <dev|prod> <key>).
  Docs:  reference the env name (dev/prod), not the literal URL.

If a file genuinely needs the URL inline (e.g. a frozen historical run
report), add its prefix to ALLOW_PATHS in scripts/lint-no-hardcoded-urls.sh
with a comment explaining why.
EOF
    exit 1
fi

echo "lint-no-hardcoded-urls: ok"
