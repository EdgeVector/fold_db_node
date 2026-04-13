#!/usr/bin/env bash
# E2E test framework entry point.
#
# Runs an E2E scenario against the real dev Exemem cloud:
#   1. Spawns N folddb_server processes (one per role), each with a unique
#      Ed25519 identity.
#   2. Registers each with Exemem via an invite code minted in DynamoDB.
#   3. Executes scenario steps sequentially (step_executor.sh dispatches
#      YAML actions to inlined HTTP calls).
#   4. Runs assertions from the scenario against the live nodes.
#   5. Tears down: deletes Exemem accounts, invalidates invite codes,
#      clears bulletin-board messages for each node's pseudonyms, kills
#      local processes.
#
# Execution is sequential — one node at a time, one action at a time. There
# is no parallelism. If you want to parallelize roles within a step, that's
# a future project and needs real design work (not the abandoned Claude
# sub-agent scaffolding that used to live in this directory).
set -euo pipefail

FRAMEWORK_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

usage() {
  cat <<EOF
Usage: run-scenario.sh <scenario.yaml> [--run-id ID] [--keep-session] [--dry-run]

Options:
  --run-id ID       Use a specific run id (default: generated)
  --keep-session    Do not tear down cloud state (accounts, invite codes,
                    messages) or kill local node processes on exit. Useful
                    when debugging a failed run — you can inspect Aurora,
                    DynamoDB, and node logs after the scenario completes.
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

# Sequentially: spawn nodes, register each, execute the scenario steps in
# order, run assertions, tear down. Each step's actions run serially for each
# of its roles (no parallelism within or across steps). Dependencies are
# implicit in YAML step order — `depends_on` is accepted for readability but
# not enforced because steps never run out of order.
run_scenario() {
  local scenario="$1"
  local nodes_json="$SESSION_DIR/state/nodes.json"
  echo "[run] running $scenario"

  # Parse node roles from the scenario.
  local roles
  if command -v yq >/dev/null 2>&1; then
    roles="$(yq -r '.nodes[].role' "$scenario")"
  else
    roles="$(python3 -c "import yaml,sys; [print(n['role']) for n in yaml.safe_load(open('$scenario'))['nodes']]")"
  fi
  [[ -n "$roles" ]] || { echo "[run] no nodes in scenario" >&2; return 2; }

  local port=40000
  echo "[]" > "$nodes_json"
  # On exit: if the user asked to keep the session, skip teardown entirely so
  # they can poke at cloud state + local logs post-mortem. Otherwise tear
  # down everything (cloud + local). Previously --keep-session was silently
  # a no-op because the trap always ran cleanup_all.
  if [[ "$KEEP_SESSION" == "1" ]]; then
    trap 'echo "[run] --keep-session: skipping teardown. Inspect state at $SESSION_DIR and dev cloud (Aurora + DynamoDB) manually."' EXIT
  else
    local trapcmd="cleanup_all '$nodes_json' || true"
    trap "$trapcmd" EXIT
  fi

  while read -r role; do
    [[ -n "$role" ]] || continue
    echo "[run] --- node: $role (port $port) ---"
    local invite
    invite="$(nf_create_invite_codes 1)"
    echo "[run] invite: $invite"
    nf_spawn_node "$role" "$port" "$SESSION_DIR" >/dev/null
    nf_wait_healthy "$port" 30
    local reg_resp
    reg_resp="$(nf_register_node "$port" "$invite" "$role" "$SESSION_DIR")"
    echo "[run] register resp: $reg_resp"
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
    # Set display name — required for accept_request (handler rejects w/o identity card).
    # Capitalize first letter of role as the display name.
    local display_name
    display_name="$(echo "$role" | awk '{ printf "%s%s", toupper(substr($0,1,1)), tolower(substr($0,2)) }')"
    nf_set_display_name "$port" "$hash" "$display_name"
    echo "[run] set display_name=$display_name"

    jq --arg role "$role" --argjson port "$port" \
       --arg hash "$hash" --arg api_key "$api_key" \
       --arg invite "$invite" --arg public_key "$public_key" \
       --arg display_name "$display_name" \
       '. + [{role:$role, port:$port, hash:$hash, api_key:$api_key, invite_code:$invite, public_key:$public_key, display_name:$display_name}]' \
       "$nodes_json" > "$nodes_json.tmp" && mv "$nodes_json.tmp" "$nodes_json"
    port=$((port + 1))
  done <<< "$roles"

  echo "[run] nodes.json:"
  cat "$nodes_json"

  # Execute scenario steps
  echo ""
  echo "[run] =========================================="
  echo "[run]  executing scenario steps"
  echo "[run] =========================================="
  local steps_ok=1
  if run_steps "$nodes_json" "$scenario" "$FRAMEWORK_DIR"; then
    echo "[run] steps complete"
  else
    steps_ok=0
    echo "[run] STEPS FAILED" >&2
  fi

  # Run assertions
  echo ""
  echo "[run] =========================================="
  echo "[run]  running assertions"
  echo "[run] =========================================="
  local asserts_ok=1
  if ! run_assertions "$nodes_json" "$scenario"; then
    asserts_ok=0
  fi

  if [[ "$steps_ok" == "1" && "$asserts_ok" == "1" ]]; then
    echo ""
    echo "[run] ✅ SCENARIO PASSED"
  else
    echo ""
    echo "[run] ❌ SCENARIO FAILED (steps_ok=$steps_ok asserts_ok=$asserts_ok)" >&2
  fi
  echo "[run] teardown..."
  [[ "$steps_ok" == "1" && "$asserts_ok" == "1" ]] || return 1
}

# shellcheck source=lib/node_factory.sh
source "$FRAMEWORK_DIR/lib/node_factory.sh"
# shellcheck source=lib/cleanup.sh
source "$FRAMEWORK_DIR/lib/cleanup.sh"
# shellcheck source=lib/assertions.sh
source "$FRAMEWORK_DIR/lib/assertions.sh"
# shellcheck source=lib/step_executor.sh
source "$FRAMEWORK_DIR/lib/step_executor.sh"

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

EOF

run_scenario "$SCENARIO"
