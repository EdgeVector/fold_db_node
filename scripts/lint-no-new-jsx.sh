#!/usr/bin/env bash
# lint-no-new-jsx.sh
#
# Frontend TypeScript migration guardrail. The web UI in
# src/server/static-react/ is being migrated from JS/JSX to TS/TSX. To
# prevent new debt while migration is in flight, every legacy .js/.jsx
# file must be listed in scripts/legacy-jsx-allowlist.txt. Adding a new
# .js/.jsx (instead of .ts/.tsx) fails CI; removing one from the allowlist
# without converting it also fails CI.
#
# How to remove an entry: convert the file to .ts/.tsx, delete the line
# from scripts/legacy-jsx-allowlist.txt, run this lint locally, commit.
# When the allowlist is empty, delete it and this script.
#
# Build/config glob (vite, eslint, tailwind, postcss, vitest configs and
# anything under scripts/) stays JS forever and is excluded from the
# check via IGNORE_PATTERNS below.
#
# Failure exit: 1.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

ALLOWLIST="scripts/legacy-jsx-allowlist.txt"
SCAN_ROOT="src/server/static-react"

if [ ! -f "$ALLOWLIST" ]; then
    echo "lint-no-new-jsx: FAIL" >&2
    echo "  Allowlist $ALLOWLIST is missing." >&2
    exit 1
fi

# Patterns that are allowed to stay as .js forever (build / config / one-off
# scripts). Matched against paths relative to the repo root with a leading
# 'src/server/static-react/' prefix.
IGNORE_PATTERNS=(
    'src/server/static-react/scripts/'
    'src/server/static-react/vite.config.js'
    'src/server/static-react/vite.config.lib.js'
    'src/server/static-react/vitest.config.js'
    'src/server/static-react/eslint.config.js'
    'src/server/static-react/tailwind.config.js'
    'src/server/static-react/postcss.config.js'
)

is_ignored() {
    local path="$1"
    for pat in "${IGNORE_PATTERNS[@]}"; do
        if [[ "$path" == "$pat"* ]]; then
            return 0
        fi
    done
    return 1
}

# Read allowlist (skip blank lines and # comments). Sort for deterministic diffs.
expected=$(grep -vE '^\s*(#|$)' "$ALLOWLIST" | sort -u)

# Scan tracked files under static-react.
tracked=$(git ls-files \
    "$SCAN_ROOT/**/*.js" \
    "$SCAN_ROOT/**/*.jsx" \
    "$SCAN_ROOT/*.js" \
    "$SCAN_ROOT/*.jsx" 2>/dev/null \
    | sort -u)

# Subtract ignored patterns to get the migration set.
actual=""
while IFS= read -r f; do
    [ -z "$f" ] && continue
    if ! is_ignored "$f"; then
        actual+="${f}"$'\n'
    fi
done <<< "$tracked"
actual=$(printf '%s' "$actual" | sed '/^$/d' | sort -u)

# Files present but not in allowlist → new debt added.
unexpected=$(comm -23 <(printf '%s\n' "$actual") <(printf '%s\n' "$expected"))

# Files in allowlist but not present → entry should be removed (file was converted or deleted).
stale=$(comm -13 <(printf '%s\n' "$actual") <(printf '%s\n' "$expected"))

errors=0

if [ -n "$unexpected" ]; then
    echo "lint-no-new-jsx: FAIL — new .js/.jsx files outside the allowlist" >&2
    while IFS= read -r f; do
        [ -z "$f" ] && continue
        echo "  + $f" >&2
        errors=$((errors + 1))
    done <<< "$unexpected"
    cat >&2 <<EOF

  New frontend code in this repo MUST be .ts/.tsx. Convert these files,
  or — if they are genuinely build/config that has to stay JS — extend
  IGNORE_PATTERNS in scripts/lint-no-new-jsx.sh with a one-line reason.

EOF
fi

if [ -n "$stale" ]; then
    echo "lint-no-new-jsx: FAIL — allowlist entries no longer exist" >&2
    while IFS= read -r f; do
        [ -z "$f" ] && continue
        echo "  - $f" >&2
        errors=$((errors + 1))
    done <<< "$stale"
    cat >&2 <<EOF

  These files were converted or deleted. Remove the matching lines from
  $ALLOWLIST and commit.

EOF
fi

if [ "$errors" -gt 0 ]; then
    exit 1
fi

echo "lint-no-new-jsx: ok ($(printf '%s\n' "$actual" | sed '/^$/d' | wc -l | tr -d ' ') legacy files remaining)"
