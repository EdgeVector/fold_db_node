#!/usr/bin/env bash
# Send a connection request to a target pseudonym.
# Env: NODE_PORT, USER_HASH
# Args: TARGET_PSEUDONYM MESSAGE PREFERRED_ROLE
set -euo pipefail
: "${NODE_PORT:?}" "${USER_HASH:?}"
TARGET="${1:?target pseudonym}"
MSG="${2:-hi}"
ROLE="${3:-peer}"

curl -fsS -X POST "http://127.0.0.1:$NODE_PORT/api/discovery/connect" \
  -H 'content-type: application/json' \
  -H "X-User-Hash: $USER_HASH" \
  -d "{\"target_pseudonym\":\"$TARGET\",\"message\":\"$MSG\",\"preferred_role\":\"$ROLE\"}"
