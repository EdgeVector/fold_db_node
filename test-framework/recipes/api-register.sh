#!/usr/bin/env bash
# Register a node with Exemem using an invite code.
# Env: NODE_PORT
# Args: INVITE_CODE
set -euo pipefail
: "${NODE_PORT:?}"
INVITE="${1:?invite code required}"

curl -fsS -X POST "http://127.0.0.1:$NODE_PORT/api/auth/register" \
  -H 'content-type: application/json' \
  -d "{\"invite_code\":\"$INVITE\"}"
