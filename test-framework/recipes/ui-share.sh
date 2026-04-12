#!/usr/bin/env bash
# UI recipe: share a record with a contact via Browse > record > Share.
# Env: NODE_PORT, GSTACK_PORT
# Args: SCHEMA RECORD CONTACT_DISPLAY_NAME
#
# TODO(phase 5): implement via gstack.
set -euo pipefail
: "${NODE_PORT:?}" "${GSTACK_PORT:?}"
SCHEMA="${1:?schema}"
RECORD="${2:?record}"
CONTACT="${3:?contact}"

export GSTACK_SERVER_PORT="$GSTACK_PORT"
echo "[ui-share] TODO: Browse > $SCHEMA > $RECORD, click Share, select $CONTACT, confirm"
