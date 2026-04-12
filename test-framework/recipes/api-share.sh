#!/usr/bin/env bash
# Share a record with a contact.
# Env: NODE_PORT, USER_HASH
# Args: CONTACT_PSEUDONYM SCHEMA RECORD_KEY
set -euo pipefail
: "${NODE_PORT:?}" "${USER_HASH:?}"
CONTACT="${1:?contact pseudonym}"
SCHEMA="${2:?schema}"
RECORD="${3:?record key}"

curl -fsS -X POST "http://127.0.0.1:$NODE_PORT/api/discovery/share" \
  -H 'content-type: application/json' \
  -H "X-User-Hash: $USER_HASH" \
  -d "{\"contact_pseudonym\":\"$CONTACT\",\"schema\":\"$SCHEMA\",\"record_key\":\"$RECORD\"}"
