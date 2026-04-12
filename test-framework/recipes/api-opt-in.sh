#!/usr/bin/env bash
# Opt a schema into discovery.
# Env: NODE_PORT, USER_HASH
# Args: SCHEMA_NAME CATEGORY [PUBLISH_FACES]
set -euo pipefail
: "${NODE_PORT:?}" "${USER_HASH:?}"
SCHEMA="${1:?schema name}"
CATEGORY="${2:?category}"
PUBLISH_FACES="${3:-false}"

curl -fsS -X POST "http://127.0.0.1:$NODE_PORT/api/discovery/opt-in" \
  -H 'content-type: application/json' \
  -H "X-User-Hash: $USER_HASH" \
  -d "{\"schema_name\":\"$SCHEMA\",\"category\":\"$CATEGORY\",\"publish_faces\":$PUBLISH_FACES}"
