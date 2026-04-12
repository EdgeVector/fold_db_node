#!/usr/bin/env bash
# Fetch notifications.
# Env: NODE_PORT, USER_HASH
set -euo pipefail
: "${NODE_PORT:?}" "${USER_HASH:?}"

curl -fsS "http://127.0.0.1:$NODE_PORT/api/notifications" \
  -H "X-User-Hash: $USER_HASH"
