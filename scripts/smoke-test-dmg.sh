#!/usr/bin/env bash
# Smoke-test a FoldDB Tauri DMG end-to-end without touching real user data.
#
# Usage:
#   scripts/smoke-test-dmg.sh <path-to-dmg>
#
# What it does:
#   1. Mounts the DMG, copies FoldDB.app to a temp dir, unmounts.
#   2. Launches the app with HOME redirected to an isolated sandbox so the
#      embedded server uses a fresh ~/.folddb under the sandbox.
#   3. Waits for the embedded server to bind 127.0.0.1:9001.
#   4. Verifies GET / returns the React bundle and GET /schemas triggers a
#      successful lazy DB init (200 JSON).
#   5. Confirms the app is still alive after a few seconds (catches
#      "launches, crashes shortly after").
#
# Exit codes: 0 on success, non-zero on any failure.
#
# Intended to run locally before publishing a release AND as a CI job in
# tauri-release.yml so a broken DMG never reaches a GitHub Release.

set -euo pipefail

PORT=9001
APP_PID=""
MOUNT_POINT=""
SMOKE_DIR=$(mktemp -d "/tmp/folddb-smoketest-XXXXXX")

fail() { echo "FAIL: $*" >&2; exit 1; }
log()  { echo "[smoke] $*"; }

cleanup() {
  local rc=$?
  if [ -n "$APP_PID" ]; then
    # Kill child processes (tokio runtime, sidecars) before the parent so the
    # server thread doesn't outlive the bash-tracked pid.
    pkill -P "$APP_PID" 2>/dev/null || true
    if kill -0 "$APP_PID" 2>/dev/null; then
      log "killing app pid=$APP_PID"
      kill "$APP_PID" 2>/dev/null || true
      sleep 1
      kill -9 "$APP_PID" 2>/dev/null || true
    fi
  fi
  # Catch any orphaned fold-app/folddb_mcp launched from our unique sandbox
  # path (safe: $SMOKE_DIR is a mktemp path unique to this run).
  pkill -f "$SMOKE_DIR" 2>/dev/null || true
  sleep 0.5
  # Final sweep: if something from our binary is still bound to port 9001,
  # force-kill it. Only our fold-app could be there since we verified the
  # port was free before launching.
  for pid in $(lsof -tiTCP:$PORT -sTCP:LISTEN 2>/dev/null); do
    if ps -o command= -p "$pid" 2>/dev/null | grep -q "$SMOKE_DIR"; then
      kill -9 "$pid" 2>/dev/null || true
    fi
  done
  if [ -n "$MOUNT_POINT" ] && [ -d "$MOUNT_POINT" ]; then
    log "unmounting $MOUNT_POINT"
    hdiutil detach "$MOUNT_POINT" -force >/dev/null 2>&1 || true
  fi
  if [ -n "${SMOKE_DIR:-}" ] && [ "$rc" -ne 0 ] && [ -f "$SMOKE_DIR/app.log" ]; then
    echo "------ app.log (tail) ------" >&2
    tail -60 "$SMOKE_DIR/app.log" >&2 || true
    echo "----------------------------" >&2
  fi
  rm -rf "$SMOKE_DIR" 2>/dev/null || true
}
trap cleanup EXIT

DMG_PATH="${1:-}"
[ -n "$DMG_PATH" ] || fail "usage: $0 <path-to-dmg>"
[ -f "$DMG_PATH" ] || fail "DMG not found: $DMG_PATH"
command -v hdiutil >/dev/null || fail "hdiutil not found (macOS only)"

# --- 1. mount ---
log "mounting $DMG_PATH"
ATTACH_OUT=$(hdiutil attach "$DMG_PATH" -nobrowse -noverify -noautoopen -plist)
# Parse the mount point from the plist output (last volume path entry).
MOUNT_POINT=$(echo "$ATTACH_OUT" | tr -d '\t' | grep -A1 '<key>mount-point</key>' | tail -1 | sed -E 's/.*<string>(.*)<\/string>.*/\1/')
[ -d "$MOUNT_POINT" ] || fail "failed to parse mount point from hdiutil output"
log "mounted at $MOUNT_POINT"

# --- 2. copy .app out so we can unmount ---
APP_IN_DMG=$(find "$MOUNT_POINT" -maxdepth 2 -name "*.app" | head -1)
[ -d "$APP_IN_DMG" ] || fail "no .app found in DMG"
cp -R "$APP_IN_DMG" "$SMOKE_DIR/"
APP_COPY="$SMOKE_DIR/$(basename "$APP_IN_DMG")"
log "copied to $APP_COPY"

# --- 3. find main binary (excluding sidecars) ---
# Tauri main binary is the one matching the bundle name, not folddb_mcp.
BUNDLE_NAME=$(basename "$APP_COPY" .app)
BINARY=""
for candidate in "$APP_COPY/Contents/MacOS/$BUNDLE_NAME" "$APP_COPY/Contents/MacOS/fold-app" "$APP_COPY/Contents/MacOS/FoldDB"; do
  [ -x "$candidate" ] && { BINARY="$candidate"; break; }
done
[ -n "$BINARY" ] || fail "main binary not found in $APP_COPY/Contents/MacOS/"
log "main binary: $BINARY"

# --- 4. make sure port 9001 is free before we start ---
if lsof -iTCP:$PORT -sTCP:LISTEN >/dev/null 2>&1; then
  fail "port $PORT is already in use — close anything listening on 127.0.0.1:$PORT and try again"
fi

# --- 5. seed a legacy Exemem-style node_config.json so this test exercises
#        the `"type": "exemem"` deserialization path in fold_db/storage/config.rs.
#        Without this, the config defaults to cloud_sync=None and the startup
#        logic in lib.rs would silently override the path to absolute — masking
#        bugs that only appear for users who actually have cloud sync enabled
#        (which is most real users).
mkdir -p "$SMOKE_DIR/home/.folddb"
cat >"$SMOKE_DIR/home/.folddb/node_config.json" <<'JSON'
{
  "database": {
    "type": "exemem",
    "api_url": "https://invalid.smoketest.example",
    "api_key": "smoketest-dummy-key",
    "session_token": null,
    "user_hash": null
  },
  "storage_path": null,
  "network_listen_address": "/ip4/0.0.0.0/tcp/0",
  "security_config": { "require_tls": true },
  "schema_service_url": "https://invalid.smoketest.example/schema"
}
JSON

# --- 6. launch with isolated HOME and CWD=/ to mirror how macOS LaunchServices
#        runs a .app when you double-click it from Finder. Running from a shell
#        would inherit the shell's (usually writable) CWD, which masks bugs
#        where the app opens sled with a relative path that only breaks
#        under the real GUI-launch CWD.
log "launching (HOME=$SMOKE_DIR/home, CWD=/)"
(cd / && HOME="$SMOKE_DIR/home" "$BINARY" >"$SMOKE_DIR/app.log" 2>&1) &
APP_PID=$!

# --- 6. wait for the server to bind ---
log "waiting for 127.0.0.1:$PORT..."
for _ in $(seq 1 60); do
  if curl -s -o /dev/null -w "%{http_code}" "http://127.0.0.1:$PORT/" 2>/dev/null | grep -q 200; then
    log "server is up"
    break
  fi
  kill -0 "$APP_PID" 2>/dev/null || fail "app died before binding to port $PORT"
  sleep 0.5
done
curl -s -o /dev/null -w "%{http_code}" "http://127.0.0.1:$PORT/" 2>/dev/null | grep -q 200 \
  || fail "server never bound to port $PORT within 30s"

# --- 7. verify React bundle is served ---
BODY=$(curl -sf "http://127.0.0.1:$PORT/") || fail "GET / failed"
echo "$BODY" | grep -qiE 'folddb|<script' || fail "GET / body did not look like the React shell"
log "GET / OK (React shell served)"

# --- 8. verify lazy DB init works. /schemas requires a FoldNode to exist,
#        so this exercises the full create_fold_db → SledPool::acquire_arc
#        path. Check Content-Type too: when the node fails to initialize
#        the SPA catchall serves the React index.html with 200, hiding
#        the failure unless we check for application/json.
CT=$(curl -sI "http://127.0.0.1:$PORT/schemas" 2>/dev/null | awk -F': ' '/[Cc]ontent-[Tt]ype/ {print $2}' | tr -d '\r\n ' || true)
if [[ "$CT" != application/json* ]]; then
  fail "GET /schemas returned content-type='$CT' (expected application/json — means the node failed to initialize and the SPA catchall served index.html)"
fi
log "GET /schemas OK (JSON, lazy DB init succeeded)"

# --- 9. belt-and-suspenders: scan the app log for known fatal strings.
#        Catches silent errors that don't flip HTTP status codes (like the
#        read-only filesystem bug where /schemas returned 200 HTML because
#        the SPA catchall masked the per-request node-creation failure).
if grep -qE "Read-only file system|could not acquire lock|Failed to open config store|Failed to open sled database after retries" "$SMOKE_DIR/app.log"; then
  log "app.log contained a fatal error string — see log above"
  fail "app log reports a sled/storage initialization failure"
fi
log "app.log clean"

# --- 10. verify app stays alive briefly — catches post-init crashes ---
sleep 5
kill -0 "$APP_PID" 2>/dev/null || fail "app exited shortly after startup"

log "SMOKE TEST PASSED"
exit 0
