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

REPORT_FILE="$REPORT_DIR/report.md"
: > "$REPORT_FILE"

log() { printf '[qa-harness] %s\n' "$*" | tee -a "$REPORT_DIR/harness.log" ; }

# -----------------------------------------------------------------------------
# Report + teardown state — populated as the harness progresses. write_report()
# consults these so the EXIT trap can always produce a report, even if the
# script aborts before reaching the final verdict.
# -----------------------------------------------------------------------------

PASS=""          # "" = no verdict reached; "1" = PASS; "0" = FAIL
FAIL=0
err_count=""
TABS=""
BACKEND_PORT=""
SCHEMA_PORT=""
VITE_PORT=""
FOLDDB_HOME=""
RUN_SH_PID=""

write_report() {
  local verdict
  case "$PASS" in
    1) verdict="PASS" ;;
    0) verdict="FAIL" ;;
    *) verdict="INCOMPLETE" ;;
  esac
  {
    printf '# QA harness smoke report\n\n'
    printf -- '- **Timestamp:** %s\n' "$TS"
    printf -- '- **Stack:** backend=%s schema=%s vite=%s\n' \
      "${BACKEND_PORT:-?}" "${SCHEMA_PORT:-?}" "${VITE_PORT:-?}"
    printf -- '- **FOLDDB_HOME:** `%s`\n' "${FOLDDB_HOME:-?}"
    printf -- '- **Tabs smoked:** %s\n' "${TABS:-n/a}"
    printf -- '- **Tab failures:** %d\n' "${FAIL:-0}"
    if [ -n "$err_count" ]; then
      printf -- '- **Console errors:** %d (expected <=20; known noise from fold_db_node#534 is ~9)\n' "$err_count"
    else
      printf -- '- **Console errors:** n/a (not measured)\n'
    fi
    printf -- '- **Verdict:** %s\n\n' "$verdict"
    if [ "$verdict" = INCOMPLETE ]; then
      printf 'The harness exited before reaching a verdict. See `harness.log` and `run.log` in this directory.\n\n'
    fi
    if [ -n "$TABS" ]; then
      printf '## Screenshots\n\n'
      for tab in $TABS; do
        printf -- '- [%s](screenshots/%s.png)\n' "$tab" "$tab"
      done
    fi
  } > "$REPORT_FILE"
}

cleanup() {
  if [ "$TEARDOWN" = false ]; then
    if [ -n "$RUN_SH_PID" ]; then
      log "teardown skipped (--no-teardown). stack still running; kill pid $RUN_SH_PID to stop."
    fi
    return
  fi

  if [ -n "$RUN_SH_PID" ]; then
    log "tearing down stack (pid $RUN_SH_PID + descendants)..."
    if kill -0 "$RUN_SH_PID" 2>/dev/null; then
      pkill -P "$RUN_SH_PID" 2>/dev/null || true
      kill "$RUN_SH_PID" 2>/dev/null || true
      sleep 2
      kill -9 "$RUN_SH_PID" 2>/dev/null || true
      pkill -9 -P "$RUN_SH_PID" 2>/dev/null || true
    fi
  fi

  # Kill any stray folddb_server / schema_service / vite children that
  # run.sh spawned. Match by FOLDDB_HOME so we only kill our own stack,
  # not some other agent's.
  if [ -n "$FOLDDB_HOME" ]; then
    pkill -f "FOLDDB_HOME=$FOLDDB_HOME" 2>/dev/null || true
  fi

  if [ -n "$FOLDDB_HOME" ] && [ -d "$FOLDDB_HOME" ]; then
    case "$FOLDDB_HOME" in
      /tmp/folddb-slot-*) rm -rf "$FOLDDB_HOME" 2>/dev/null || true ;;
    esac
  fi
  if [ -n "$BACKEND_PORT" ]; then
    rm -f "$HOME/.folddb-slots/$BACKEND_PORT.json" 2>/dev/null || true
  fi
}

on_exit() {
  write_report
  cleanup
}
trap on_exit EXIT

# -----------------------------------------------------------------------------
# Locate the gstack browse binary and pick a free browse port. Both have to be
# settled before the preflight check, because `browse status` binds the
# configured port to start its daemon — a port collision would otherwise look
# like a browse failure.
# -----------------------------------------------------------------------------

BROWSE_BIN="${BROWSE_BIN:-$HOME/.claude/skills/gstack/browse/dist/browse}"
if [ ! -x "$BROWSE_BIN" ]; then
  log "browse binary not found at $BROWSE_BIN"
  log "install via gstack or set BROWSE_BIN=/path/to/browse"
  exit 2
fi

if [ -z "${BROWSE_PORT:-}" ]; then
  for candidate in $(seq 9400 9499); do
    if ! lsof -iTCP:"$candidate" -sTCP:LISTEN -t >/dev/null 2>&1; then
      BROWSE_PORT="$candidate"; break
    fi
  done
  if [ -z "${BROWSE_PORT:-}" ]; then
    log "no free browse port in 9400..=9499"
    exit 2
  fi
fi
export BROWSE_PORT
log "browse port: $BROWSE_PORT"

# -----------------------------------------------------------------------------
# Preflight the browse binary.
#
# The browse binary bundles playwright-core and expects a matching Chromium
# headless-shell cache under ~/Library/Caches/ms-playwright/. On a fresh
# machine, after a cache clear, or when playwright-core drifts, that cache is
# missing and every browse call fails. Without this preflight those failures
# were silent — `browse goto` with stderr discarded plus set -e dropped the
# script with no useful error, leaving report.md at 0 bytes.
# -----------------------------------------------------------------------------

preflight_browse() {
  local out
  if out="$("$BROWSE_BIN" status 2>&1)"; then
    log "browse preflight: ok"
    return 0
  fi
  printf '%s\n' "$out" | sed 's/^/    /' >> "$REPORT_DIR/harness.log"
  if ! printf '%s' "$out" | grep -qiE 'playwright|chromium|chrome-headless-shell|executable doesn'\''t exist'; then
    log "browse preflight FAILED (non-Playwright error; see harness.log)"
    return 1
  fi
  log "browse preflight: Playwright chromium missing — attempting one install"
  local gstack_dir="$HOME/.claude/skills/gstack"
  if [ ! -d "$gstack_dir" ]; then
    log "browse preflight FAILED: gstack skill dir not found at $gstack_dir"
    return 1
  fi
  log "running: (cd $gstack_dir && npx playwright install chromium) — this can take a minute"
  if ! (cd "$gstack_dir" && npx playwright install chromium) >> "$REPORT_DIR/harness.log" 2>&1; then
    log "browse preflight FAILED: npx playwright install chromium exited nonzero (see harness.log)"
    return 1
  fi
  log "playwright chromium install completed; retrying preflight"
  if out="$("$BROWSE_BIN" status 2>&1)"; then
    log "browse preflight: ok after install"
    return 0
  fi
  printf '%s\n' "$out" | sed 's/^/    /' >> "$REPORT_DIR/harness.log"
  log "browse preflight FAILED even after install (see harness.log)"
  return 1
}

if ! preflight_browse; then
  echo "qa-harness: browse binary failed preflight (see $REPORT_DIR/harness.log)" >&2
  exit 2
fi

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
# Smoke test — walk the main tabs, screenshot each, count console errors.
# Every browse call routes stderr to harness.log so any future silent-browse
# failure leaves a real error message instead of an empty report.
# -----------------------------------------------------------------------------

log "running smoke test..."
APP_URL="http://localhost:$VITE_PORT"
BROWSE_ERR="$REPORT_DIR/harness.log"

# -----------------------------------------------------------------------------
# Dismiss the onboarding wizard.
#
# A fresh FOLDDB_HOME triggers the 6-step onboarding wizard (see
# OnboardingWizard.jsx), which is a full-viewport overlay that intercepts every
# hash route — `/#agent`, `/#query`, etc. all render the wizard, not the tab.
# Previously we tried to click the "Skip setup entirely" button via snapshot
# ref-lookup; that was racy against the React mount and silently no-op'd, so
# every tab smoke reported the wizard's 549-char body length and passed.
#
# Instead we set the persistence flag the wizard writes on completion
# (ONBOARDING_STORAGE_KEY = 'folddb_onboarding_complete') and reload so
# useDatabaseInit re-evaluates with the flag present. This is deterministic
# and independent of wizard UI changes.
# -----------------------------------------------------------------------------

"$BROWSE_BIN" goto "$APP_URL/" >/dev/null 2>>"$BROWSE_ERR"

# Wait up to 15s for the React app to render *something* (wizard or app shell).
# Without this, localStorage.setItem can race the app's first mount and get
# overwritten — or worse, run before the JS bundle attaches.
for i in $(seq 1 15); do
  rendered="$("$BROWSE_BIN" js "document.body.innerText.length > 0" 2>>"$BROWSE_ERR" | tail -1 | tr -d ' ')"
  [ "$rendered" = "true" ] && break
  sleep 1
done

"$BROWSE_BIN" js "localStorage.setItem('folddb_onboarding_complete', '1'); 'ok'" >/dev/null 2>>"$BROWSE_ERR" || true
"$BROWSE_BIN" reload >/dev/null 2>>"$BROWSE_ERR" || true
sleep 2

# Confirm the dismissal stuck. If the wizard is still present here, tab smokes
# below will all see it — fail fast with a clear reason rather than reporting
# 7 identical "ok" tabs.
wizard_still_up="$("$BROWSE_BIN" js "document.body.innerText.includes('Skip setup entirely')" 2>>"$BROWSE_ERR" | tail -1 | tr -d ' ')"
if [ "$wizard_still_up" = "true" ]; then
  log "FAIL: onboarding wizard still showing after localStorage dismissal + reload"
  FAIL=$((FAIL + 1))
fi

TABS="agent data-browser query schemas people discovery settings"
BODY_LENS=""
for tab in $TABS; do
  "$BROWSE_BIN" goto "$APP_URL/#$tab" >/dev/null 2>>"$BROWSE_ERR" || true
  sleep 1
  "$BROWSE_BIN" screenshot "$REPORT_DIR/screenshots/$tab.png" >/dev/null 2>>"$BROWSE_ERR" || true

  # Wizard-intercept guard: if any tab URL still renders the wizard, the
  # smoke is lying about coverage — the tab's code never ran.
  wizard_here="$("$BROWSE_BIN" js "document.body.innerText.includes('Skip setup entirely')" 2>>"$BROWSE_ERR" | tail -1 | tr -d ' ')"
  if [ "$wizard_here" = "true" ]; then
    log "FAIL tab=$tab onboarding wizard intercepted the route"
    FAIL=$((FAIL + 1))
    continue
  fi

  # title sanity: document body should not be empty
  body_len="$("$BROWSE_BIN" js "document.body.innerText.length" 2>>"$BROWSE_ERR" | tail -1 | tr -cd '0-9')"
  if [ -z "$body_len" ] || [ "$body_len" -lt 50 ]; then
    log "FAIL tab=$tab body length=${body_len:-0} (expected >= 50)"
    FAIL=$((FAIL + 1))
    continue
  fi
  BODY_LENS="$BODY_LENS $body_len"
  log "ok tab=$tab body-length=$body_len"
done

# If every tab reported the *exact same* body length, the tabs aren't
# actually rendering different content — some overlay or error boundary is
# swallowing them. This is the specific regression that kept the wizard
# false-positive hidden, so guard against it explicitly.
uniq_lens="$(printf '%s\n' $BODY_LENS | sort -u | wc -l | tr -d ' ')"
tab_count="$(printf '%s\n' $BODY_LENS | wc -l | tr -d ' ')"
if [ "$tab_count" -gt 1 ] && [ "$uniq_lens" = 1 ]; then
  log "FAIL: all $tab_count tabs reported identical body-length ($BODY_LENS) — overlay likely intercepting routes"
  FAIL=$((FAIL + 1))
fi

# console error count across whole session
err_count="$("$BROWSE_BIN" console --errors 2>>"$BROWSE_ERR" | grep -c '\[error\]' || true)"
log "console error total: $err_count"

# -----------------------------------------------------------------------------
# Set verdict — the EXIT trap writes the report from the state we recorded.
# -----------------------------------------------------------------------------

PASS=1
[ "$FAIL" -gt 0 ] && PASS=0
# tolerate the known public-key 401 loop (~9/session) until fold_db_node#534
# lands; fail if >20 (indicates something else broken).
[ "${err_count:-0}" -gt 20 ] && PASS=0

log "report: $REPORT_FILE"
if [ "$PASS" = 1 ]; then
  log "PASS"
  exit 0
else
  log "FAIL"
  exit 1
fi
