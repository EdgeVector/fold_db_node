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
export FOLDDB_TEST_DEV_API="${FOLDDB_TEST_DEV_API:-https://api-dev.exemem.com}"
export FOLDDB_TEST_DEV_SCHEMA="${FOLDDB_TEST_DEV_SCHEMA:-https://schema-dev.folddb.com}"

# shellcheck source=lib/node_factory.sh
source "$FRAMEWORK_DIR/lib/node_factory.sh"
# shellcheck source=lib/coordination.sh
source "$FRAMEWORK_DIR/lib/coordination.sh"
# shellcheck source=lib/cleanup.sh
source "$FRAMEWORK_DIR/lib/cleanup.sh"
# shellcheck source=lib/assertions.sh
source "$FRAMEWORK_DIR/lib/assertions.sh"

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
