#!/usr/bin/env bash
# lint-no-new-jsx.sh
#
# Frontend TypeScript migration is COMPLETE — there must be zero tracked
# .js / .jsx files inside src/server/static-react/src/. This lint enforces
# the post-migration invariant: any new .js/.jsx in that directory fails CI.
#
# Build/config files at the static-react root (vite/eslint/postcss/tailwind
# configs and anything under scripts/) and the e2e/ Playwright dir are
# allowed to stay JS — listed in IGNORE_PATTERNS below.
#
# Failure exit: 1.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

SCAN_ROOT="src/server/static-react"

# Patterns allowed to stay as .js / .jsx (build / config / one-off scripts /
# Playwright e2e). Matched against paths relative to the repo root with the
# leading 'src/server/static-react/' prefix.
IGNORE_PATTERNS=(
    'src/server/static-react/scripts/'
    'src/server/static-react/e2e/'
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

# Scan tracked .js / .jsx under static-react, excluding ignored patterns.
tracked=$(git ls-files \
    "$SCAN_ROOT/**/*.js" \
    "$SCAN_ROOT/**/*.jsx" \
    "$SCAN_ROOT/*.js" \
    "$SCAN_ROOT/*.jsx" 2>/dev/null \
    | sort -u)

offenders=""
while IFS= read -r f; do
    [ -z "$f" ] && continue
    if ! is_ignored "$f"; then
        offenders+="${f}"$'\n'
    fi
done <<< "$tracked"
offenders=$(printf '%s' "$offenders" | sed '/^$/d')

if [ -n "$offenders" ]; then
    echo "lint-no-new-jsx: FAIL — .js/.jsx file(s) found under $SCAN_ROOT/src/" >&2
    echo "$offenders" | sed 's/^/  + /' >&2
    cat >&2 <<EOF

  The frontend TypeScript migration is complete: every file under
  src/server/static-react/src/ MUST be .ts / .tsx. Convert these files
  to TypeScript, or — if they are genuinely build/config that has to
  stay JS — extend IGNORE_PATTERNS in scripts/lint-no-new-jsx.sh with a
  one-line reason.

EOF
    exit 1
fi

echo "lint-no-new-jsx: ok (no .js/.jsx files under $SCAN_ROOT outside the ignore list)"
