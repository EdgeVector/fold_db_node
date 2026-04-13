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

_cleanup_assert_no_ghosts() {
  # Postcondition for cleanup_all: after every node has been torn down, the
  # discovery Lambda's vector tables MUST be empty. The framework runs under
  # `concurrency.group: e2e-cloud` so nothing else should be writing to dev
  # cloud during a run — any non-zero count after teardown is a silent leak.
  #
  # This is the assertion that would have caught the delete-by-public-key
  # bug on day 1: cleanup reported "OK" for every node, but the rows it
  # was supposed to delete stayed in the database forever, polluting
  # face-search results in subsequent test runs. Runs WARN, not FAIL —
  # we don't want a stray dev-cloud row to red-fail an otherwise green
  # scenario, but the loud log line forces the next person to look.
  if [[ -z "${FOLDDB_TEST_ADMIN_SECRET:-}" ]]; then
    echo "[cleanup] FOLDDB_TEST_ADMIN_SECRET unset; skipping post-teardown ghost assertion" >&2
    return 0
  fi
  local out payload status body fv tv
  out="$(mktemp)"
  payload=$(cat <<JSON
{
  "rawPath": "/admin/counts",
  "requestContext": {"http": {"method": "GET", "path": "/admin/counts"}},
  "headers": {"x-test-admin-secret": "$FOLDDB_TEST_ADMIN_SECRET"},
  "body": ""
}
JSON
)
  if ! aws lambda invoke \
      --function-name "$FOLDDB_TEST_DISCOVERY_LAMBDA" \
      --region "$AWS_REGION" \
      --cli-binary-format raw-in-base64-out \
      --payload "$payload" \
      "$out" >/dev/null 2>&1; then
    echo "[cleanup] post-teardown ghost check: lambda invoke failed" >&2
    rm -f "$out"
    return 0
  fi
  status="$(jq -r '.statusCode // empty' "$out" 2>/dev/null)"
  body="$(jq -r '.body // empty' "$out" 2>/dev/null)"
  rm -f "$out"
  if [[ "$status" != "200" || -z "$body" ]]; then
    echo "[cleanup] post-teardown ghost check: unexpected response status=$status body=$(echo "$body" | head -c 200)" >&2
    return 0
  fi
  fv="$(echo "$body" | jq -r '.discovery_face_vectors // 0' 2>/dev/null || echo "?")"
  tv="$(echo "$body" | jq -r '.discovery_vectors // 0' 2>/dev/null || echo "?")"
  if [[ "$fv" == "0" && "$tv" == "0" ]]; then
    echo "[cleanup] post-teardown discovery counts: face=0 text=0 ✓"
  else
    echo "[cleanup] WARN: post-teardown discovery counts NON-ZERO (face_vectors=$fv discovery_vectors=$tv)." >&2
    echo "[cleanup] WARN: a per-node delete-by-pseudonyms call above reported OK but left rows behind — silent leak somewhere." >&2
    echo "[cleanup] WARN: re-run with FOLDDB_TEST_VERBOSE=1, or invoke /admin/wipe-discovery-vectors manually if this is dev cloud." >&2
  fi
}

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
  local aws_err
  if ! aws_err="$(aws lambda invoke \
      --function-name "$fn" \
      --region "$AWS_REGION" \
      --cli-binary-format raw-in-base64-out \
      --payload "$payload" \
      "$out" 2>&1 >/dev/null)"; then
    echo "[cleanup] lambda invoke failed: $fn $path — $aws_err" >&2
  else
    # A 4xx/5xx from the Lambda shows up in the response body, not the invoke
    # exit code. Parse it and surface so "silently wrong payload shape" bugs
    # (see finding #8 of the framework review) are loud instead of cumulative.
    local status body
    status="$(jq -r '.statusCode // empty' "$out" 2>/dev/null)"
    if [[ -n "$status" && "$status" != "200" ]]; then
      body="$(head -c 300 "$out")"
      echo "[cleanup] lambda $fn $path returned status=$status body=$body" >&2
    fi
  fi
  rm -f "$out"
}

cleanup_all() {
  local nodes_json="$1"
  local api="${FOLDDB_TEST_DEV_API:?}"
  local session_dir="${FOLDDB_TEST_SESSION_DIR:?}"

  # Drain any pending-invite codes that were created before they made it into
  # nodes.json. These would otherwise leak in DynamoDB on SIGINT during spawn.
  local pending_file="$session_dir/state/pending-invites.txt"
  if [[ -f "$pending_file" ]]; then
    local pinv
    while IFS= read -r pinv; do
      [[ -n "$pinv" ]] || continue
      aws dynamodb delete-item \
        --table-name "$FOLDDB_TEST_INVITE_TABLE" \
        --region "$AWS_REGION" \
        --key "{\"code\":{\"S\":\"$pinv\"}}" >/dev/null 2>&1 || true
    done < "$pending_file"
    rm -f "$pending_file"
  fi

  [[ -f "$nodes_json" ]] || { echo "no nodes.json at $nodes_json" >&2; return 0; }

  local count
  count="$(jq 'length' "$nodes_json")"
  local i
  for ((i=0; i<count; i++)); do
    local role port hash api_key invite public_key gstack_port
    role="$(jq -r ".[$i].role" "$nodes_json")"
    port="$(jq -r ".[$i].port" "$nodes_json")"
    hash="$(jq -r ".[$i].hash // \"\"" "$nodes_json")"
    api_key="$(jq -r ".[$i].api_key // \"\"" "$nodes_json")"
    invite="$(jq -r ".[$i].invite_code // \"\"" "$nodes_json")"
    public_key="$(jq -r ".[$i].public_key // \"\"" "$nodes_json")"
    gstack_port="$(jq -r ".[$i].gstack_port // \"\"" "$nodes_json")"

    echo "[cleanup] $role (port=$port hash=$hash)"

    # Fetch this node's published pseudonyms BEFORE killing it. Both the
    # discovery cleanup (step 3) and the messaging cleanup (step 4) need
    # this list, and it's only available while the node process is alive.
    local pseudonyms_json="[]"
    if [[ -n "$hash" ]]; then
      pseudonyms_json="$(curl -fsS "http://127.0.0.1:$port/api/discovery/my-pseudonyms" \
        -H "X-User-Hash: $hash" 2>/dev/null \
        | jq -c '.pseudonyms // [] | map(tostring)' 2>/dev/null || echo '[]')"
    fi

    # Delete account at Exemem (API Gateway route).
    if [[ -n "$api_key" && -n "$hash" ]]; then
      curl -fsS -X DELETE "$api/auth/account" \
        -H "X-User-Hash: $hash" \
        -H "Authorization: Bearer $api_key" \
        >/dev/null 2>&1 || true
    fi

    # Admin: wipe discovery vectors for this node's pseudonyms.
    #
    # NB: an earlier version called /admin/delete-by-public-key with the
    # node's identity public_key. That always matched zero rows because
    # discovery_face_vectors.public_key is per-pseudonym DERIVED
    # (publisher.rs::get_pseudonym_public_key_b64), not the node identity.
    # The correct primitive is /admin/delete-by-pseudonyms, which takes
    # the same pseudonym list we're about to use for messaging cleanup.
    if [[ "$pseudonyms_json" != "[]" && -n "$pseudonyms_json" ]]; then
      _cleanup_lambda_invoke "$FOLDDB_TEST_DISCOVERY_LAMBDA" \
        "/admin/delete-by-pseudonyms" \
        "{\"pseudonyms\":$pseudonyms_json}"
    else
      echo "[cleanup] $role has no published pseudonyms (skip discovery vector cleanup)"
    fi

    # Admin: clear bulletin-board messages for this node's pseudonyms.
    # messaging_service::handle_clear_messages deletes by target_pseudonym.
    if [[ "$pseudonyms_json" != "[]" && -n "$pseudonyms_json" ]]; then
      _cleanup_lambda_invoke "$FOLDDB_TEST_MESSAGING_LAMBDA" \
        "/admin/clear-messages" \
        "{\"pseudonyms\":$pseudonyms_json}"
    else
      echo "[cleanup] $role has no pseudonyms to clear (skip messaging)"
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

    # Shut down per-node gstack daemon (if one was allocated for UI recipes).
    if [[ -n "$gstack_port" && "$gstack_port" != "null" ]]; then
      nf_shutdown_gstack "$gstack_port" || true
    fi
  done

  _cleanup_assert_no_ghosts
}
