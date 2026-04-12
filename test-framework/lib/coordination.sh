#!/usr/bin/env bash
# Inter-worker coordination via files in $FOLDDB_TEST_SESSION_DIR/state.
set -euo pipefail

_coord_state_dir() {
  echo "${FOLDDB_TEST_SESSION_DIR:?FOLDDB_TEST_SESSION_DIR unset}/state"
}

coord_mark_done() {
  local step="$1" role="$2"
  local d
  d="$(_coord_state_dir)"
  mkdir -p "$d"
  date +%s > "$d/${step}.${role}.done"
}

coord_mark_failed() {
  local step="$1" role="$2" msg="${3:-failed}"
  local d
  d="$(_coord_state_dir)"
  mkdir -p "$d"
  printf '%s\n' "$msg" > "$d/${step}.${role}.failed"
}

coord_wait_for() {
  local step="$1" role="$2" timeout="${3:-300}"
  local d
  d="$(_coord_state_dir)"
  local deadline=$(( $(date +%s) + timeout ))
  while (( $(date +%s) < deadline )); do
    if [[ -f "$d/${step}.${role}.failed" ]]; then
      echo "dependency ${step}.${role} FAILED" >&2
      return 2
    fi
    if [[ -f "$d/${step}.${role}.done" ]]; then
      return 0
    fi
    sleep 0.5
  done
  echo "timeout waiting for ${step}.${role}" >&2
  return 1
}
