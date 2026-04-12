#!/usr/bin/env bash
# Respond to a connection request.
# Env: NODE_PORT, USER_HASH
# Args: REQUEST_ID (accept|decline)
set -euo pipefail
: "${NODE_PORT:?}" "${USER_HASH:?}"
REQ_ID="${1:?request id}"
DECISION="${2:?accept|decline}"

curl -fsS -X POST "http://127.0.0.1:$NODE_PORT/api/discovery/connection-requests/respond" \
  -H 'content-type: application/json' \
  -H "X-User-Hash: $USER_HASH" \
  -d "{\"request_id\":\"$REQ_ID\",\"decision\":\"$DECISION\"}"
