#!/usr/bin/env bash
# Publish local opt-ins to discovery service.
# Env: NODE_PORT, USER_HASH
set -euo pipefail
: "${NODE_PORT:?}" "${USER_HASH:?}"

curl -fsS -X POST "http://127.0.0.1:$NODE_PORT/api/discovery/publish" \
  -H 'content-type: application/json' \
  -H "X-User-Hash: $USER_HASH" \
  -d '{}'
