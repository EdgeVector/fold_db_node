#!/usr/bin/env bash
#
# org-test.sh — Spin up two isolated FoldDB nodes in Exemem mode for org sharing testing.
#
# Usage:
#   ./org-test.sh <invite_code>    # Start both nodes, register with the given invite code
#   ./org-test.sh --local          # Start both nodes in local-only mode (org ops will fail)
#   ./org-test.sh stop             # Stop both nodes and clean up
#
# Both nodes get fresh identities and register with the provided invite code.
# The invite code must allow at least 2 uses (create 2 codes if single-use).
# Each node gets its own FOLDDB_HOME, identity, ports, and data directory.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

SESSION_DIR="/tmp/folddb-org-test"
NUM_NODES=2
LOCAL_MODE=false
INVITE_CODES=()
EXEMEM_API_URL="https://ygyu7ritx8.execute-api.us-west-2.amazonaws.com"

# ── Stop command ──────────────────────────────────────────────────────────────
if [ "${1:-}" = "stop" ]; then
    echo "Stopping org-test session..."
    for pidfile in "$SESSION_DIR"/node-*/*.pid; do
        if [ -f "$pidfile" ]; then
            pid=$(cat "$pidfile")
            kill "$pid" 2>/dev/null && echo "  Killed PID $pid" || true
        fi
    done
    rm -rf "$SESSION_DIR"
    echo "Cleaned up $SESSION_DIR"
    exit 0
fi

# ── Parse args ────────────────────────────────────────────────────────────────
for arg in "$@"; do
    case "$arg" in
        --local) LOCAL_MODE=true ;;
        -*) echo "Unknown option: $arg"; exit 1 ;;
        *) INVITE_CODES+=("$arg") ;;
    esac
done

if [ "$LOCAL_MODE" = false ] && [ "${#INVITE_CODES[@]}" -lt 2 ]; then
    echo "Usage: ./org-test.sh <invite_code_1> <invite_code_2>"
    echo "       ./org-test.sh --local"
    echo "       ./org-test.sh stop"
    echo ""
    echo "Two invite codes are required (one per node, single-use)."
    echo "Create them in the DynamoDB ExememInviteCodes table or via the API."
    exit 1
fi

# ── Resolve API key for Exemem mode ──────────────────────────────────────────
if [ "$LOCAL_MODE" = false ]; then
    if [ -z "${EXEMEM_API_KEY:-}" ]; then
        CREDS_FILE="$HOME/.folddb/credentials.json"
        if [ -f "$CREDS_FILE" ]; then
            EXEMEM_API_KEY=$(python3 -c "import json; print(json.load(open('$CREDS_FILE'))['api_key'])" 2>/dev/null || echo "")
        fi
    fi
    if [ -z "${EXEMEM_API_KEY:-}" ]; then
        echo "ERROR: No API key found. Set EXEMEM_API_KEY or put it in ~/.folddb/credentials.json"
        exit 1
    fi
    echo "Mode: Exemem (cloud sync enabled — org sharing will work)"
    echo "API:  $EXEMEM_API_URL"
else
    echo "Mode: Local (org operations will NOT work — no cloud connectivity)"
fi
echo ""

# ── Clean up any previous session ─────────────────────────────────────────────
if [ -d "$SESSION_DIR" ]; then
    echo "Cleaning up previous session..."
    for pidfile in "$SESSION_DIR"/node-*/*.pid; do
        [ -f "$pidfile" ] && kill "$(cat "$pidfile")" 2>/dev/null || true
    done
    rm -rf "$SESSION_DIR"
fi
mkdir -p "$SESSION_DIR"

# ── Find free ports ───────────────────────────────────────────────────────────
allocate_ports() {
    local count=$1
    local base=$((RANDOM % 30000 + 10000))
    local found=0
    local ports=()
    for p in $(seq "$base" $((base + 200))); do
        if ! lsof -i :"$p" >/dev/null 2>&1; then
            ports+=("$p")
            found=$((found + 1))
            [ "$found" -eq "$count" ] && break
        fi
    done
    echo "${ports[@]}"
}

PORTS=($(allocate_ports $((NUM_NODES * 2))))
# Save ports for restart
echo "${PORTS[@]}" > "$SESSION_DIR/ports"

# ── Build once ────────────────────────────────────────────────────────────────
echo "Building binaries..."
cargo build --bin folddb_server --bin schema_service --bin ensure_identity 2>&1 | tail -3
BINARY="$SCRIPT_DIR/target/debug"
echo ""

# ── Helper: start a node ─────────────────────────────────────────────────────
start_node() {
    local n=$1
    local BACKEND_PORT="${PORTS[$((n * 2 - 2))]}"
    local SCHEMA_PORT="${PORTS[$((n * 2 - 1))]}"
    local NODE_HOME="${SESSION_DIR}/node-${n}"

    # Start schema service
    AI_PROVIDER=anthropic "$BINARY/schema_service" \
        --port "$SCHEMA_PORT" \
        --db-path "$NODE_HOME/schema_registry" \
        &>"$NODE_HOME/schema_service.log" &
    echo $! > "$NODE_HOME/schema.pid"

    # Start folddb_server
    FOLDDB_HOME="$NODE_HOME" \
    DISCOVERY_SERVICE_URL="${EXEMEM_API_URL}/api" \
    RUST_LOG=info \
        "$BINARY/folddb_server" \
        --port "$BACKEND_PORT" \
        --schema-service-url "http://127.0.0.1:$SCHEMA_PORT" \
        &>"$NODE_HOME/server.log" &
    echo $! > "$NODE_HOME/folddb.pid"

    # Health check
    echo -n "  Waiting for server..."
    for i in $(seq 1 45); do
        PID=$(cat "$NODE_HOME/folddb.pid")
        if ! kill -0 "$PID" 2>/dev/null; then
            echo " DIED! Check $NODE_HOME/server.log"
            tail -5 "$NODE_HOME/server.log" 2>/dev/null
            return 1
        fi
        if curl -s "http://localhost:$BACKEND_PORT/api/system/auto-identity" 2>/dev/null | grep -q user_hash; then
            echo " ready!"
            return 0
        fi
        [ "$i" -eq 45 ] && echo " TIMEOUT!" && return 1
        sleep 1
    done
}

stop_node() {
    local n=$1
    local NODE_HOME="${SESSION_DIR}/node-${n}"
    for pidfile in "$NODE_HOME"/*.pid; do
        [ -f "$pidfile" ] && kill "$(cat "$pidfile")" 2>/dev/null || true
    done
    sleep 1
}

# ── Phase 1: Create homes, generate identities, write initial configs ────────
for n in $(seq 1 $NUM_NODES); do
    BACKEND_PORT="${PORTS[$((n * 2 - 2))]}"
    SCHEMA_PORT="${PORTS[$((n * 2 - 1))]}"
    NODE_HOME="${SESSION_DIR}/node-${n}"

    mkdir -p "$NODE_HOME/config" "$NODE_HOME/data"

    echo "── Node $n ──────────────────────────────────────"
    echo "  Home:    $NODE_HOME"
    echo "  Backend: http://localhost:$BACKEND_PORT"
    echo "  Schema:  http://localhost:$SCHEMA_PORT"

    # Generate fresh identity
    FOLDDB_HOME="$NODE_HOME" "$BINARY/ensure_identity" > /dev/null 2>&1 || true

    # Write config with bootstrap API key
    CONFIG_FILE="$NODE_HOME/config/node_config.json"
    if [ "$LOCAL_MODE" = true ]; then
        cat > "$CONFIG_FILE" <<CONF
{
  "database": {"type": "local", "path": "$NODE_HOME/data"},
  "storage_path": "$NODE_HOME/data",
  "default_trust_distance": 1,
  "schema_service_url": "http://127.0.0.1:$SCHEMA_PORT"
}
CONF
    else
        cat > "$CONFIG_FILE" <<CONF
{
  "database": {
    "type": "exemem",
    "api_url": "$EXEMEM_API_URL",
    "api_key": "$EXEMEM_API_KEY",
    "session_token": null,
    "user_hash": null
  },
  "storage_path": "$NODE_HOME/data",
  "default_trust_distance": 1,
  "schema_service_url": "http://127.0.0.1:$SCHEMA_PORT"
}
CONF
    fi

    # Start the node
    start_node "$n"

    # Register with Exemem and restart to pick up new API key
    if [ "$LOCAL_MODE" = false ]; then
        INVITE_CODE="${INVITE_CODES[$((n - 1))]}"
        echo -n "  Registering with invite code $INVITE_CODE..."
        REG_RESP=$(curl -s -X POST "http://localhost:$BACKEND_PORT/api/auth/register" \
            -H "Content-Type: application/json" \
            -d "{\"invite_code\": \"$INVITE_CODE\"}")
        REG_OK=$(echo "$REG_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('ok',False))" 2>/dev/null || echo "False")
        if [ "$REG_OK" = "True" ]; then
            echo " registered!"

            # Update config with the new API key from credentials
            CREDS_PATH="$NODE_HOME/credentials.json"
            if [ -f "$CREDS_PATH" ]; then
                python3 -c "
import json
with open('$CONFIG_FILE') as f: config = json.load(f)
with open('$CREDS_PATH') as f: creds = json.load(f)
config['database']['api_key'] = creds['api_key']
with open('$CONFIG_FILE', 'w') as f: json.dump(config, f, indent=2)
"
            fi

            # Restart to pick up new API key
            echo -n "  Restarting with new credentials..."
            stop_node "$n"
            start_node "$n"
        else
            REG_ERR=$(echo "$REG_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('error','unknown'))" 2>/dev/null || echo "unknown")
            echo " failed: $REG_ERR"
        fi
    fi

    echo ""
done

# ── Summary ───────────────────────────────────────────────────────────────────
echo "============================================="
echo "  Org Test Session Ready"
echo "============================================="
echo ""
for n in $(seq 1 $NUM_NODES); do
    BACKEND_PORT="${PORTS[$((n * 2 - 2))]}"
    NODE_HOME="${SESSION_DIR}/node-${n}"
    PK=$(python3 -c "import json; print(json.load(open('$NODE_HOME/config/node_identity.json'))['public_key'])" 2>/dev/null || echo "unknown")
    echo "  Node $n: http://localhost:$BACKEND_PORT"
    echo "    Public Key: $PK"
    echo "    Logs:       $NODE_HOME/server.log"
done
echo ""
echo "  Test flow:"
echo "    1. Open Node 1 in browser → Settings → Organizations → Create Org"
echo "    2. Copy Node 2's public key (shown above or from Node 2's header)"
echo "    3. In Node 1's org → Add Member with Node 2's key"
echo "    4. Open Node 2 in browser → click envelope icon → Accept invite"
echo "    5. Ingest data on Node 1 (select org) → watch it sync to Node 2"
echo ""
echo "  Stop:"
echo "    ./org-test.sh stop"
echo ""
