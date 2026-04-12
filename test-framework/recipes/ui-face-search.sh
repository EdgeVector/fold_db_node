#!/usr/bin/env bash
# UI recipe: run face search from a record page.
# Env: NODE_PORT, GSTACK_PORT
# Args: SCHEMA RECORD FACE_INDEX
#
# NOTE(phase 5): single-worker UI path only. Parallel UI testing TODO.
set -euo pipefail
: "${NODE_PORT:?}" "${GSTACK_PORT:?}"
SCHEMA="${1:?schema}"
RECORD="${2:?record}"
FACE="${3:-0}"

export GSTACK_SERVER_PORT="$GSTACK_PORT"
B="$HOME/.claude/skills/gstack/browse/dist/browse"
[[ -x "$B" ]] || { echo "[ui-face-search] gstack binary not found at $B" >&2; exit 1; }

"$B" goto "http://127.0.0.1:$NODE_PORT/#/browse/$SCHEMA/$RECORD"
sleep 2
"$B" click 'button:has-text("Source"), button:has-text("Source info")' || true
sleep 1
"$B" click "button[data-face-index=\"$FACE\"], .face-thumb:nth-of-type($((FACE + 1)))"
sleep 2
echo "[ui-face-search] triggered face search on $SCHEMA/$RECORD face=$FACE"
