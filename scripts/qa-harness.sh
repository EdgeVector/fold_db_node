#!/usr/bin/env bash
#
# qa-harness.sh — self-contained UI QA runner for fold_db_node.
#
# Starts an isolated dev stack (own backend port, own schema port, own vite
# port, own data dir), runs a browser-based smoke test against each major
# dashboard tab, and tears everything down when it's done. Exits 0 on pass,
# 1 on fail.
#
# Intended callers:
#   - `cron` for automated regression detection
#   - PR hooks for pre-merge UI smoke
#   - Claude Code agents running /qa-only
#
# Design reference: gbrain `projects/qa-harness`.
#
# Usage:
#   ./scripts/qa-harness.sh [--report-dir <dir>] [--no-teardown]
#
#   --report-dir <dir>   Where to write the smoke report + screenshots.
#                        Default: .gstack/qa-reports/harness-<ts>
#   --no-teardown        Leave the stack running after the smoke test
#                        (useful for debugging a failure). Default: tear down.
#
# Environment:
#   BROWSE_BIN           Path to the gstack browse binary. Default:
#                        ~/.claude/skills/gstack/browse/dist/browse
#   BROWSE_PORT          Port the browse server uses. Default: auto-pick
#                        in 9400..=9499.

set -euo pipefail

# -----------------------------------------------------------------------------
# CLI + defaults
# -----------------------------------------------------------------------------

TEARDOWN=true
REPORT_DIR=""
while [ $# -gt 0 ]; do
  case "$1" in
    --no-teardown) TEARDOWN=false; shift ;;
    --report-dir)  REPORT_DIR="$2"; shift 2 ;;
    --help|-h)
      sed -n '3,23p' "$0"; exit 0 ;;
    *) echo "qa-harness: unknown flag: $1" >&2; exit 2 ;;
  esac
done

TS="$(date -u +%Y-%m-%d-%H%M%S)"
REPORT_DIR="${REPORT_DIR:-.gstack/qa-reports/harness-$TS}"
mkdir -p "$REPORT_DIR/screenshots"

BROWSE_BIN="${BROWSE_BIN:-$HOME/.claude/skills/gstack/browse/dist/browse}"
if [ ! -x "$BROWSE_BIN" ]; then
  echo "qa-harness: browse binary not found at $BROWSE_BIN" >&2
  echo "            install via gstack or set BROWSE_BIN=/path/to/browse" >&2
  exit 2
fi

REPORT_FILE="$REPORT_DIR/report.md"
: > "$REPORT_FILE"

log() { printf '[qa-harness] %s\n' "$*" | tee -a "$REPORT_DIR/harness.log" ; }

# -----------------------------------------------------------------------------
# Pick a browse port (auto-slot the harness itself; matches the repo's
# parallel-agent-safety story from PRs #520 / #532)
# -----------------------------------------------------------------------------

if [ -z "${BROWSE_PORT:-}" ]; then
  for candidate in $(seq 9400 9499); do
    if ! lsof -iTCP:"$candidate" -sTCP:LISTEN -t >/dev/null 2>&1; then
      BROWSE_PORT="$candidate"; break
    fi
  done
  if [ -z "${BROWSE_PORT:-}" ]; then
    echo "qa-harness: no free browse port in 9400..=9499" >&2
    exit 2
  fi
fi
export BROWSE_PORT
log "browse port: $BROWSE_PORT"

# -----------------------------------------------------------------------------
# Start the dev stack
# -----------------------------------------------------------------------------

# run.sh needs to be executed from the fold_db_node root
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
NODE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$NODE_ROOT"

# RustEmbed needs a non-empty dist/index.html to compile. Create a placeholder
# if missing so a fresh worktree doesn't hit the "Asset::get not found" build
# error. Real content comes from vite in dev anyway.
DIST="src/server/static-react/dist"
if [ ! -f "$DIST/index.html" ]; then
  mkdir -p "$DIST"
  printf '<!DOCTYPE html><html><body>placeholder</body></html>\n' > "$DIST/index.html"
  log "created placeholder $DIST/index.html for RustEmbed"
fi

log "starting ./run.sh --local --local-schema"
nohup ./run.sh --local --local-schema > "$REPORT_DIR/run.log" 2>&1 &
RUN_SH_PID=$!
log "run.sh pid: $RUN_SH_PID"

# -----------------------------------------------------------------------------
# Teardown
# -----------------------------------------------------------------------------

cleanup() {
  if [ "$TEARDOWN" = false ]; then
    log "teardown skipped (--no-teardown). stack still running; kill pid $RUN_SH_PID to stop."
    return
  fi

  log "tearing down stack (pid $RUN_SH_PID + descendants)..."
  # kill the whole process group
  if kill -0 "$RUN_SH_PID" 2>/dev/null; then
    pkill -P "$RUN_SH_PID" 2>/dev/null || true
    kill "$RUN_SH_PID" 2>/dev/null || true
    # give it a sec, then SIGKILL
    sleep 2
    kill -9 "$RUN_SH_PID" 2>/dev/null || true
    pkill -9 -P "$RUN_SH_PID" 2>/dev/null || true
  fi

  # Kill any stray folddb_server / schema_service / vite children
  # that `run.sh` spawned. Match by their FOLDDB_HOME so we only kill
  # our own stack, not some other agent's.
  if [ -n "${FOLDDB_HOME:-}" ]; then
    pkill -f "FOLDDB_HOME=$FOLDDB_HOME" 2>/dev/null || true
  fi

  # Remove our slot dir if we own it
  if [ -n "${FOLDDB_HOME:-}" ] && [ -d "$FOLDDB_HOME" ]; then
    case "$FOLDDB_HOME" in
      /tmp/folddb-slot-*) rm -rf "$FOLDDB_HOME" 2>/dev/null || true ;;
    esac
  fi
  # Remove our slot JSON
  if [ -n "${BACKEND_PORT:-}" ]; then
    rm -f "$HOME/.folddb-slots/$BACKEND_PORT.json" 2>/dev/null || true
  fi
}
trap cleanup EXIT

# -----------------------------------------------------------------------------
# Wait for the slot JSON + services
# -----------------------------------------------------------------------------

log "waiting for slot info (parsing run.log, cargo builds can take a while)..."
SLOT_FILE=""
# Grep run.log for "HTTP Port: NNNN" (emitted after cargo build + server start).
# run.log is authoritative — no mtime race, no stale-slot-JSON ambiguity.
for i in $(seq 1 600); do
  if ! kill -0 "$RUN_SH_PID" 2>/dev/null; then
    log "FAIL: run.sh exited before slot info appeared"
    log "last 20 lines of run.log:"
    tail -20 "$REPORT_DIR/run.log" | sed 's/^/    /' | tee -a "$REPORT_DIR/harness.log"
    exit 1
  fi
  port_line="$(grep -m1 'HTTP Port: ' "$REPORT_DIR/run.log" 2>/dev/null || true)"
  if [ -n "$port_line" ]; then
    BACKEND_PORT="${port_line##*: }"
    SLOT_FILE="$HOME/.folddb-slots/$BACKEND_PORT.json"
    # wait up to 10s more for the slot JSON to actually be written
    for j in $(seq 1 10); do
      [ -f "$SLOT_FILE" ] && break
      sleep 1
    done
    [ -f "$SLOT_FILE" ] && break
    log "WARN: saw HTTP Port: $BACKEND_PORT in run.log but slot JSON didn't materialize"
    SLOT_FILE=""
  fi
  sleep 1
done
if [ -z "$SLOT_FILE" ]; then
  log "FAIL: no slot info appeared within 600s"
  exit 1
fi

SCHEMA_PORT="$(jq  -r '.schema_port' "$SLOT_FILE")"
VITE_PORT="$(jq    -r '.vite_port'   "$SLOT_FILE")"
FOLDDB_HOME="$(jq  -r '.home'        "$SLOT_FILE")"
export FOLDDB_HOME
log "stack: backend=$BACKEND_PORT schema=$SCHEMA_PORT vite=$VITE_PORT home=$FOLDDB_HOME"

wait_http() {
  local url="$1" timeout="${2:-60}" name="${3:-endpoint}"
  for i in $(seq 1 "$timeout"); do
    if curl -fsS --max-time 1 "$url" >/dev/null 2>&1; then
      log "$name ready: $url"
      return 0
    fi
    sleep 1
  done
  log "FAIL: $name not ready within ${timeout}s: $url"
  return 1
}

wait_http "http://localhost:$BACKEND_PORT/api/system/auto-identity" 60 "backend" || exit 1
wait_http "http://localhost:$VITE_PORT/" 60 "vite" || exit 1

# -----------------------------------------------------------------------------
# Smoke test — walk the main tabs, screenshot each, count console errors
# -----------------------------------------------------------------------------

log "running smoke test..."
APP_URL="http://localhost:$VITE_PORT"

# land on root, click "Skip setup entirely" so we're out of onboarding
"$BROWSE_BIN" goto "$APP_URL/" >/dev/null 2>&1

SKIP_REF="$("$BROWSE_BIN" snapshot -i 2>/dev/null | grep -F '"Skip setup entirely"' | awk '{print $1}' | head -1 || true)"
if [ -n "$SKIP_REF" ]; then
  "$BROWSE_BIN" click "$SKIP_REF" >/dev/null 2>&1 || true
  sleep 1
fi

# tabs to smoke
TABS="agent data-browser query schemas people discovery settings"
FAIL=0
for tab in $TABS; do
  "$BROWSE_BIN" goto "$APP_URL/#$tab" >/dev/null 2>&1 || true
  sleep 1
  "$BROWSE_BIN" screenshot "$REPORT_DIR/screenshots/$tab.png" >/dev/null 2>&1 || true

  # title sanity: document body should not be empty
  body_len="$("$BROWSE_BIN" js "document.body.innerText.length" 2>/dev/null | tail -1 | tr -cd '0-9')"
  if [ -z "$body_len" ] || [ "$body_len" -lt 50 ]; then
    log "FAIL tab=$tab body length=${body_len:-0} (expected >= 50)"
    FAIL=$((FAIL + 1))
    continue
  fi
  log "ok tab=$tab body-length=$body_len"
done

# console error count across whole session
err_count="$("$BROWSE_BIN" console --errors 2>/dev/null | grep -c '\[error\]' || true)"
log "console error total: $err_count"

# -----------------------------------------------------------------------------
# Write the report
# -----------------------------------------------------------------------------

PASS=1
[ "$FAIL" -gt 0 ] && PASS=0
# be tolerant of the known public-key 401 loop — ~9 per session is expected
# until fold_db_node#534 lands. Fail if >20 (indicates something else broken).
[ "$err_count" -gt 20 ] && PASS=0

{
  printf '# QA harness smoke report\n\n'
  printf -- '- **Timestamp:** %s\n' "$TS"
  printf -- '- **Stack:** backend=%s schema=%s vite=%s\n' \
    "$BACKEND_PORT" "$SCHEMA_PORT" "$VITE_PORT"
  printf -- '- **FOLDDB_HOME:** `%s`\n' "$FOLDDB_HOME"
  printf -- '- **Tabs smoked:** %s\n' "$TABS"
  printf -- '- **Tab failures:** %d\n' "$FAIL"
  printf -- '- **Console errors:** %d (expected <=20; known noise from fold_db_node#534 is ~9)\n' "$err_count"
  printf -- '- **Verdict:** %s\n\n' "$( [ "$PASS" = 1 ] && echo PASS || echo FAIL )"
  printf '## Screenshots\n\n'
  for tab in $TABS; do
    printf -- '- [%s](screenshots/%s.png)\n' "$tab" "$tab"
  done
} > "$REPORT_FILE"

log "report: $REPORT_FILE"
if [ "$PASS" = 1 ]; then
  log "PASS"
  exit 0
else
  log "FAIL"
  exit 1
fi
