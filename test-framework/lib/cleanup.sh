#!/usr/bin/env bash
# Idempotent per-run cleanup. Given a nodes.json, tear down each node.
set -euo pipefail

cleanup_all() {
  local nodes_json="$1"
  [[ -f "$nodes_json" ]] || { echo "no nodes.json at $nodes_json" >&2; return 0; }

  local api="${FOLDDB_TEST_DEV_API:-https://api-dev.exemem.com}"
  local session_dir="${FOLDDB_TEST_SESSION_DIR:?}"

  local count
  count="$(jq 'length' "$nodes_json")"
  local i
  for ((i=0; i<count; i++)); do
    local role port hash api_key invite
    role="$(jq -r ".[$i].role" "$nodes_json")"
    port="$(jq -r ".[$i].port" "$nodes_json")"
    hash="$(jq -r ".[$i].hash" "$nodes_json")"
    api_key="$(jq -r ".[$i].api_key // \"\"" "$nodes_json")"
    invite="$(jq -r ".[$i].invite_code // \"\"" "$nodes_json")"

    echo "[cleanup] $role (port=$port hash=$hash)"

    # Delete account at Exemem
    curl -fsS -X DELETE "$api/auth/account" \
      -H "X-User-Hash: $hash" \
      -H "Authorization: Bearer $api_key" \
      >/dev/null 2>&1 || true

    # Admin: delete by public key
    curl -fsS -X POST "$api/admin/delete-by-public-key" \
      -H "X-User-Hash: $hash" \
      >/dev/null 2>&1 || true

    # Admin: clear messages
    curl -fsS -X POST "$api/admin/clear-messages" \
      -H "X-User-Hash: $hash" \
      >/dev/null 2>&1 || true

    # Revoke invite code
    if [[ -n "$invite" ]]; then
      aws dynamodb delete-item --table-name ExememInviteCodes-dev \
        --key "{\"code\":{\"S\":\"$invite\"}}" >/dev/null 2>&1 || true
    fi

    # Kill process
    local pidfile="$session_dir/nodes/$role/pid"
    if [[ -f "$pidfile" ]]; then
      local pid
      pid="$(cat "$pidfile")"
      kill "$pid" 2>/dev/null || true
      sleep 0.1
      kill -9 "$pid" 2>/dev/null || true
    fi
  done
}
