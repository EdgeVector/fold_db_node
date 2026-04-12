#!/usr/bin/env bash
# UI recipe: ingest a photo via the Import tab.
# Env: NODE_PORT, GSTACK_PORT
# Args: FOLDER_PATH
#
# NOTE(phase 5): gstack only supports a single daemon port. For v1, UI recipes
# run against the default gstack instance — parallel UI testing across multiple
# workers needs gstack multi-instance support (TODO).
set -euo pipefail
: "${NODE_PORT:?}" "${GSTACK_PORT:?}"
FOLDER="${1:?folder path}"

export GSTACK_SERVER_PORT="$GSTACK_PORT"
B="$HOME/.claude/skills/gstack/browse/dist/browse"
[[ -x "$B" ]] || { echo "[ui-ingest-photo] gstack binary not found at $B" >&2; exit 1; }

"$B" goto "http://127.0.0.1:$NODE_PORT/"
sleep 2
"$B" click 'a[href="#/import"], button:has-text("Import")' || true
sleep 1
"$B" fill 'input[name="folder"], input[placeholder*="folder" i]' "$FOLDER"
"$B" click 'button:has-text("Scan")'
sleep 3
"$B" click 'button:has-text("Proceed"), button:has-text("Ingest")'
sleep 2
echo "[ui-ingest-photo] requested ingest for $FOLDER"
