#!/usr/bin/env bash
# E2E test framework entry point.
set -euo pipefail

FRAMEWORK_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

usage() {
  cat <<EOF
Usage: run-scenario.sh <scenario.yaml> [--run-id ID] [--keep-session] [--dry-run]

Options:
  --run-id ID       Use a specific run id (default: generated)
  --keep-session    Do not delete the session dir on exit
  --dry-run         Validate the scenario and exit
EOF
  exit 1
}

SCENARIO=""
RUN_ID=""
KEEP_SESSION=0
DRY_RUN=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --run-id) RUN_ID="$2"; shift 2 ;;
    --keep-session) KEEP_SESSION=1; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) usage ;;
    *)
      if [[ -z "$SCENARIO" ]]; then
        SCENARIO="$1"; shift
      else
        echo "Unknown arg: $1" >&2; usage
      fi
      ;;
  esac
done

[[ -n "$SCENARIO" ]] || usage
[[ -f "$SCENARIO" ]] || { echo "Scenario not found: $SCENARIO" >&2; exit 1; }

if [[ -z "$RUN_ID" ]]; then
  RUN_ID="run-$(date +%Y%m%d-%H%M%S)-$$"
fi

SESSION_DIR="$FRAMEWORK_DIR/logs/$RUN_ID"
mkdir -p "$SESSION_DIR/state" "$SESSION_DIR/nodes"

# Validate scenario with ajv if available
SCHEMA="$FRAMEWORK_DIR/scenarios/schema.json"
if command -v ajv >/dev/null 2>&1; then
  # Convert yaml to json for validation
  if command -v yq >/dev/null 2>&1; then
    TMP_JSON="$(mktemp)"
    yq -o=json '.' "$SCENARIO" > "$TMP_JSON"
    if ajv validate -s "$SCHEMA" -d "$TMP_JSON" >/dev/null 2>&1; then
      echo "[run-scenario] scenario valid: $SCENARIO"
    else
      echo "[run-scenario] scenario INVALID: $SCENARIO" >&2
      ajv validate -s "$SCHEMA" -d "$TMP_JSON" >&2 || true
      rm -f "$TMP_JSON"
      exit 2
    fi
    rm -f "$TMP_JSON"
  else
    echo "[run-scenario] yq not installed; skipping schema validation" >&2
  fi
else
  echo "[run-scenario] ajv not installed; skipping schema validation" >&2
fi

if [[ "$DRY_RUN" == "1" ]]; then
  echo "[run-scenario] dry-run OK"
  exit 0
fi

export FOLDDB_TEST_RUN_ID="$RUN_ID"
export FOLDDB_TEST_SESSION_DIR="$SESSION_DIR"
export FOLDDB_TEST_DEV_API="${FOLDDB_TEST_DEV_API:-https://ygyu7ritx8.execute-api.us-west-2.amazonaws.com}"
export FOLDDB_TEST_DEV_SCHEMA="${FOLDDB_TEST_DEV_SCHEMA:-https://y0q3m6vk75.execute-api.us-west-2.amazonaws.com}"
export AWS_REGION="${AWS_REGION:-us-west-2}"

# Load test admin secret for cleanup Lambda invocations.
ADMIN_SECRET_FILE="${FOLDDB_TEST_ADMIN_SECRET_FILE:-$HOME/.folddb/test-admin-secret-dev.txt}"
if [[ -z "${FOLDDB_TEST_ADMIN_SECRET:-}" && -f "$ADMIN_SECRET_FILE" ]]; then
  FOLDDB_TEST_ADMIN_SECRET="$(tr -d '[:space:]' < "$ADMIN_SECRET_FILE")"
  export FOLDDB_TEST_ADMIN_SECRET
fi

# Standalone smoke driver: spawn nodes, register, cleanup. Triggered by
# FOLDDB_TEST_STANDALONE=1 (the Claude-driven agent driver is aspirational).
run_standalone_smoke() {
  local scenario="$1"
  local nodes_json="$SESSION_DIR/state/nodes.json"
  echo "[standalone] smoke-running $scenario"

  # Parse node roles from the scenario.
  local roles
  if command -v yq >/dev/null 2>&1; then
    roles="$(yq -r '.nodes[].role' "$scenario")"
  else
    roles="$(python3 -c "import yaml,sys; [print(n['role']) for n in yaml.safe_load(open('$scenario'))['nodes']]")"
  fi
  [[ -n "$roles" ]] || { echo "[standalone] no nodes in scenario" >&2; return 2; }

  local port=40000
  echo "[]" > "$nodes_json"
  local trapcmd="cleanup_all '$nodes_json' || true"
  trap "$trapcmd" EXIT

  while read -r role; do
    [[ -n "$role" ]] || continue
    echo "[standalone] --- node: $role (port $port) ---"
    local invite
    invite="$(nf_create_invite_codes 1)"
    echo "[standalone] invite: $invite"
    nf_spawn_node "$role" "$port" "$SESSION_DIR" >/dev/null
    nf_wait_healthy "$port" 30
    local reg_resp
    reg_resp="$(nf_register_node "$port" "$invite")"
    echo "[standalone] register resp: $reg_resp"
    local hash api_key public_key
    hash="$(echo "$reg_resp" | jq -r '.user_hash // .hash // ""')"
    api_key="$(echo "$reg_resp" | jq -r '.api_key // ""')"
    public_key="$(echo "$reg_resp" | jq -r '.public_key // ""')"
    if [[ -z "$hash" || -z "$public_key" ]]; then
      # Fallback: fetch from auto-identity endpoint.
      local ident
      ident="$(curl -fsS "http://127.0.0.1:$port/api/system/auto-identity" || true)"
      [[ -z "$hash" ]]        && hash="$(echo "$ident" | jq -r '.user_hash // ""')"
      [[ -z "$public_key" ]]  && public_key="$(echo "$ident" | jq -r '.public_key // ""')"
    fi
    jq --arg role "$role" --argjson port "$port" \
       --arg hash "$hash" --arg api_key "$api_key" \
       --arg invite "$invite" --arg public_key "$public_key" \
       '. + [{role:$role, port:$port, hash:$hash, api_key:$api_key, invite_code:$invite, public_key:$public_key}]' \
       "$nodes_json" > "$nodes_json.tmp" && mv "$nodes_json.tmp" "$nodes_json"
    port=$((port + 1))
  done <<< "$roles"

  echo "[standalone] nodes.json:"
  cat "$nodes_json"
  echo "[standalone] smoke complete; running cleanup"
}

# shellcheck source=lib/node_factory.sh
source "$FRAMEWORK_DIR/lib/node_factory.sh"
# shellcheck source=lib/coordination.sh
source "$FRAMEWORK_DIR/lib/coordination.sh"
# shellcheck source=lib/cleanup.sh
source "$FRAMEWORK_DIR/lib/cleanup.sh"
# shellcheck source=lib/assertions.sh
source "$FRAMEWORK_DIR/lib/assertions.sh"

if [[ "${FOLDDB_TEST_STANDALONE:-0}" == "1" ]]; then
  run_standalone_smoke "$SCENARIO"
  exit 0
fi

cat <<EOF

==========================================================================
 FoldDB E2E Test Framework
==========================================================================
 run_id       : $RUN_ID
 scenario     : $SCENARIO
 session_dir  : $SESSION_DIR
 dev_api      : $FOLDDB_TEST_DEV_API
 dev_schema   : $FOLDDB_TEST_DEV_SCHEMA
 keep_session : $KEEP_SESSION
==========================================================================

Next step: launch the driver agent with the prompt template at:
  $FRAMEWORK_DIR/driver.md

Substitute placeholders:
  SCENARIO_PATH  = $SCENARIO
  RUN_ID         = $RUN_ID
  SESSION_DIR    = $SESSION_DIR
  DEV_API        = $FOLDDB_TEST_DEV_API
  DEV_SCHEMA     = $FOLDDB_TEST_DEV_SCHEMA
  FRAMEWORK_DIR  = $FRAMEWORK_DIR

EOF
