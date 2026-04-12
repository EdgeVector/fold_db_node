#!/usr/bin/env bash
# Idempotent per-run cleanup. Given a nodes.json, tear down each node.
#
# Admin endpoints (/admin/delete-by-public-key, /admin/clear-messages) are NOT
# exposed through API Gateway. They must be invoked directly against the Lambda
# functions. The account delete (DELETE /auth/account) and invite-code removal
# (DynamoDB DeleteItem) stay on their regular paths.
set -euo pipefail

# Lambda function names (dev env).
: "${FOLDDB_TEST_DISCOVERY_LAMBDA:=ExememDiscovery-dev}"
: "${FOLDDB_TEST_MESSAGING_LAMBDA:=ExememMessagingService-dev}"
: "${FOLDDB_TEST_INVITE_TABLE:=ExememInviteCodes-dev}"
: "${AWS_REGION:=us-west-2}"

_cleanup_lambda_invoke() {
  # Args: LAMBDA_FN PATH JSON_BODY
  local fn="$1" path="$2" body="$3"
  local secret="${FOLDDB_TEST_ADMIN_SECRET:-}"
  if [[ -z "$secret" ]]; then
    echo "[cleanup] FOLDDB_TEST_ADMIN_SECRET unset; skipping $path on $fn" >&2
    return 0
  fi
  # Escape body for embedding in JSON payload.
  local escaped
  escaped="$(printf '%s' "$body" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))')"
  local payload
  payload=$(cat <<JSON
{
  "rawPath": "$path",
  "requestContext": {"http": {"method": "POST", "path": "$path"}},
  "headers": {
    "content-type": "application/json",
    "x-test-admin-secret": "$secret"
  },
  "body": $escaped
}
JSON
)
  local out
  out="$(mktemp)"
  if aws lambda invoke \
      --function-name "$fn" \
      --region "$AWS_REGION" \
      --cli-binary-format raw-in-base64-out \
      --payload "$payload" \
      "$out" >/dev/null 2>&1; then
    :
  else
    echo "[cleanup] lambda invoke failed: $fn $path" >&2
  fi
  rm -f "$out"
}

cleanup_all() {
  local nodes_json="$1"
  [[ -f "$nodes_json" ]] || { echo "no nodes.json at $nodes_json" >&2; return 0; }

  local api="${FOLDDB_TEST_DEV_API:?}"
  local session_dir="${FOLDDB_TEST_SESSION_DIR:?}"

  local count
  count="$(jq 'length' "$nodes_json")"
  local i
  for ((i=0; i<count; i++)); do
    local role port hash api_key invite public_key
    role="$(jq -r ".[$i].role" "$nodes_json")"
    port="$(jq -r ".[$i].port" "$nodes_json")"
    hash="$(jq -r ".[$i].hash" "$nodes_json")"
    api_key="$(jq -r ".[$i].api_key // \"\"" "$nodes_json")"
    invite="$(jq -r ".[$i].invite_code // \"\"" "$nodes_json")"
    public_key="$(jq -r ".[$i].public_key // \"\"" "$nodes_json")"

    echo "[cleanup] $role (port=$port hash=$hash)"

    # Delete account at Exemem (API Gateway route).
    if [[ -n "$api_key" && -n "$hash" ]]; then
      curl -fsS -X DELETE "$api/auth/account" \
        -H "X-User-Hash: $hash" \
        -H "Authorization: Bearer $api_key" \
        >/dev/null 2>&1 || true
    fi

    # Admin: delete by public key (direct Lambda invoke).
    if [[ -n "$public_key" ]]; then
      _cleanup_lambda_invoke "$FOLDDB_TEST_DISCOVERY_LAMBDA" \
        "/admin/delete-by-public-key" \
        "{\"public_key\":\"$public_key\"}"
    fi

    # Admin: clear messages (direct Lambda invoke).
    if [[ -n "$hash" ]]; then
      _cleanup_lambda_invoke "$FOLDDB_TEST_MESSAGING_LAMBDA" \
        "/admin/clear-messages" \
        "{\"user_hash\":\"$hash\"}"
    fi

    # Revoke invite code.
    if [[ -n "$invite" ]]; then
      aws dynamodb delete-item \
        --table-name "$FOLDDB_TEST_INVITE_TABLE" \
        --region "$AWS_REGION" \
        --key "{\"code\":{\"S\":\"$invite\"}}" >/dev/null 2>&1 || true
    fi

    # Kill local process.
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
