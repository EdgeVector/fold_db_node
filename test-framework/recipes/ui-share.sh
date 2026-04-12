#!/usr/bin/env bash
# UI recipe: share a record with a contact via Browse > record > Share.
# Env: NODE_PORT, GSTACK_PORT
# Args: SCHEMA RECORD CONTACT_DISPLAY_NAME
#
# NOTE(phase 5): single-worker UI path only. Parallel UI testing TODO.
set -euo pipefail
: "${NODE_PORT:?}" "${GSTACK_PORT:?}"
SCHEMA="${1:?schema}"
RECORD="${2:?record}"
CONTACT="${3:?contact}"

export GSTACK_SERVER_PORT="$GSTACK_PORT"
B="$HOME/.claude/skills/gstack/browse/dist/browse"
[[ -x "$B" ]] || { echo "[ui-share] gstack binary not found at $B" >&2; exit 1; }

"$B" goto "http://127.0.0.1:$NODE_PORT/#/browse/$SCHEMA/$RECORD"
sleep 2
"$B" click 'button:has-text("Share")'
sleep 1
"$B" fill 'input[placeholder*="contact" i], input[name="contact"]' "$CONTACT"
sleep 1
"$B" click "li:has-text(\"$CONTACT\"), [role=option]:has-text(\"$CONTACT\")" || true
"$B" click 'button:has-text("Confirm"), button:has-text("Share")'
sleep 2
echo "[ui-share] shared $SCHEMA/$RECORD with $CONTACT"
