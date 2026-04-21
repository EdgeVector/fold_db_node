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
# Per-source content fidelity: each SOURCES entry declares a marker-field
# whose per-record value is unique across the write loop. After the query
# round-trip, the set of marker values returned must exactly equal the set
# written. Mismatches report up to 3 missing + 3 extra marker values so the
# regression is actionable (see expected-markers-<src>.sorted.txt vs
# actual-markers-<src>.sorted.txt under the report dir).
#
# Also runs an org-sync leg that spins up a second node in local mode and
# verifies the multi-node plumbing (separate data dir, slot JSON, key pair).
# The real two-node round-trip assertion lives in the cloud-mode E2E
# framework instead — this harness runs without AWS creds, so it cannot
# reach dev Exemem to exercise OrgSyncEngine. See
# test-framework/scenarios/org-sync-2node.yaml for the acceptance test
# (runs nightly via .github/workflows/e2e-cloud.yml, which has AWS creds).
# As of c1388 that scenario also covers: exact-count (13/14) on both
# nodes (no lucky-run on partial sync) and a reverse-write leg
# (Bob writes → Alice reads) so cross-direction org replay is gated too.
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
STACK_SCHEMA_PORTS=()

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
  #
  # On failure, writes a one-line reason to $REPORT_DIR/boot-fail-$label.txt
  # so the caller (running in a command-substitution subshell) can still
  # surface *why* the boot failed. Without this the caller only sees the
  # non-zero exit and has to guess from the run log — that's exactly the
  # silent-skip trap gap aee77 closed.
  local label="$1"
  local run_log="$REPORT_DIR/run-$label.log"
  local fail_reason_file="$REPORT_DIR/boot-fail-$label.txt"
  : > "$fail_reason_file"

  log "[$label] starting ./run.sh --local --local-schema"
  nohup ./run.sh --local --local-schema > "$run_log" 2>&1 &
  local pid=$!
  STACK_PIDS+=("$pid")
  log "[$label] run.sh pid=$pid"

  local backend_port=""
  local slot_file=""
  for _ in $(seq 1 600); do
    if ! kill -0 "$pid" 2>/dev/null; then
      local tail_line
      tail_line="$(tail -5 "$run_log" 2>/dev/null | tr '\n' ' ' | sed 's/[[:space:]]\+/ /g' | cut -c1-400)"
      log "[$label] FAIL: run.sh exited before slot info appeared"
      tail -20 "$run_log" | sed 's/^/    /' | tee -a "$LOG_FILE"
      printf 'run.sh exited before slot info appeared; tail: %s\n' "$tail_line" > "$fail_reason_file"
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
    printf 'no slot info within 600s (backend_port=%s)\n' "${backend_port:-?}" > "$fail_reason_file"
    return 1
  fi

  local schema_port vite_port folddb_home
  schema_port="$(jq -r '.schema_port' "$slot_file")"
  vite_port="$(jq   -r '.vite_port'   "$slot_file")"
  folddb_home="$(jq -r '.home'        "$slot_file")"
  STACK_HOMES+=("$folddb_home")
  STACK_PORTS+=("$backend_port")
  STACK_SCHEMA_PORTS+=("$schema_port")

  log "[$label] backend=$backend_port schema=$schema_port vite=$vite_port home=$folddb_home"

  # Wait for backend + vite liveness
  if ! wait_http "http://localhost:$backend_port/api/system/auto-identity" 60 "[$label] backend"; then
    printf 'backend not reachable on port %s within 60s\n' "$backend_port" > "$fail_reason_file"
    return 1
  fi
  if ! wait_http "http://localhost:$vite_port/" 60 "[$label] vite"; then
    printf 'vite not reachable on port %s within 60s\n' "$vite_port" > "$fail_reason_file"
    return 1
  fi

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
        log "  pid=${STACK_PIDS[$i]} home=${STACK_HOMES[$i]:-?} backend=${STACK_PORTS[$i]:-?} schema=${STACK_SCHEMA_PORTS[$i]:-?}"
      done
    fi
    return
  fi
  # Pass 1 — graceful: SIGTERM run.sh + its direct children (npm/vite).
  for pid in "${STACK_PIDS[@]:-}"; do
    if kill -0 "$pid" 2>/dev/null; then
      pkill -P "$pid" 2>/dev/null || true
      kill "$pid" 2>/dev/null || true
    fi
  done
  # Pass 2 — orphan binaries: run.sh starts the backend + schema service via
  # `nohup cargo run --bin X -- --port N`. Killing run.sh / cargo does not
  # reliably propagate to the actual `target/.../X --port N` binary, which is
  # then reparented to init. Target by name+port — unambiguous because each
  # slot's port is unique. The `cargo run` command line has `-- --port N`
  # (with the `--` separator), so this pattern only matches the binary.
  for port in "${STACK_PORTS[@]:-}"; do
    [ -z "$port" ] && continue
    pkill -f "folddb_server --port $port" 2>/dev/null || true
  done
  for port in "${STACK_SCHEMA_PORTS[@]:-}"; do
    [ -z "$port" ] && continue
    pkill -f "schema_service --port $port" 2>/dev/null || true
  done
  sleep 2
  # Pass 3 — SIGKILL anything still alive after the grace period.
  for pid in "${STACK_PIDS[@]:-}"; do
    if kill -0 "$pid" 2>/dev/null; then
      kill -9 "$pid" 2>/dev/null || true
      pkill -9 -P "$pid" 2>/dev/null || true
    fi
  done
  for port in "${STACK_PORTS[@]:-}"; do
    [ -z "$port" ] && continue
    pkill -9 -f "folddb_server --port $port" 2>/dev/null || true
  done
  for port in "${STACK_SCHEMA_PORTS[@]:-}"; do
    [ -z "$port" ] && continue
    pkill -9 -f "schema_service --port $port" 2>/dev/null || true
  done
  # Slot data dirs + slot JSON: only touch our own (auto-slot dirs live under
  # /tmp/folddb-slot-*; slot JSON keys are the backend ports we tracked).
  for home in "${STACK_HOMES[@]:-}"; do
    [ -z "$home" ] && continue
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

# Source table: label : schema-name : schema-file : generator-fn : marker-field
#
# marker-field names the per-record identity we set-compare on for content
# fidelity. It must be stable per record and unique across the write loop, so
# that a write of N records yields exactly N distinct marker values. On mismatch
# we report up to 3 missing + 3 extra so regressions are actionable.
SOURCES=(
  "notes:QaDogfoodNote:notes.schema.json:gen_notes_mutation:title"
  "photos:QaDogfoodPhoto:photos.schema.json:gen_photos_mutation:photo_id"
  "calendar:QaDogfoodCalendarEvent:calendar.schema.json:gen_calendar_mutation:event_id"
  "contacts:QaDogfoodContact:contacts.schema.json:gen_contacts_mutation:contact_id"
  "reminders:QaDogfoodReminder:reminders.schema.json:gen_reminders_mutation:reminder_id"
  "files:QaDogfoodLocalFile:files.schema.json:gen_files_mutation:path"
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
  # Phase 0 T0 renamed every schema_service route from /api/* to /v1/*, and
  # T3 now boots the binary from ../schema_service (which only serves /v1/*).
  response="$(USER_HASH="" api POST "http://localhost:$schema_port/v1/schemas" "$body")" \
    || return 1
  printf '%s' "$response" > "$out_file"
  jq -r '.schema.name' <<<"$response"
}

exercise_source() {
  local backend_port="$1" schema_port="$2" entry="$3"
  local label="${entry%%:*}"; rest="${entry#*:}"
  local descriptive_name="${rest%%:*}"; rest="${rest#*:}"
  local schema_file="${rest%%:*}"; rest="${rest#*:}"
  local gen_fn="${rest%%:*}"; rest="${rest#*:}"
  local marker_field="$rest"

  if [ -z "$marker_field" ] || [ "$marker_field" = "$gen_fn" ]; then
    RESULT_LINES+=("$label|FAIL|SOURCES entry missing marker-field: $entry")
    return 1
  fi

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

  log "[$label] writing $PER_SOURCE_COUNT fixture molecules (marker='$marker_field')"
  local i mut write_fail=0 marker_val
  local expected_markers="$REPORT_DIR/expected-markers-$label.txt"
  : > "$expected_markers"
  for i in $(seq 1 "$PER_SOURCE_COUNT"); do
    mut="$("$gen_fn" "$schema_name" "$i")"
    marker_val="$(jq -r --arg f "$marker_field" '.fields_and_values[$f] // empty' <<<"$mut")"
    if [ -z "$marker_val" ]; then
      RESULT_LINES+=("$label|FAIL|generator did not emit marker-field '$marker_field' on i=$i")
      return 1
    fi
    printf '%s\n' "$marker_val" >> "$expected_markers"
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

  # Assert marker-field uniqueness across the write loop — a dupe here means
  # the generator is broken and the later set-equality check would silently
  # under-count expected markers.
  local expected_unique
  expected_unique="$(sort -u "$expected_markers" | wc -l | tr -d ' ')"
  if [ "$expected_unique" -lt "$PER_SOURCE_COUNT" ]; then
    RESULT_LINES+=("$label|FAIL|marker-field '$marker_field' not unique across $PER_SOURCE_COUNT writes (got $expected_unique distinct)")
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
  # Each result record has shape { "key": {"range": ...}, "fields": {<field>: <value>, ...} }.
  local returned; returned="$(jq '.results | length' <<<"$qresp" 2>/dev/null || echo 0)"
  if ! [[ "$returned" =~ ^[0-9]+$ ]]; then returned=0; fi
  if [ "$returned" -lt "$PER_SOURCE_COUNT" ]; then
    RESULT_LINES+=("$label|FAIL|query returned $returned of $PER_SOURCE_COUNT expected")
    return 1
  fi

  # Content-fidelity: the set of marker-field values returned by the query must
  # exactly match the set written. Set equality — query order is not part of
  # the API contract, so an ordered diff would be brittle.
  local actual_markers="$REPORT_DIR/actual-markers-$label.txt"
  jq -r --arg f "$marker_field" '.results[] | .fields[$f] // empty' <<<"$qresp" \
    > "$actual_markers"
  local expected_sorted="$REPORT_DIR/expected-markers-$label.sorted.txt"
  local actual_sorted="$REPORT_DIR/actual-markers-$label.sorted.txt"
  sort -u "$expected_markers" > "$expected_sorted"
  sort -u "$actual_markers"   > "$actual_sorted"
  local missing="$REPORT_DIR/missing-markers-$label.txt"
  local extra="$REPORT_DIR/extra-markers-$label.txt"
  comm -23 "$expected_sorted" "$actual_sorted" > "$missing"
  comm -13 "$expected_sorted" "$actual_sorted" > "$extra"
  local missing_n extra_n
  missing_n="$(awk 'NF' "$missing" | wc -l | tr -d ' ')"
  extra_n="$(awk 'NF' "$extra" | wc -l | tr -d ' ')"
  if [ "$missing_n" -gt 0 ] || [ "$extra_n" -gt 0 ]; then
    local missing_preview extra_preview
    missing_preview="$(awk 'NF' "$missing" | head -n 3 | paste -sd',' -)"
    extra_preview="$(awk 'NF' "$extra" | head -n 3 | paste -sd',' -)"
    RESULT_LINES+=("$label|FAIL|content-fidelity on '$marker_field': missing=$missing_n extra=$extra_n [missing≤3: ${missing_preview:-none}] [extra≤3: ${extra_preview:-none}]")
    log "[$label] FAIL content-fidelity missing=$missing_n extra=$extra_n"
    return 1
  fi
  log "[$label] content-fidelity ok: $PER_SOURCE_COUNT/$PER_SOURCE_COUNT markers on '$marker_field'"

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

  RESULT_LINES+=("$label|PASS|wrote=$PER_SOURCE_COUNT query=$returned molecules=$total fidelity=ok(marker=$marker_field)")
  log "[$label] PASS wrote=$PER_SOURCE_COUNT query=$returned molecules=$total fidelity=ok"
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
# Set-org-hash leg — single-node regression coverage for pre-tag queryability
#
# Gap 249d8 (from alpha dogfood run 4): the per-source loop above exercises
# /api/mutation + /api/query + /api/schema/{name}/keys, but never calls
# POST /api/schema/{name}/set-org-hash. That blind-spot let two run-4 BLOCKERs
# ship un-caught:
#
#   - 3e063: tagging a schema with an org_hash orphans every molecule written
#     under the personal prefix before the tag. Any later query that targets
#     the now-org-tagged field hits "Atom not found for key".
#   - af4ba: the org-tagged schema itself never propagates to peer nodes over
#     the org sync log, so even if the personal-prefix molecules were still
#     queryable locally they would not be resolvable on a peer.
#
# This leg catches 3e063 on a single local node — we don't need AWS creds for
# that: "pre-tag molecules stay queryable after tagging the schema" is a
# local invariant. The cross-node propagation half — af4ba (schema arrives)
# AND 4b171 (pre-tag molecule doesn't orphan peer's unfiltered query) —
# lives in the cloud-e2e nightly (test-framework/scenarios/org-sync-2node.yaml)
# where the scenario spawns two nodes on dev Exemem, writes 1 pre-tag + 10
# post-tag molecules on Alice + 3 reverse-write molecules on Bob, then
# asserts exact counts (13 on Bob, 14 on Alice) after sync. Exact counts
# replace the pre-c1388 ">=1" which a partial replay could satisfy.
# ---------------------------------------------------------------------------

# Pick the "files" source — its marker-field (`path`) is a stable disk path
# so the set-equality check is easy to read in the diff when it fails, and
# the 50 molecules written earlier in the per-source loop are exactly the
# "pre-tag" population we need to probe for orphaning.
SET_ORG_HASH_SOURCE="files"
# Deterministic fake org_hash (64 hex chars, shape-valid for the endpoint —
# set-org-hash does not cross-check the org exists on this node, it just
# writes the tag onto the schema + runtime fields).
SET_ORG_HASH_FAKE="a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1"
SET_ORG_HASH_RESULT="SKIP|set-org-hash leg not run"

exercise_set_org_hash() {
  local backend_port="$1"
  local label="$SET_ORG_HASH_SOURCE"
  local fake_hash="$SET_ORG_HASH_FAKE"

  # Recover the registered schema identity-hash name and field list from the
  # per-source artifacts. If the source itself failed earlier we can't run
  # this leg, so surface that cleanly.
  local schema_resp="$REPORT_DIR/schema-$label.resp.json"
  local fixture_path="$FIXTURES_DIR/$(printf '%s.schema.json' "$label")"
  local expected_sorted="$REPORT_DIR/expected-markers-$label.sorted.txt"
  if [ ! -f "$schema_resp" ] || [ ! -f "$fixture_path" ] || [ ! -f "$expected_sorted" ]; then
    SET_ORG_HASH_RESULT="SKIP|prerequisites missing (source '$label' did not run successfully)"
    log "[set-org-hash] $SET_ORG_HASH_RESULT"
    return 0
  fi

  local schema_name
  schema_name="$(jq -r '.schema.name' "$schema_resp")"
  if [ -z "$schema_name" ] || [ "$schema_name" = "null" ]; then
    SET_ORG_HASH_RESULT="FAIL|could not read schema name from $schema_resp"
    log "[set-org-hash] $SET_ORG_HASH_RESULT"
    return 0
  fi

  local marker_field="path"  # matches SOURCES "files" entry

  log "[set-org-hash] tag $schema_name with org_hash=$fake_hash"
  local tag_resp
  if ! tag_resp="$(api POST \
      "http://localhost:$backend_port/api/schema/$schema_name/set-org-hash" \
      "$(jq -cn --arg h "$fake_hash" '{org_hash:$h}')")"; then
    SET_ORG_HASH_RESULT="FAIL|POST /api/schema/$schema_name/set-org-hash non-2xx: $tag_resp"
    log "[set-org-hash] $SET_ORG_HASH_RESULT"
    return 0
  fi
  printf '%s' "$tag_resp" > "$REPORT_DIR/set-org-hash-tag.resp.json"
  local tagged_hash
  tagged_hash="$(jq -r '.org_hash // .data.org_hash // empty' <<<"$tag_resp")"
  if [ "$tagged_hash" != "$fake_hash" ]; then
    SET_ORG_HASH_RESULT="FAIL|tag response org_hash=$tagged_hash != expected $fake_hash"
    log "[set-org-hash] $SET_ORG_HASH_RESULT"
    return 0
  fi

  # Confirm GET /api/schema/{name} round-trips the new tag through Sled.
  local get_resp
  if ! get_resp="$(api GET "http://localhost:$backend_port/api/schema/$schema_name" '')"; then
    SET_ORG_HASH_RESULT="FAIL|GET /api/schema/$schema_name non-2xx after tag"
    log "[set-org-hash] $SET_ORG_HASH_RESULT"
    return 0
  fi
  printf '%s' "$get_resp" > "$REPORT_DIR/set-org-hash-get.resp.json"
  local persisted_hash
  persisted_hash="$(jq -r '.schema.schema.org_hash // .schema.org_hash // empty' <<<"$get_resp")"
  if [ "$persisted_hash" != "$fake_hash" ]; then
    SET_ORG_HASH_RESULT="FAIL|schema.org_hash after tag=$persisted_hash != $fake_hash (not persisted)"
    log "[set-org-hash] $SET_ORG_HASH_RESULT"
    return 0
  fi

  # ---- 3e063 assertion: pre-tag molecules must stay queryable ------------
  log "[set-org-hash] re-query $schema_name; pre-tag molecules must survive"
  local fields_json; fields_json="$(jq -c '.fields' "$fixture_path")"
  local q
  q="$(jq -cn --arg s "$schema_name" --argjson f "$fields_json" \
    '{schema_name:$s, fields:$f}')"
  local post_tag_qresp
  if ! post_tag_qresp="$(api POST "http://localhost:$backend_port/api/query" "$q")"; then
    SET_ORG_HASH_RESULT="FAIL|3e063: query after tag returned non-2xx (orphaned pre-tag molecules?)"
    log "[set-org-hash] $SET_ORG_HASH_RESULT"
    printf '%s' "$post_tag_qresp" > "$REPORT_DIR/set-org-hash-query-after-tag.resp.json" 2>/dev/null || true
    return 0
  fi
  printf '%s' "$post_tag_qresp" > "$REPORT_DIR/set-org-hash-query-after-tag.resp.json"

  local post_tag_n
  post_tag_n="$(jq '.results | length' <<<"$post_tag_qresp" 2>/dev/null || echo 0)"
  if ! [[ "$post_tag_n" =~ ^[0-9]+$ ]]; then post_tag_n=0; fi
  if [ "$post_tag_n" -lt "$PER_SOURCE_COUNT" ]; then
    SET_ORG_HASH_RESULT="FAIL|3e063: query after tag returned $post_tag_n of $PER_SOURCE_COUNT pre-tag molecules"
    log "[set-org-hash] $SET_ORG_HASH_RESULT"
    return 0
  fi

  local post_tag_markers="$REPORT_DIR/set-org-hash-markers-after-tag.txt"
  local post_tag_sorted="$REPORT_DIR/set-org-hash-markers-after-tag.sorted.txt"
  jq -r --arg f "$marker_field" '.results[] | .fields[$f] // empty' \
    <<<"$post_tag_qresp" > "$post_tag_markers"
  sort -u "$post_tag_markers" > "$post_tag_sorted"
  local missing_after_tag="$REPORT_DIR/set-org-hash-missing-after-tag.txt"
  comm -23 "$expected_sorted" "$post_tag_sorted" > "$missing_after_tag"
  local missing_n
  missing_n="$(awk 'NF' "$missing_after_tag" | wc -l | tr -d ' ')"
  if [ "$missing_n" -gt 0 ]; then
    local missing_preview
    missing_preview="$(awk 'NF' "$missing_after_tag" | head -n 3 | paste -sd',' -)"
    SET_ORG_HASH_RESULT="FAIL|3e063: $missing_n pre-tag molecules orphaned on '$marker_field' after set-org-hash [missing≤3: $missing_preview]"
    log "[set-org-hash] $SET_ORG_HASH_RESULT"
    return 0
  fi
  log "[set-org-hash] 3e063 ok: all $PER_SOURCE_COUNT pre-tag molecules still queryable"

  # ---- clear path: {"org_hash": null} must revert + pre-tag molecules must
  # still be queryable afterwards (a clear that orphans data would be worse
  # than the tag that orphans it).
  log "[set-org-hash] clear $schema_name org_hash"
  local clear_resp
  if ! clear_resp="$(api POST \
      "http://localhost:$backend_port/api/schema/$schema_name/set-org-hash" \
      '{"org_hash":null}')"; then
    SET_ORG_HASH_RESULT="FAIL|POST .../set-org-hash {org_hash:null} non-2xx: $clear_resp"
    log "[set-org-hash] $SET_ORG_HASH_RESULT"
    return 0
  fi
  printf '%s' "$clear_resp" > "$REPORT_DIR/set-org-hash-clear.resp.json"

  local get_after_clear
  if ! get_after_clear="$(api GET "http://localhost:$backend_port/api/schema/$schema_name" '')"; then
    SET_ORG_HASH_RESULT="FAIL|GET /api/schema/$schema_name non-2xx after clear"
    log "[set-org-hash] $SET_ORG_HASH_RESULT"
    return 0
  fi
  printf '%s' "$get_after_clear" > "$REPORT_DIR/set-org-hash-get-after-clear.resp.json"
  local cleared_hash
  cleared_hash="$(jq -r '.schema.schema.org_hash // .schema.org_hash // empty' <<<"$get_after_clear")"
  if [ -n "$cleared_hash" ] && [ "$cleared_hash" != "null" ]; then
    SET_ORG_HASH_RESULT="FAIL|org_hash not cleared after {org_hash:null}: $cleared_hash"
    log "[set-org-hash] $SET_ORG_HASH_RESULT"
    return 0
  fi

  local clear_qresp
  if ! clear_qresp="$(api POST "http://localhost:$backend_port/api/query" "$q")"; then
    SET_ORG_HASH_RESULT="FAIL|query after clear returned non-2xx"
    log "[set-org-hash] $SET_ORG_HASH_RESULT"
    return 0
  fi
  printf '%s' "$clear_qresp" > "$REPORT_DIR/set-org-hash-query-after-clear.resp.json"
  local clear_n
  clear_n="$(jq '.results | length' <<<"$clear_qresp" 2>/dev/null || echo 0)"
  if ! [[ "$clear_n" =~ ^[0-9]+$ ]]; then clear_n=0; fi
  if [ "$clear_n" -lt "$PER_SOURCE_COUNT" ]; then
    SET_ORG_HASH_RESULT="FAIL|query after clear returned $clear_n of $PER_SOURCE_COUNT pre-tag molecules"
    log "[set-org-hash] $SET_ORG_HASH_RESULT"
    return 0
  fi

  SET_ORG_HASH_RESULT="PASS|source=$label schema=$schema_name tagged+queryable=$post_tag_n cleared+queryable=$clear_n (3e063 guard)"
  log "[set-org-hash] PASS"
  return 0
}

exercise_set_org_hash "$A_BACKEND"

# ---------------------------------------------------------------------------
# Org-sync leg — local plumbing check only
#
# This harness runs without AWS credentials, so it cannot reach dev Exemem
# to exercise OrgSyncEngine end-to-end. The real two-node round-trip now
# lives in test-framework/scenarios/org-sync-2node.yaml, wired into nightly
# .github/workflows/e2e-cloud.yml (which has AWS creds). Here we only
# confirm a second node boots with its own slot JSON + data dir, keeping
# the multi-node plumbing under test in this harness too.
# ---------------------------------------------------------------------------

ORG_RESULT="SKIP|org leg skipped (--skip-org)"
if [ "$SKIP_ORG" = false ]; then
  log "=== org-sync leg (local plumbing only; real assertion: e2e-cloud nightly) ==="
  if B_INFO="$(boot_stack node-b)"; then
    # shellcheck disable=SC2086
    set -- $B_INFO
    B_BACKEND=$1; B_SCHEMA=$2; B_VITE=$3; B_HOME=$4
    log "node B: backend=$B_BACKEND schema=$B_SCHEMA vite=$B_VITE home=$B_HOME"

    if curl -fsS --max-time 5 "http://localhost:$B_BACKEND/api/system/auto-identity" >/dev/null 2>&1; then
      ORG_RESULT="PENDING|node B live; real two-node org round-trip (4b171 + af4ba + 500b9 + c1388 exact-count + reverse-sync) runs in test-framework/scenarios/org-sync-2node.yaml via nightly e2e-cloud (has AWS creds)"
      log "[org] $ORG_RESULT"
    else
      ORG_RESULT="FAIL|node B backend not reachable after boot"
      log "[org] $ORG_RESULT"
    fi
  else
    # boot_stack dropped a one-line reason file (see boot_stack docstring).
    # Surface it so the report shows WHY node B failed to boot instead of a
    # bare "node B boot failed" — that was the aee77 silent-skip trap.
    B_BOOT_REASON=""
    if [ -f "$REPORT_DIR/boot-fail-node-b.txt" ]; then
      B_BOOT_REASON="$(tr '\n' ' ' < "$REPORT_DIR/boot-fail-node-b.txt" | sed 's/[[:space:]]\+/ /g' | sed 's/^ *//;s/ *$//')"
    fi
    if [ -z "$B_BOOT_REASON" ]; then
      B_BOOT_REASON="no reason captured (see $REPORT_DIR/run-node-b.log)"
    fi
    ORG_RESULT="FAIL|node B boot failed: $B_BOOT_REASON"
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
# A failing set-org-hash leg is a verdict-breaking regression too (249d8 was
# filed precisely so 3e063-class regressions aren't silent).
case "$SET_ORG_HASH_RESULT" in
  FAIL\|*) VERDICT="FAIL" ;;
esac
# A failing org-sync leg (e.g. node B boot failure) must flip the verdict
# too. Before aee77 this was a silent skip: the harness exited 0 even though
# the org leg could not run, masking a regression in the multi-node boot
# path. The real two-node round-trip still runs in the cloud-e2e nightly;
# this local plumbing check is the gate for "can a second node even boot
# from the same run.sh entrypoint?". PENDING is not a failure — the real
# assertion lives elsewhere; SKIP (--skip-org) is not a failure either.
case "$ORG_RESULT" in
  FAIL\|*) VERDICT="FAIL" ;;
esac

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
  printf '\n## Set-org-hash leg (single-node; 3e063 guard)\n\n'
  IFS='|' read -r soh_status soh_detail <<<"$SET_ORG_HASH_RESULT"
  printf -- '- **Status:** %s\n' "$soh_status"
  printf -- '- **Detail:** %s\n' "$soh_detail"
  printf -- '- **Scope:** tags the `%s` schema, asserts pre-tag molecules stay queryable, then clears the tag and re-asserts. Cross-node propagation (af4ba) lives in `test-framework/scenarios/org-sync-2node.yaml` (cloud e2e nightly).\n' "$SET_ORG_HASH_SOURCE"
  printf '\n## Org-sync leg\n\n'
  IFS='|' read -r org_status org_detail <<<"$ORG_RESULT"
  printf -- '- **Status:** %s\n' "$org_status"
  printf -- '- **Detail:** %s\n' "$org_detail"
  printf -- '- **Design:** docs/plans/alpha-self-dogfood.md §4 M1 (workspace repo)\n'
} > "$REPORT_FILE"

log "report: $REPORT_FILE"
log "verdict: $VERDICT ($PASS_COUNT pass / $FAIL_COUNT fail of ${#SOURCES[@]} sources; set-org-hash: ${SET_ORG_HASH_RESULT%%|*}; org leg: ${ORG_RESULT%%|*})"

if [ "$VERDICT" = PASS ]; then
  exit 0
else
  exit 1
fi
