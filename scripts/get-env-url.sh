#!/usr/bin/env bash
# Print a URL from environments.json (the single source of truth).
#
# Usage: scripts/get-env-url.sh <env> <key>
#   env: dev | prod
#   key: region | exemem_api | schema_service | discovery
#
# Exit non-zero if the env or key is missing — never fall back silently,
# because a stale URL is what this registry exists to prevent.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REGISTRY="${SCRIPT_DIR}/../environments.json"

ENV="${1:?env required (dev|prod)}"
KEY="${2:?key required (region|exemem_api|schema_service|discovery)}"

if ! command -v jq >/dev/null 2>&1; then
    echo "get-env-url.sh: jq is required (brew install jq)" >&2
    exit 2
fi

jq -er --arg env "$ENV" --arg key "$KEY" \
    '.environments[$env][$key] // error("missing environments.\($env).\($key) in environments.json")' \
    "$REGISTRY"
