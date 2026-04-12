#!/usr/bin/env bash
# UI recipe: run face search from a record page.
# Env: NODE_PORT, GSTACK_PORT
# Args: SCHEMA RECORD FACE_INDEX
#
# TODO(phase 5): implement via gstack.
set -euo pipefail
: "${NODE_PORT:?}" "${GSTACK_PORT:?}"
SCHEMA="${1:?schema}"
RECORD="${2:?record}"
FACE="${3:-0}"

export GSTACK_SERVER_PORT="$GSTACK_PORT"
echo "[ui-face-search] TODO: Browse > $SCHEMA > $RECORD, expand Source info, click Face $FACE"
