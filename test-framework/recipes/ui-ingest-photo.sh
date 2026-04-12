#!/usr/bin/env bash
# UI recipe: ingest a photo via the Import tab.
# Env: NODE_PORT, GSTACK_PORT
# Args: FOLDER_PATH
#
# TODO(phase 5): implement via gstack navigate/click/fill.
set -euo pipefail
: "${NODE_PORT:?}" "${GSTACK_PORT:?}"
FOLDER="${1:?folder path}"

export GSTACK_SERVER_PORT="$GSTACK_PORT"
echo "[ui-ingest-photo] TODO: navigate to http://127.0.0.1:$NODE_PORT/ Import tab, fill folder=$FOLDER, click Scan, click Proceed"
