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
Usage: run-scenario.sh <scenario.yaml> [--run-id ID] [--keep-session] [--dry-run] [--timeout SECONDS]

Options:
  --run-id ID         Use a specific run id (default: generated)
  --keep-session      Do not tear down cloud state (accounts, invite codes,
                      messages) or kill local node processes on exit. Useful
                      when debugging a failed run — you can inspect Aurora,
                      DynamoDB, and node logs after the scenario completes.
  --dry-run           Validate the scenario and exit
  --timeout SECONDS   Hard-kill the scenario after this many seconds. Default
                      1800 (30 min). Prevents a single hung action from
                      hanging the whole run indefinitely.
EOF
  exit 1
}

SCENARIO=""
RUN_ID=""
KEEP_SESSION=0
DRY_RUN=0
TIMEOUT_SECS="${FOLDDB_TEST_TIMEOUT:-1800}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --run-id) RUN_ID="$2"; shift 2 ;;
    --keep-session) KEEP_SESSION=1; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    --timeout) TIMEOUT_SECS="$2"; shift 2 ;;
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

# Validate the scenario against scenarios/schema.json. This is MANDATORY —
# skipping it lets typos in action names (e.g. "polll_requests") pass as
# implicit no-ops, which then surface as cryptic "step passed with no
# effect" failures deep into a cloud run. Fail loudly if ajv or yq is
# missing so the operator installs them once instead of debugging ghost
# steps for an afternoon.
SCHEMA="$FRAMEWORK_DIR/scenarios/schema.json"
if ! command -v ajv >/dev/null 2>&1; then
  cat >&2 <<EOF
[run-scenario] ERROR: ajv is required for scenario validation.
[run-scenario]   install: npm install -g ajv-cli
EOF
  exit 3
fi
if ! command -v yq >/dev/null 2>&1; then
  cat >&2 <<EOF
[run-scenario] ERROR: yq is required to convert scenario YAML for validation.
[run-scenario]   install: brew install yq  (or: pip install yq)
EOF
  exit 3
fi
# ajv-cli infers format from file extension, so the temp file MUST end in
# .json — otherwise ajv tries to YAML-parse a JSON payload and fails with
# "Unexpected token ':'". macOS mktemp's -t prefix appends a random suffix
# that breaks the extension; use a fixed path under SESSION_DIR instead.
TMP_JSON="$SESSION_DIR/state/scenario-validate.json"
trap 'rm -f "$TMP_JSON"' EXIT
yq -o=json '.' "$SCENARIO" > "$TMP_JSON"
if ajv validate -s "$SCHEMA" -d "$TMP_JSON" >/dev/null 2>&1; then
  echo "[run-scenario] scenario valid: $SCENARIO"
else
  echo "[run-scenario] scenario INVALID: $SCENARIO" >&2
  ajv validate -s "$SCHEMA" -d "$TMP_JSON" >&2 || true
  exit 2
fi
rm -f "$TMP_JSON"
trap - EXIT

if [[ "$DRY_RUN" == "1" ]]; then
  echo "[run-scenario] dry-run OK"
  exit 0
fi

# Prevent stale ambient state from a developer shell leaking into test nodes.
# Per-node configs set FOLDDB_HOME explicitly; clear it here as a safety belt.
unset FOLDDB_HOME NODE_CONFIG GSTACK_SERVER_PORT GSTACK_PORT

export FOLDDB_TEST_RUN_ID="$RUN_ID"
export FOLDDB_TEST_SESSION_DIR="$SESSION_DIR"
: > "$SESSION_DIR/state/pending-invites.txt"
# URLs come from environments.json registry. SCRIPT_DIR is set by the
# scenario runner; resolve relative to the repo root.
REPO_ROOT_FOR_REGISTRY="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export FOLDDB_TEST_DEV_API="${FOLDDB_TEST_DEV_API:-$("$REPO_ROOT_FOR_REGISTRY/scripts/get-env-url.sh" dev exemem_api)}"
export FOLDDB_TEST_DEV_SCHEMA="${FOLDDB_TEST_DEV_SCHEMA:-$("$REPO_ROOT_FOR_REGISTRY/scripts/get-env-url.sh" dev schema_service)}"
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

  echo "[]" > "$nodes_json"
  # On exit: if the user asked to keep the session, skip teardown entirely so
  # they can poke at cloud state + local logs post-mortem. Otherwise tear
  # down everything (cloud + local). Previously --keep-session was silently
  # a no-op because the trap always ran cleanup_all.
  #
  # The trap is installed BEFORE any cloud state is created (invite codes,
  # nodes) and fires on EXIT/INT/TERM so SIGINT during spawn still revokes
  # pending invites and kills half-started processes.
  # Build the exit handler. Always kills the global-timeout watchdog (if
  # set) so it doesn't linger after a normal exit. In the default path it
  # also runs cleanup_all to tear down cloud state. --keep-session skips
  # the teardown for post-mortem debugging.
  local kill_watchdog='if [[ -n "${WATCHDOG_PID:-}" ]]; then kill "$WATCHDOG_PID" 2>/dev/null || true; fi'
  if [[ "$KEEP_SESSION" == "1" ]]; then
    trap "$kill_watchdog; echo '[run] --keep-session: skipping teardown. Inspect state at \$SESSION_DIR and dev cloud (Aurora + DynamoDB) manually.'" EXIT
  else
    local trapcmd="$kill_watchdog; cleanup_all '$nodes_json' || true"
    trap "$trapcmd" EXIT INT TERM
  fi

  # Offset port ranges by a per-run value so two concurrent framework runs on
  # the same host cannot collide at the starting point. nf_find_free_port
  # handles individual busy ports via lsof, but its lookup is not atomic —
  # two runs calling it simultaneously from 40000 could both pick 40000 in
  # the window between the check and the bind. Randomising the starting
  # offset makes that collision vanishingly unlikely.
  local port_offset=$(( (RANDOM % 80) * 10 ))
  local next_node_port=$(( 40000 + port_offset ))
  local next_gstack_port=$(( 9400 + port_offset ))
  echo "[run] port_offset=$port_offset (node ${next_node_port}+, gstack ${next_gstack_port}+)"
  while read -r role; do
    [[ -n "$role" ]] || continue
    local port gstack_port
    port="$(nf_find_free_port "$next_node_port")"
    next_node_port=$((port + 1))
    gstack_port="$(nf_find_free_port "$next_gstack_port")"
    next_gstack_port=$((gstack_port + 1))
    echo "[run] --- node: $role (port $port, gstack $gstack_port) ---"
    local invite
    invite="$(nf_create_invite_codes 1)"
    echo "[run] invite: $invite"
    # Record invite + ports in nodes.json immediately so cleanup can find them
    # even if spawn/registration crash halfway through.
    jq --arg role "$role" --argjson port "$port" --argjson gport "$gstack_port" \
       --arg invite "$invite" \
       '. + [{role:$role, port:$port, gstack_port:$gport, invite_code:$invite, hash:"", api_key:"", public_key:"", display_name:""}]' \
       "$nodes_json" > "$nodes_json.tmp" && mv "$nodes_json.tmp" "$nodes_json"
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

    # Update the pre-recorded entry for this role with registration results.
    jq --arg role "$role" \
       --arg hash "$hash" --arg api_key "$api_key" \
       --arg public_key "$public_key" --arg display_name "$display_name" \
       'map(if .role == $role then . + {hash:$hash, api_key:$api_key, public_key:$public_key, display_name:$display_name} else . end)' \
       "$nodes_json" > "$nodes_json.tmp" && mv "$nodes_json.tmp" "$nodes_json"
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
 timeout      : ${TIMEOUT_SECS}s
==========================================================================

EOF

# Global scenario deadline. A background watchdog sends SIGTERM to this
# script's process group if we exceed TIMEOUT_SECS, so a single hung action
# (unresponsive node, stuck poll, Lambda cold start) fails the run instead
# of hanging indefinitely. SIGTERM trips the cleanup_all EXIT trap, so
# cloud state still gets torn down on timeout.
WATCHDOG_PID=""
if [[ "$TIMEOUT_SECS" =~ ^[0-9]+$ ]] && (( TIMEOUT_SECS > 0 )); then
  (
    sleep "$TIMEOUT_SECS"
    echo "[run-scenario] TIMEOUT after ${TIMEOUT_SECS}s — killing scenario" >&2
    kill -TERM $$ 2>/dev/null || true
  ) &
  WATCHDOG_PID=$!
  export WATCHDOG_PID
  # run_scenario installs its own EXIT trap that will kill this pid.
fi

run_scenario "$SCENARIO"
