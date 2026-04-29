#!/usr/bin/env bash
# lint-rev-pin-format.sh
#
# Enforces single-line `rev = "<40-hex>"` pinning for cascade-relevant
# git deps in Cargo.toml.
#
# The bump-cascade bot's sed regex matches `URL", *rev *= *"<sha>"` on a
# single line. If Cargo.toml splits the URL and rev across lines (e.g.
# a `[dependencies.fold_db]` block, multi-line inline table, or
# `fold_db.rev = "..."` syntax), the sed silently misses the dep and
# lands a no-op PR. This lint catches that drift before it ships.
#
# Cascade-relevant URL patterns: EdgeVector/fold_db.git and
# EdgeVector/schema_service.git. Other git deps (e.g. file_to_markdown)
# are not in the cascade and free to use any pin form.
#
# Failure exit: 1.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TOML="$REPO_ROOT/Cargo.toml"

if [ ! -f "$TOML" ]; then
    echo "lint-rev-pin-format: no Cargo.toml at $REPO_ROOT" >&2
    exit 1
fi

URL_PATTERN='https://github\.com/EdgeVector/(fold_db|schema_service)\.git'

errors=0
while IFS= read -r line; do
    linenum="${line%%:*}"
    content="${line#*:}"
    # Skip comment lines.
    if [[ "$content" =~ ^[[:space:]]*# ]]; then
        continue
    fi
    if [[ ! "$content" =~ rev[[:space:]]*=[[:space:]]*\"[0-9a-f]{40}\" ]]; then
        echo "lint-rev-pin-format: FAIL" >&2
        echo "  $TOML:$linenum" >&2
        echo "  Cargo.toml line references EdgeVector/(fold_db|schema_service) but lacks a 40-hex 'rev = \"...\"' on the same line." >&2
        echo "  Got: $content" >&2
        errors=$((errors + 1))
    fi
done < <(grep -nE "$URL_PATTERN" "$TOML")

if [ "$errors" -gt 0 ]; then
    cat >&2 <<'EOF'

The bump-cascade bot's regex matches `URL", *rev *= *"<sha>"` on a single
line. If Cargo.toml splits the URL and rev across lines, the sed silently
misses the dep and lands a no-op PR. Keep cascade-relevant deps as
single-line inline tables:

  name = { git = "https://github.com/EdgeVector/X.git", rev = "<40-hex>", ... }
EOF
    exit 1
fi

echo "lint-rev-pin-format: ok"
