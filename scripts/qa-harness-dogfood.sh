#!/usr/bin/env bash
#
# qa-harness-dogfood.sh — alpha-dogfood QA harness (M6 from
# docs/plans/alpha-self-dogfood.md in the exemem-workspace repo).
#
# Runs a deterministic fixture-only exercise of the six alpha ingestion
# surfaces — Apple Notes, Photos, Calendar, Contacts, Reminders, and local
# files — by POSTing fixture schemas directly to the local schema service
# and 50 fixture molecules per source via /api/mutation, then round-tripping
# them via /api/query. No AI backend, no real Apple data, no external
# dependencies. Safe to run alongside other agents' dev servers (auto-slots
# its own backend/schema/vite ports via run.sh).
#
# Also runs an org-sync leg that spins up a second node and attempts to
# observe a molecule written on node A from node B. Today the leg is a
# SCAFFOLD: M1 (OrgSyncEngine, see plan §4) has not landed, so the second
# node just boots, confirms liveness, and records a TODO. Once M1 ships,
# fill in the "org-sync leg" section marked `TODO M1` below.
#
# Usage:
#   ./scripts/qa-harness-dogfood.sh [--no-teardown] [--report-dir <path>]
#                                    [--per-source-count <N>] [--skip-org]
#
# Exit code: 0 = all six sources pass, 1 = any source failed.
#
# CI: wired via .github/workflows/nightly-qa-dogfood.yml.

set -euo pipefail

# ---------------------------------------------------------------------------
# CLI + defaults
# ---------------------------------------------------------------------------

TEARDOWN=true
REPORT_DIR=""
PER_SOURCE_COUNT=50
SKIP_ORG=false

while [ $# -gt 0 ]; do
  case "$1" in
    --no-teardown)        TEARDOWN=false; shift ;;
    --report-dir)         REPORT_DIR="$2"; shift 2 ;;
    --per-source-count)   PER_SOURCE_COUNT="$2"; shift 2 ;;
    --skip-org)           SKIP_ORG=true; shift ;;
    --help|-h)            sed -n '3,30p' "$0"; exit 0 ;;
    *) echo "qa-harness-dogfood: unknown flag: $1" >&2; exit 2 ;;
  esac
done

if ! [[ "$PER_SOURCE_COUNT" =~ ^[0-9]+$ ]] || [ "$PER_SOURCE_COUNT" -lt 1 ]; then
  echo "qa-harness-dogfood: --per-source-count must be a positive integer" >&2
  exit 2
fi

TS="$(date -u +%Y-%m-%d-%H%M%S)"
REPORT_DIR="${REPORT_DIR:-.gstack/qa-reports/dogfood-$TS}"
mkdir -p "$REPORT_DIR"
REPORT_FILE="$REPORT_DIR/report.md"
LOG_FILE="$REPORT_DIR/harness.log"
: > "$REPORT_FILE"
: > "$LOG_FILE"

log() { printf '[qa-dogfood] %s\n' "$*" | tee -a "$LOG_FILE" 1>&2; }

for bin in curl jq; do
  command -v "$bin" >/dev/null 2>&1 || { echo "qa-harness-dogfood: missing required tool: $bin" >&2; exit 2; }
done

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
NODE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
FIXTURES_DIR="$SCRIPT_DIR/qa-fixtures"
cd "$NODE_ROOT"

# ---------------------------------------------------------------------------
# RustEmbed placeholder — match qa-harness.sh behavior
# ---------------------------------------------------------------------------

DIST="src/server/static-react/dist"
if [ ! -f "$DIST/index.html" ]; then
  mkdir -p "$DIST"
  printf '<!DOCTYPE html><html><body>placeholder</body></html>\n' > "$DIST/index.html"
  log "created placeholder $DIST/index.html for RustEmbed"
fi

# ---------------------------------------------------------------------------
# Stack lifecycle helpers
# ---------------------------------------------------------------------------

# bash 3.2 (macOS default) errors on `${arr[@]}` when `set -u` is on and the
# array has never been written to. Initialize with a sentinel and strip it in
# cleanup. See `arr_len`.
STACK_PIDS=()
STACK_HOMES=()
STACK_PORTS=()

arr_len() {
  # Usage: arr_len "${ARR[@]:-}"
  # Works around bash 3.2 unbound-variable on empty arrays with set -u.
  local count=0
  for _ in "$@"; do count=$((count + 1)); done
  printf '%d' "$count"
}

boot_stack() {
  # Boot one ./run.sh --local --local-schema, extract the slot JSON.
  # Emits three lines on stdout: BACKEND_PORT SCHEMA_PORT VITE_PORT FOLDDB_HOME
  local label="$1"
  local run_log="$REPORT_DIR/run-$label.log"

  log "[$label] starting ./run.sh --local --local-schema"
  nohup ./run.sh --local --local-schema > "$run_log" 2>&1 &
  local pid=$!
  STACK_PIDS+=("$pid")
  log "[$label] run.sh pid=$pid"

  local backend_port=""
  local slot_file=""
  for _ in $(seq 1 600); do
    if ! kill -0 "$pid" 2>/dev/null; then
      log "[$label] FAIL: run.sh exited before slot info appeared"
      tail -20 "$run_log" | sed 's/^/    /' | tee -a "$LOG_FILE"
      return 1
    fi
    local line
    line="$(grep -m1 'HTTP Port: ' "$run_log" 2>/dev/null || true)"
    if [ -n "$line" ]; then
      backend_port="${line##*: }"
      slot_file="$HOME/.folddb-slots/$backend_port.json"
      for _ in $(seq 1 10); do
        [ -f "$slot_file" ] && break
        sleep 1
      done
      [ -f "$slot_file" ] && break
      slot_file=""
    fi
    sleep 1
  done
  if [ -z "$slot_file" ]; then
    log "[$label] FAIL: no slot info within 600s"
    return 1
  fi

  local schema_port vite_port folddb_home
  schema_port="$(jq -r '.schema_port' "$slot_file")"
  vite_port="$(jq   -r '.vite_port'   "$slot_file")"
  folddb_home="$(jq -r '.home'        "$slot_file")"
  STACK_HOMES+=("$folddb_home")
  STACK_PORTS+=("$backend_port")

  log "[$label] backend=$backend_port schema=$schema_port vite=$vite_port home=$folddb_home"

  # Wait for backend + vite liveness
  wait_http "http://localhost:$backend_port/api/system/auto-identity" 60 "[$label] backend" || return 1
  wait_http "http://localhost:$vite_port/" 60 "[$label] vite" || return 1

  printf '%s %s %s %s\n' "$backend_port" "$schema_port" "$vite_port" "$folddb_home"
}

wait_http() {
  local url="$1" timeout="${2:-60}" name="${3:-endpoint}"
  for _ in $(seq 1 "$timeout"); do
    if curl -fsS --max-time 1 "$url" >/dev/null 2>&1; then
      log "$name ready: $url"
      return 0
    fi
    sleep 1
  done
  log "FAIL: $name not ready within ${timeout}s: $url"
  return 1
}

cleanup() {
  if [ "$TEARDOWN" = false ]; then
    log "teardown skipped (--no-teardown). stacks still running:"
    if [ "$(arr_len "${STACK_PIDS[@]:-}")" -gt 0 ]; then
      for i in "${!STACK_PIDS[@]}"; do
        log "  pid=${STACK_PIDS[$i]} home=${STACK_HOMES[$i]:-?} backend=${STACK_PORTS[$i]:-?}"
      done
    fi
    return
  fi
  for pid in "${STACK_PIDS[@]:-}"; do
    if kill -0 "$pid" 2>/dev/null; then
      pkill -P "$pid" 2>/dev/null || true
      kill "$pid" 2>/dev/null || true
    fi
  done
  sleep 2
  for pid in "${STACK_PIDS[@]:-}"; do
    if kill -0 "$pid" 2>/dev/null; then
      kill -9 "$pid" 2>/dev/null || true
      pkill -9 -P "$pid" 2>/dev/null || true
    fi
  done
  for home in "${STACK_HOMES[@]:-}"; do
    [ -z "$home" ] && continue
    # only target our own isolated data dirs
    case "$home" in /tmp/folddb-slot-*) pkill -f "FOLDDB_HOME=$home" 2>/dev/null || true ;; esac
    case "$home" in /tmp/folddb-slot-*) rm -rf "$home" 2>/dev/null || true ;; esac
  done
  for port in "${STACK_PORTS[@]:-}"; do
    [ -z "$port" ] && continue
    rm -f "$HOME/.folddb-slots/$port.json" 2>/dev/null || true
  done
}
trap cleanup EXIT

# ---------------------------------------------------------------------------
# HTTP helpers
# ---------------------------------------------------------------------------

api() {
  # api <method> <url> [<json-body>] → echo body, return 0 on 2xx else 1.
  # Sends X-User-Hash from the caller-set $USER_HASH env var when non-empty.
  local method="$1" url="$2" body="${3:-}"
  local tmp; tmp="$(mktemp)"
  local status
  # bash 3.2 on macOS expands "${arr[@]:-}" to a single empty string on an
  # empty array, which curl rejects as a blank argument. Use the
  # `${arr[@]+"${arr[@]}"}` idiom to emit zero args when empty, or pass a
  # `-H X-User-Hash: …` pair when set.
  if [ -n "${USER_HASH:-}" ]; then
    if [ -n "$body" ]; then
      status="$(curl -sS -o "$tmp" -w '%{http_code}' -X "$method" \
        -H 'Content-Type: application/json' \
        -H "X-User-Hash: $USER_HASH" --data "$body" "$url")"
    else
      status="$(curl -sS -o "$tmp" -w '%{http_code}' -X "$method" \
        -H "X-User-Hash: $USER_HASH" "$url")"
    fi
  else
    if [ -n "$body" ]; then
      status="$(curl -sS -o "$tmp" -w '%{http_code}' -X "$method" \
        -H 'Content-Type: application/json' --data "$body" "$url")"
    else
      status="$(curl -sS -o "$tmp" -w '%{http_code}' -X "$method" "$url")"
    fi
  fi
  cat "$tmp"
  rm -f "$tmp"
  [[ "$status" =~ ^2 ]]
}

# Read the auto-identity user_hash off a freshly-booted node.
fetch_user_hash() {
  local backend_port="$1"
  curl -fsS --max-time 5 "http://localhost:$backend_port/api/system/auto-identity" \
    | jq -r '.user_hash'
}

# ---------------------------------------------------------------------------
# Fixture generators (deterministic, one record per $i)
# ---------------------------------------------------------------------------

gen_notes_mutation() {
  local schema="$1" i="$2"
  local ts; ts="$(printf '2026-04-%02dT%02d:%02d:00Z' $((1 + (i % 28))) $((i % 24)) $((i % 60)))"
  jq -cn --arg s "$schema" --arg ts "$ts" --arg i "$i" '{
    type:"mutation",
    schema:$s,
    fields_and_values:{
      title:("Fixture note " + $i),
      body:("This is fixture note body #" + $i + " — deterministic QA content."),
      folder:"QA/Dogfood",
      modified_at:$ts
    },
    key_value:{range:$ts},
    mutation_type:"create"
  }'
}
gen_photos_mutation() {
  local schema="$1" i="$2"
  local ts; ts="$(printf '2026-03-%02dT%02d:%02d:00Z' $((1 + (i % 28))) $((i % 24)) $((i % 60)))"
  jq -cn --arg s "$schema" --arg ts "$ts" --arg i "$i" '{
    type:"mutation",
    schema:$s,
    fields_and_values:{
      photo_id:("QA-PHOTO-" + $i),
      caption:("Fixture photo caption " + $i),
      album:"QA/Dogfood",
      taken_at:$ts
    },
    key_value:{range:$ts},
    mutation_type:"create"
  }'
}
gen_calendar_mutation() {
  local schema="$1" i="$2"
  local start; start="$(printf '2026-05-%02dT%02d:00:00Z' $((1 + (i % 28))) $((9 + (i % 8))))"
  local end;   end="$(printf   '2026-05-%02dT%02d:30:00Z' $((1 + (i % 28))) $((9 + (i % 8))))"
  jq -cn --arg s "$schema" --arg st "$start" --arg e "$end" --arg i "$i" '{
    type:"mutation",
    schema:$s,
    fields_and_values:{
      event_id:("QA-EVT-" + $i),
      title:("Fixture event " + $i),
      location:"QA Conference Room",
      start_at:$st,
      end_at:$e
    },
    key_value:{range:$st},
    mutation_type:"create"
  }'
}
gen_contacts_mutation() {
  local schema="$1" i="$2"
  local name; name="$(printf 'QA Fixture Contact %03d' "$i")"
  jq -cn --arg s "$schema" --arg n "$name" --arg i "$i" '{
    type:"mutation",
    schema:$s,
    fields_and_values:{
      contact_id:("QA-CON-" + $i),
      full_name:$n,
      email:("fixture" + $i + "@example.invalid"),
      phone:("+1-555-01" + ($i | tostring | .[0:2]))
    },
    key_value:{range:$n},
    mutation_type:"create"
  }'
}
gen_reminders_mutation() {
  local schema="$1" i="$2"
  local due; due="$(printf '2026-06-%02dT%02d:00:00Z' $((1 + (i % 28))) $((8 + (i % 12))))"
  local completed="false"
  [ $((i % 3)) -eq 0 ] && completed="true"
  jq -cn --arg s "$schema" --arg d "$due" --arg c "$completed" --arg i "$i" '{
    type:"mutation",
    schema:$s,
    fields_and_values:{
      reminder_id:("QA-REM-" + $i),
      title:("Fixture reminder " + $i),
      list:"QA/Dogfood",
      due_at:$d,
      completed:$c
    },
    key_value:{range:$d},
    mutation_type:"create"
  }'
}
gen_files_mutation() {
  local schema="$1" i="$2"
  local path; path="$(printf '/qa/dogfood/fixture_%03d.txt' "$i")"
  local mtime; mtime="$(printf '2026-02-%02dT12:00:00Z' $((1 + (i % 28))))"
  jq -cn --arg s "$schema" --arg p "$path" --arg m "$mtime" --arg i "$i" '{
    type:"mutation",
    schema:$s,
    fields_and_values:{
      path:$p,
      mime_type:"text/plain",
      size_bytes:(($i | tonumber) * 128 | tostring),
      modified_at:$m,
      summary:("Fixture file summary " + $i)
    },
    key_value:{range:$p},
    mutation_type:"create"
  }'
}

# Source table: name, schema-name, schema-file, generator-fn
SOURCES=(
  "notes:QaDogfoodNote:notes.schema.json:gen_notes_mutation"
  "photos:QaDogfoodPhoto:photos.schema.json:gen_photos_mutation"
  "calendar:QaDogfoodCalendarEvent:calendar.schema.json:gen_calendar_mutation"
  "contacts:QaDogfoodContact:contacts.schema.json:gen_contacts_mutation"
  "reminders:QaDogfoodReminder:reminders.schema.json:gen_reminders_mutation"
  "files:QaDogfoodLocalFile:files.schema.json:gen_files_mutation"
)

# ---------------------------------------------------------------------------
# Per-source exercise
# ---------------------------------------------------------------------------

# Associative array results[source]=PASS|FAIL(reason)
declare -a RESULT_LINES=()

register_schema_on_service() {
  # Echoes the identity-hash name assigned by the schema service on success.
  local schema_port="$1" file="$2" out_file="$3"
  local body
  # Wrap the raw schema JSON into the /api/schemas AddSchemaRequest shape
  body="$(jq -c --slurpfile s "$file" -n '{schema: $s[0], mutation_mappers: {}}')"
  local response
  # Schema service doesn't require X-User-Hash — the registry is global.
  response="$(USER_HASH="" api POST "http://localhost:$schema_port/api/schemas" "$body")" \
    || return 1
  printf '%s' "$response" > "$out_file"
  jq -r '.schema.name' <<<"$response"
}

exercise_source() {
  local backend_port="$1" schema_port="$2" entry="$3"
  local label="${entry%%:*}"; rest="${entry#*:}"
  local descriptive_name="${rest%%:*}"; rest="${rest#*:}"
  local schema_file="${rest%%:*}"
  local gen_fn="${rest##*:}"

  local schema_path="$FIXTURES_DIR/$schema_file"
  if [ ! -f "$schema_path" ]; then
    RESULT_LINES+=("$label|FAIL|missing fixture schema $schema_path")
    return 1
  fi

  log "[$label] register '$descriptive_name' on schema service"
  local schema_name
  if ! schema_name="$(register_schema_on_service "$schema_port" "$schema_path" \
      "$REPORT_DIR/schema-$label.resp.json")"; then
    RESULT_LINES+=("$label|FAIL|register schema failed")
    return 1
  fi
  if [ -z "$schema_name" ] || [ "$schema_name" = "null" ]; then
    RESULT_LINES+=("$label|FAIL|schema service returned empty name")
    return 1
  fi
  log "[$label] registered id=$schema_name"

  log "[$label] /api/schemas/load"
  if ! api POST "http://localhost:$backend_port/api/schemas/load" '' \
      > "$REPORT_DIR/load-$label.resp.json"; then
    RESULT_LINES+=("$label|FAIL|/api/schemas/load failed")
    return 1
  fi

  log "[$label] /api/schema/$schema_name/approve"
  if ! api POST "http://localhost:$backend_port/api/schema/$schema_name/approve" '' \
      > "$REPORT_DIR/approve-$label.resp.json"; then
    RESULT_LINES+=("$label|FAIL|/api/schema approve failed")
    return 1
  fi

  log "[$label] writing $PER_SOURCE_COUNT fixture molecules"
  local i mut write_fail=0
  for i in $(seq 1 "$PER_SOURCE_COUNT"); do
    mut="$("$gen_fn" "$schema_name" "$i")"
    if ! api POST "http://localhost:$backend_port/api/mutation" "$mut" \
        >> "$REPORT_DIR/mutation-$label.out" 2>&1; then
      write_fail=$((write_fail + 1))
    fi
    printf '\n' >> "$REPORT_DIR/mutation-$label.out"
  done
  if [ "$write_fail" -gt 0 ]; then
    RESULT_LINES+=("$label|FAIL|$write_fail of $PER_SOURCE_COUNT mutations failed")
    return 1
  fi

  log "[$label] /api/query round-trip"
  local fields_json; fields_json="$(jq -c '.fields' "$schema_path")"
  local q
  q="$(jq -cn --arg s "$schema_name" --argjson f "$fields_json" \
    '{schema_name:$s, fields:$f}')"
  local qresp; qresp="$(api POST "http://localhost:$backend_port/api/query" "$q")" \
    || { RESULT_LINES+=("$label|FAIL|query returned non-2xx"); return 1; }
  printf '%s' "$qresp" > "$REPORT_DIR/query-$label.resp.json"

  # Query response is ApiResponse<QueryResponse> with `results` flattened.
  local returned; returned="$(jq '.results | length' <<<"$qresp" 2>/dev/null || echo 0)"
  if ! [[ "$returned" =~ ^[0-9]+$ ]]; then returned=0; fi
  if [ "$returned" -lt "$PER_SOURCE_COUNT" ]; then
    RESULT_LINES+=("$label|FAIL|query returned $returned of $PER_SOURCE_COUNT expected")
    return 1
  fi

  log "[$label] /api/schema/$schema_name/keys molecule count"
  local keys_resp total
  keys_resp="$(api GET "http://localhost:$backend_port/api/schema/$schema_name/keys?limit=1")" \
    || { RESULT_LINES+=("$label|FAIL|keys endpoint non-2xx"); return 1; }
  printf '%s' "$keys_resp" > "$REPORT_DIR/keys-$label.resp.json"
  total="$(jq -r '.data.total_count // .total_count // 0' <<<"$keys_resp")"
  if ! [[ "$total" =~ ^[0-9]+$ ]]; then total=0; fi
  if [ "$total" -lt "$PER_SOURCE_COUNT" ]; then
    RESULT_LINES+=("$label|FAIL|total_count=$total < expected $PER_SOURCE_COUNT")
    return 1
  fi

  RESULT_LINES+=("$label|PASS|wrote=$PER_SOURCE_COUNT query=$returned molecules=$total")
  log "[$label] PASS wrote=$PER_SOURCE_COUNT query=$returned molecules=$total"
  return 0
}

# ---------------------------------------------------------------------------
# Boot node A + run per-source exercises
# ---------------------------------------------------------------------------

log "=== dogfood harness start ==="
log "fixtures: $FIXTURES_DIR  per-source-count: $PER_SOURCE_COUNT  skip-org: $SKIP_ORG"

A_INFO="$(boot_stack node-a)" || { log "boot_stack node-a failed"; exit 1; }
# shellcheck disable=SC2086
set -- $A_INFO
A_BACKEND=$1; A_SCHEMA=$2; A_VITE=$3; A_HOME=$4
log "node A: backend=$A_BACKEND schema=$A_SCHEMA vite=$A_VITE home=$A_HOME"
USER_HASH="$(fetch_user_hash "$A_BACKEND")"
if [ -z "${USER_HASH:-}" ] || [ "$USER_HASH" = "null" ]; then
  log "FAIL: could not read user_hash from node A"
  exit 1
fi
export USER_HASH
log "node A user_hash=$USER_HASH"

for entry in "${SOURCES[@]}"; do
  exercise_source "$A_BACKEND" "$A_SCHEMA" "$entry" || true
done

# ---------------------------------------------------------------------------
# Org-sync leg (SCAFFOLD; fill in once M1 lands)
# ---------------------------------------------------------------------------

ORG_RESULT="SKIP|org leg skipped (--skip-org)"
if [ "$SKIP_ORG" = false ]; then
  log "=== org-sync leg (SCAFFOLD — TODO M1 OrgSyncEngine) ==="
  if B_INFO="$(boot_stack node-b)"; then
    # shellcheck disable=SC2086
    set -- $B_INFO
    B_BACKEND=$1; B_SCHEMA=$2; B_VITE=$3; B_HOME=$4
    log "node B: backend=$B_BACKEND schema=$B_SCHEMA vite=$B_VITE home=$B_HOME"

    # TODO M1 (docs/plans/alpha-self-dogfood.md §4 M1): once OrgSyncEngine ships,
    # replace this block with:
    #   1. folddb org create on node A → invite bundle
    #   2. folddb org join < bundle on node B
    #   3. tag a schema with org_hash, write molecule on A
    #   4. poll node B for the molecule, assert observed within 2 sync cycles
    #
    # Until then, we only verify node B boots + is live, to keep the harness
    # exercising the multi-node plumbing (separate data dirs, own slot JSON).
    if curl -fsS --max-time 5 "http://localhost:$B_BACKEND/api/system/auto-identity" >/dev/null 2>&1; then
      ORG_RESULT="PENDING|node B live; OrgSyncEngine (M1) not yet landed — see TODO in script"
      log "[org] $ORG_RESULT"
    else
      ORG_RESULT="FAIL|node B backend not reachable after boot"
      log "[org] $ORG_RESULT"
    fi
  else
    ORG_RESULT="FAIL|node B boot failed"
    log "[org] $ORG_RESULT"
  fi
fi

# ---------------------------------------------------------------------------
# Report
# ---------------------------------------------------------------------------

PASS_COUNT=0; FAIL_COUNT=0
for line in "${RESULT_LINES[@]}"; do
  case "$line" in
    *'|PASS|'*) PASS_COUNT=$((PASS_COUNT + 1)) ;;
    *'|FAIL|'*) FAIL_COUNT=$((FAIL_COUNT + 1)) ;;
  esac
done

VERDICT="PASS"
[ "$FAIL_COUNT" -gt 0 ] && VERDICT="FAIL"

{
  printf '# QA dogfood harness report\n\n'
  printf -- '- **Timestamp:** %s\n' "$TS"
  printf -- '- **Per-source count:** %d\n' "$PER_SOURCE_COUNT"
  printf -- '- **Node A:** backend=%s schema=%s vite=%s home=`%s`\n' "$A_BACKEND" "$A_SCHEMA" "$A_VITE" "$A_HOME"
  if [ -n "${B_BACKEND:-}" ]; then
    printf -- '- **Node B:** backend=%s schema=%s vite=%s home=`%s`\n' "$B_BACKEND" "$B_SCHEMA" "$B_VITE" "$B_HOME"
  fi
  printf -- '- **Verdict:** %s (%d pass / %d fail of %d sources)\n\n' "$VERDICT" "$PASS_COUNT" "$FAIL_COUNT" "${#SOURCES[@]}"
  printf '## Per-source results\n\n'
  printf '| Source | Result | Detail |\n'
  printf '|---|---|---|\n'
  for line in "${RESULT_LINES[@]}"; do
    IFS='|' read -r src status detail <<<"$line"
    printf '| %s | %s | %s |\n' "$src" "$status" "$detail"
  done
  printf '\n## Org-sync leg\n\n'
  IFS='|' read -r org_status org_detail <<<"$ORG_RESULT"
  printf -- '- **Status:** %s\n' "$org_status"
  printf -- '- **Detail:** %s\n' "$org_detail"
  printf -- '- **Design:** docs/plans/alpha-self-dogfood.md §4 M1 (workspace repo)\n'
} > "$REPORT_FILE"

log "report: $REPORT_FILE"
log "verdict: $VERDICT ($PASS_COUNT pass / $FAIL_COUNT fail of ${#SOURCES[@]} sources; org leg: ${ORG_RESULT%%|*})"

if [ "$VERDICT" = PASS ]; then
  exit 0
else
  exit 1
fi
