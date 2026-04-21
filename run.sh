#!/bin/bash

set -e

#######################################
# FoldDB Development Server
#
# Usage:
#   ./run.sh [OPTIONS]
#
# Options:
#   --local          Use local Sled storage (default, kept for compatibility)
#   --exemem         Exemem cloud sync mode (local Sled + encrypted sync)
#   --local-schema   Run local schema service (for offline development)
#   --dev            Use dev schema service (default: prod)
#   --reset-db       Reset database from test_db template
#   --empty-db       Start with empty database
#   --demo           Use isolated demo directories ($FOLDDB_HOME/demo-data, demo-config)
#   --region=REGION  Legacy flag, ignored
#   --home <path>    Set FOLDDB_HOME (default: .folddb relative to CWD)
#   --port <port>    HTTP server port (default: auto-slot in 9101..=9199,
#                    or value of FOLDDB_PORT env var)
#   --schema-port <port>  Schema service port (default: <http_port> + 1)
#
# Environment Variables:
#   FOLDDB_HOME      Where all instance-specific state lives (default: .folddb)
#   FOLDDB_PORT      HTTP server port (alternative to --port)
#   VITE_PORT        Vite frontend port (pin to a specific port; disables scan)
#   VITE_PORT_BASE   First port in the Vite auto-slot scan (default: 5173)
#   VITE_PORT_COUNT  How many ports the scan covers (default: 127 → 5173..=5299)
#
# Examples:
#   ./run.sh                           # Local Sled mode with prod schema service
#   ./run.sh --dev                     # Local Sled mode with dev schema service
#   ./run.sh --local                   # Local storage with global schema service
#   ./run.sh --local --local-schema    # Fully offline development
#   ./run.sh --local --empty-db        # Local with fresh database
#   ./run.sh --exemem                  # Exemem cloud sync mode (requires EXEMEM_API_KEY)
#   ./run.sh --home /tmp/node2 --port 9003 --local --local-schema
#######################################

# ============================================================================
# Shared Functions
# ============================================================================

# Kill a process by reading its PID from a file.
# Usage: kill_pid_file <path>
kill_pid_file() {
    local pidfile="$1"
    if [ -f "$pidfile" ]; then
        local pid
        pid=$(cat "$pidfile" 2>/dev/null)
        if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
            echo "Stopping process $pid (from $pidfile)..."
            kill "$pid" 2>/dev/null || true
            # Wait up to 3 seconds for graceful shutdown
            for i in 1 2 3; do
                kill -0 "$pid" 2>/dev/null || break
                sleep 1
            done
            # Force kill if still alive
            if kill -0 "$pid" 2>/dev/null; then
                kill -9 "$pid" 2>/dev/null || true
            fi
        fi
        rm -f "$pidfile"
    fi
}

cleanup_processes() {
    echo "Checking for existing fold_db processes..."

    # PID-based cleanup — only kill processes we started
    kill_pid_file "$FOLDDB_HOME/folddb.pid"
    kill_pid_file "$FOLDDB_HOME/schema.pid"
    kill_pid_file "$FOLDDB_HOME/vite.pid"

    echo "Cleaned up existing processes."
}

# Cleanup handler for script exit
on_exit() {
    echo "Shutting down..."
    kill_pid_file "$FOLDDB_HOME/folddb.pid"
    kill_pid_file "$FOLDDB_HOME/schema.pid"
    kill_pid_file "$FOLDDB_HOME/vite.pid"
    # Remove the auto-slot discovery file so stale entries don't accumulate.
    if [ "${AUTO_SLOT:-}" = true ] && [ -n "$HTTP_PORT" ]; then
        rm -f "$HOME/.folddb-slots/$HTTP_PORT.json" 2>/dev/null || true
    fi
}
trap on_exit EXIT

reset_db() {
    echo "Resetting database from test_db template..."
    rm -rf "$FOLDDB_HOME/data"
    cp -R test_db "$FOLDDB_HOME/data"
    echo "Database reset complete."
}

empty_db() {
    echo "Initializing empty database directory..."
    rm -rf "$FOLDDB_HOME/data"
    mkdir -p "$FOLDDB_HOME/data"
    echo "Empty database directory ready."
}

load_api_keys() {
    # Load shell profile to get API keys
    # Temporarily disable set -e because shell profiles often have commands
    # that return non-zero (completions, conda init, etc.)
    set +e
    source ~/.zshrc 2>/dev/null || source ~/.bashrc 2>/dev/null || true
    set -e

    if [ -n "$ANTHROPIC_API_KEY" ]; then
        export ANTHROPIC_API_KEY
        echo "Anthropic API key configured"
    else
        echo "NOTE: ANTHROPIC_API_KEY not set. Configure AI provider in the UI or set it in your shell profile."
    fi
}

check_schema_service() {
    local url="$1"
    echo "Checking schema service connectivity..."
    if curl -s --connect-timeout 10 "$url/api/health" > /dev/null 2>&1; then
        echo "Schema service is reachable."
        return 0
    else
        return 1
    fi
}

start_local_schema_service() {
    # Schema service auto-detects its AI provider:
    #   - ANTHROPIC_API_KEY set → Anthropic (fast, accurate classification via Haiku)
    #   - No API key → Ollama (local, needs model config)
    # We do NOT force AI_PROVIDER=ollama — let it prefer Anthropic when available.

    local has_anthropic_key=false
    [ -n "$ANTHROPIC_API_KEY" ] && has_anthropic_key=true

    if [ "$has_anthropic_key" = true ]; then
        echo "Starting LOCAL schema service on port $SCHEMA_PORT (Anthropic for classification)..."
    else
        echo "Starting LOCAL schema service on port $SCHEMA_PORT (Ollama for classification)..."
    fi

    # Read Ollama model/URL from saved config (used when Anthropic key is absent)
    local config_file="${FOLDDB_HOME}/config/ingestion_config.json"
    local ollama_model=""
    local ollama_url=""
    if [ -f "$config_file" ]; then
        ollama_model=$(python3 -c "import json; c=json.load(open('$config_file')); print(c.get('ollama',{}).get('model',''))" 2>/dev/null)
        ollama_url=$(python3 -c "import json; c=json.load(open('$config_file')); print(c.get('ollama',{}).get('base_url',''))" 2>/dev/null)
    fi

    # If no saved config, detect a safe default based on system RAM.
    # Without this, the schema service falls back to OLLAMA_DEFAULT (llama3.3 / 70B)
    # which most users don't have installed.
    if [ -z "$ollama_model" ]; then
        local ram_gb
        ram_gb=$(sysctl -n hw.memsize 2>/dev/null | awk '{printf "%d", $1/1073741824}')
        if [ -n "$ram_gb" ] && [ "$ram_gb" -ge 64 ] 2>/dev/null; then
            ollama_model="llama3.3"
        elif [ -n "$ram_gb" ] && [ "$ram_gb" -ge 32 ] 2>/dev/null; then
            ollama_model="llama3.1:8b"
        else
            ollama_model="llama3.2:3b"
        fi
    fi

    [ -n "$ollama_model" ] && echo "  Ollama model (fallback): $ollama_model"
    [ -n "$ollama_url" ] && echo "  Ollama URL: $ollama_url"

    # Pass Ollama config as fallback — schema service uses Anthropic when ANTHROPIC_API_KEY is set
    local schema_env=""
    [ -n "$ollama_model" ] && schema_env="$schema_env OLLAMA_MODEL=$ollama_model"
    [ -n "$ollama_url" ] && schema_env="$schema_env OLLAMA_BASE_URL=$ollama_url"

    # Phase 0 T3: the dev binary moved to the sibling submodule at
    # ../schema_service. Resolve absolute paths for everything we pass to the
    # spawned process, then enter the submodule so cargo picks up its
    # .cargo/config.toml (which patches the fold_db git dep to ../fold_db for
    # local dev). Pre-build synchronously so the 30s liveness loop below
    # doesn't race a cold cargo build.
    local schema_service_dir home_abs schema_db_path schema_log
    schema_service_dir="$(cd ../schema_service && pwd)"
    home_abs="$(cd "$FOLDDB_HOME" && pwd)"
    schema_db_path="$home_abs/schema_registry"
    schema_log="$home_abs/schema_service.log"

    echo "Building schema_service binary from $schema_service_dir..."
    ( cd "$schema_service_dir" && cargo build -p schema_service_server_http --bin schema_service )

    pushd "$schema_service_dir" > /dev/null
    nohup env $schema_env cargo run -p schema_service_server_http --bin schema_service -- --port "$SCHEMA_PORT" --db-path "$schema_db_path" > "$schema_log" 2>&1 &
    SCHEMA_SERVICE_PID=$!
    popd > /dev/null
    echo "$SCHEMA_SERVICE_PID" > "$FOLDDB_HOME/schema.pid"

    echo "Waiting for local schema service to be ready..."
    for i in {1..30}; do
        if kill -0 $SCHEMA_SERVICE_PID 2>/dev/null; then
            # Submodule binary serves routes under /v1/ (old /api/ prefix dropped in Phase 0).
            if curl -s "http://127.0.0.1:${SCHEMA_PORT}/v1/health" > /dev/null 2>&1; then
                echo "Local schema service started successfully with PID: $SCHEMA_SERVICE_PID"
                echo "Schema service logs: $schema_log"
                return 0
            fi
            sleep 1
        else
            echo "Schema service process died. Check $schema_log for details."
            exit 1
        fi
    done

    echo "Local schema service failed to become healthy within 30 seconds."
    kill $SCHEMA_SERVICE_PID 2>/dev/null || true
    rm -f "$FOLDDB_HOME/schema.pid"
    exit 1
}

build_project() {
    local features="$1"
    echo "Building the Rust project..."
    if [ -n "$features" ]; then
        cargo build --features "$features"
    else
        cargo build
    fi

    if [ $? -ne 0 ]; then
        echo "Rust build failed. Exiting."
        exit 1
    fi
}

generate_openapi() {
    local features="$1"
    echo "Generating OpenAPI spec..."
    mkdir -p target
    if [ -n "$features" ]; then
        cargo run --features "$features" --quiet --bin openapi_dump > target/openapi.json
    else
        cargo run --quiet --bin openapi_dump > target/openapi.json
    fi

    if [ $? -ne 0 ]; then
        echo "Failed to generate OpenAPI spec. Exiting."
        exit 1
    fi
}

install_frontend_deps() {
    cd src/server/static-react
    local needs_install=false
    local reason=""

    if [ ! -d "node_modules" ] || [ ! -x "node_modules/.bin/vite" ]; then
        needs_install=true
        reason="missing or corrupted node_modules"
        rm -rf node_modules
    elif [ ! -f "node_modules/.package-lock.json" ]; then
        # npm writes .package-lock.json on every successful install; absence means stale.
        needs_install=true
        reason="node_modules/.package-lock.json missing (stale install)"
    elif [ "package-lock.json" -nt "node_modules/.package-lock.json" ]; then
        needs_install=true
        reason="package-lock.json newer than installed deps"
    elif [ "package.json" -nt "node_modules/.package-lock.json" ]; then
        needs_install=true
        reason="package.json newer than installed deps"
    fi

    if [ "$needs_install" = true ]; then
        echo "Installing frontend dependencies ($reason)..."
        npm install
        if [ $? -ne 0 ]; then
            echo "Failed to install frontend dependencies. Exiting."
            exit 1
        fi
    fi
    cd ../../..
}

start_http_server() {
    local features="$1"
    local schema_url="$2"
    local timeout="$3"
    local demo_flag="$4"

    local extra_args=""
    if [ "$demo_flag" = true ]; then
        extra_args="--demo"
    fi

    # Preflight: fail fast with a clear error if something else is already
    # bound to $HTTP_PORT. Otherwise folddb_server crashes with EADDRINUSE
    # but Vite (started later) happily serves a dead UI that 401s forever.
    local holder
    holder=$(lsof -iTCP:"$HTTP_PORT" -sTCP:LISTEN -t 2>/dev/null | head -1)
    if [ -n "$holder" ]; then
        local holder_cmd
        holder_cmd=$(ps -o command= -p "$holder" 2>/dev/null || echo "<unknown>")
        echo "error: port $HTTP_PORT is already bound by PID $holder ($holder_cmd)." >&2
        echo "       Stop that process or rerun without --port / FOLDDB_PORT to auto-slot a free port." >&2
        return 1
    fi

    echo "Starting the HTTP server on port $HTTP_PORT..."
    if [ -n "$features" ]; then
        FOLDDB_HOME="$FOLDDB_HOME" RUST_LOG=debug nohup cargo run --features "$features" --bin folddb_server -- --port "$HTTP_PORT" --schema-service-url "$schema_url" $extra_args > "$FOLDDB_HOME/server.log" 2>&1 &
    else
        FOLDDB_HOME="$FOLDDB_HOME" RUST_LOG=debug nohup cargo run --bin folddb_server -- --port "$HTTP_PORT" --schema-service-url "$schema_url" $extra_args > "$FOLDDB_HOME/server.log" 2>&1 &
    fi
    SERVER_PID=$!
    echo "$SERVER_PID" > "$FOLDDB_HOME/folddb.pid"

    echo "Waiting for HTTP server to be ready..."
    for i in $(seq 1 $timeout); do
        if kill -0 $SERVER_PID 2>/dev/null; then
            if curl -s "http://127.0.0.1:${HTTP_PORT}/api/system/status" > /dev/null 2>&1; then
                echo "HTTP server started successfully with PID: $SERVER_PID"
                echo "Server logs: $FOLDDB_HOME/server.log"
                return 0
            fi
            sleep 1
        else
            echo "HTTP server process died. Tail of $FOLDDB_HOME/server.log:" >&2
            tail -n 20 "$FOLDDB_HOME/server.log" >&2 2>/dev/null || true
            return 1
        fi
    done

    echo "HTTP server failed to become healthy within $timeout seconds." >&2
    echo "Tail of $FOLDDB_HOME/server.log:" >&2
    tail -n 20 "$FOLDDB_HOME/server.log" >&2 2>/dev/null || true
    kill $SERVER_PID 2>/dev/null || true
    rm -f "$FOLDDB_HOME/folddb.pid"
    return 1
}

start_vite_dev() {
    echo ""
    echo "Starting Vite dev server with hot reload..."
    echo "Access app at: http://localhost:$VITE_PORT"
    echo ""

    cd src/server/static-react
    export VITE_ENABLE_SAMPLES=true
    export VITE_API_PORT="$HTTP_PORT"
    npm run dev -- --port "$VITE_PORT" --strictPort
}

# ============================================================================
# Parse Arguments
# ============================================================================

LOCAL_MODE=false
EXEMEM_MODE=false
LOCAL_SCHEMA=false
DEV_MODE=false
RESET_DB=false
EMPTY_DB=false
DEMO_MODE=false
# Auto-slot: when neither --port nor FOLDDB_PORT is set, pick the first free
# port in 9101..=9199 so N parallel agents can each run their own fold_db
# instance without any coordination. If FOLDDB_HOME is also unset, derive
# a per-slot FOLDDB_HOME from the chosen port; otherwise preserve the
# caller's FOLDDB_HOME. The prod Tauri bundle owns 9001; dev lives in the
# 9101 range.
HTTP_PORT=""
SCHEMA_PORT=""
AUTO_SLOT=false
if [ -n "$FOLDDB_PORT" ]; then
    HTTP_PORT="$FOLDDB_PORT"
fi

for arg in "$@"; do
    case "$arg" in
        --local)
            LOCAL_MODE=true
            ;;
        --exemem)
            EXEMEM_MODE=true
            ;;
        --local-schema)
            LOCAL_SCHEMA=true
            ;;
        --dev)
            DEV_MODE=true
            ;;
        --reset-db)
            RESET_DB=true
            ;;
        --empty-db)
            EMPTY_DB=true
            ;;
        --demo)
            DEMO_MODE=true
            ;;
        --region=*)
            # Legacy flag, ignored
            ;;
        --home)
            # Handled below via positional peek
            ;;
        --home=*)
            FOLDDB_HOME="${arg#*=}"
            ;;
        --port)
            # Handled below via positional peek
            ;;
        --port=*)
            HTTP_PORT="${arg#*=}"
            ;;
        --schema-port)
            # Handled below via positional peek
            ;;
        --schema-port=*)
            SCHEMA_PORT="${arg#*=}"
            ;;
        --help|-h)
            head -38 "$0" | tail -33
            exit 0
            ;;
        *)
            ;;
    esac
done

# Handle --home <value>, --port <value>, --schema-port <value> (space-separated)
args=("$@")
for i in "${!args[@]}"; do
    case "${args[$i]}" in
        --home)
            FOLDDB_HOME="${args[$((i+1))]}"
            ;;
        --port)
            HTTP_PORT="${args[$((i+1))]}"
            ;;
        --schema-port)
            SCHEMA_PORT="${args[$((i+1))]}"
            ;;
    esac
done

# Auto-slot: if no HTTP port was pinned (no --port, no FOLDDB_PORT), scan
# 9101..=9199 for a free port so parallel agents don't collide. An explicit
# FOLDDB_HOME does NOT disable the port scan — the caller may want an
# isolated data dir but still let us find a free port for them. If
# FOLDDB_HOME is also unset we derive a per-slot one from the chosen port;
# otherwise we preserve what the caller set.
#
# Use lsof (not a bash /dev/tcp probe) because folddb_server may be bound to
# an IPv6 listener; a /dev/tcp/127.0.0.1 probe would miss it and hand us a
# port the backend can't actually bind, crashing it with EADDRINUSE while
# Vite (started later) would happily serve a UI talking to nothing.
if [ -z "$HTTP_PORT" ]; then
    for candidate in $(seq 9101 9199); do
        if ! lsof -iTCP:"$candidate" -sTCP:LISTEN -t >/dev/null 2>&1; then
            HTTP_PORT="$candidate"
            AUTO_SLOT=true
            break
        fi
    done
    if [ -z "$HTTP_PORT" ]; then
        echo "error: no free TCP port found in 9101..=9199 — every port that run.sh would try is occupied" >&2
        exit 1
    fi
    if [ -z "$FOLDDB_HOME" ]; then
        FOLDDB_HOME="/tmp/folddb-slot-$HTTP_PORT"
        echo "[run.sh] auto-slot: port=$HTTP_PORT, home=$FOLDDB_HOME"
    else
        echo "[run.sh] auto-slot: port=$HTTP_PORT (home=$FOLDDB_HOME preserved)"
    fi
fi

# Fill in remaining defaults for whichever of port/home wasn't pinned.
if [ -z "$HTTP_PORT" ]; then
    HTTP_PORT=9101
fi
if [ -z "$SCHEMA_PORT" ]; then
    SCHEMA_PORT=$((HTTP_PORT + 1))
fi
if [ -z "$FOLDDB_HOME" ]; then
    FOLDDB_HOME=".folddb"
fi
export FOLDDB_HOME

# Vite port: scan VITE_PORT_BASE..VITE_PORT_BASE+VITE_PORT_COUNT-1 for the
# first free slot so that parallel `run.sh` invocations don't collide on
# the frontend. Defaults cover 5173..=5299 (127 ports); override
# VITE_PORT_BASE / VITE_PORT_COUNT for stacks that already hold a chunk
# of 5173+. An explicit $VITE_PORT pins a single port and disables the
# scan. Independent from the backend HTTP_PORT auto-slot above — Vite
# port collisions happen even when the backend was explicitly pinned.
#
# Use lsof (not a bash /dev/tcp probe) because Vite binds IPv6 by default;
# a /dev/tcp/127.0.0.1 probe would miss an IPv6-only listener and hand us
# a port Vite can't actually bind.
if [ -z "${VITE_PORT:-}" ]; then
    vite_port_base="${VITE_PORT_BASE:-5173}"
    vite_port_count="${VITE_PORT_COUNT:-127}"
    vite_port_end=$((vite_port_base + vite_port_count - 1))
    for candidate in $(seq "$vite_port_base" "$vite_port_end"); do
        if ! lsof -iTCP:"$candidate" -sTCP:LISTEN -t >/dev/null 2>&1; then
            VITE_PORT="$candidate"
            break
        fi
    done
    if [ -z "${VITE_PORT:-}" ]; then
        echo "error: no free TCP port found in ${vite_port_base}..=${vite_port_end} for Vite dev server" >&2
        echo "       Free a port, or widen the range via VITE_PORT_BASE / VITE_PORT_COUNT," >&2
        echo "       or pin a specific one with VITE_PORT=<port>." >&2
        exit 1
    fi
fi
export VITE_PORT

# Publish the chosen slot so external tools can discover a running instance
# without hardcoding a port. Best-effort; non-fatal if it fails.
if [ "$AUTO_SLOT" = true ]; then
    mkdir -p "$HOME/.folddb-slots" 2>/dev/null || true
    cat > "$HOME/.folddb-slots/$HTTP_PORT.json" 2>/dev/null <<EOF || true
{"port": $HTTP_PORT, "schema_port": $SCHEMA_PORT, "vite_port": $VITE_PORT, "home": "$FOLDDB_HOME", "pid": $$}
EOF
fi

# Export EXEMEM_ENV so the Rust process picks up the correct environment.
# Default is prod. --dev flag overrides to dev.
if [ "$DEV_MODE" = true ]; then
    export EXEMEM_ENV="${EXEMEM_ENV:-dev}"
else
    export EXEMEM_ENV="${EXEMEM_ENV:-prod}"
fi

# Resolve the schema service URL once so the persisted node_config.json and the
# in-memory runtime config agree. Without this, debugging is misleading: a
# --local-schema node writes the prod URL to disk even though it talks to
# 127.0.0.1 at runtime.
SCHEMA_URL_PROD="https://axo709qs11.execute-api.us-east-1.amazonaws.com"
SCHEMA_URL_DEV="https://y0q3m6vk75.execute-api.us-west-2.amazonaws.com"
if [ "$LOCAL_SCHEMA" = true ]; then
    CONFIG_SCHEMA_URL="http://127.0.0.1:${SCHEMA_PORT}"
elif [ "$DEV_MODE" = true ]; then
    CONFIG_SCHEMA_URL="$SCHEMA_URL_DEV"
else
    CONFIG_SCHEMA_URL="$SCHEMA_URL_PROD"
fi

# ============================================================================
# Main Script
# ============================================================================

# Ensure FOLDDB_HOME directory exists
mkdir -p "$FOLDDB_HOME"

# Cleanup existing processes (PID-based, only kills our processes)
cleanup_processes

# Handle database reset options
if [ "$RESET_DB" = true ]; then
    reset_db
fi

if [ "$EMPTY_DB" = true ]; then
    empty_db
fi

# Ensure config directory exists
mkdir -p "$FOLDDB_HOME/config"
CONFIG_FILE="$FOLDDB_HOME/config/node_config.json"

# Set NODE_CONFIG so Rust code finds the config file
export NODE_CONFIG="$CONFIG_FILE"

# Set FOLD_CONFIG_DIR so ingestion config can be saved/loaded
export FOLD_CONFIG_DIR="$FOLDDB_HOME/config"

# Backup existing config
if [ -f "$CONFIG_FILE" ]; then
    cp "$CONFIG_FILE" "${CONFIG_FILE}.backup"
fi

# If no explicit mode flag was given and saved config is Exemem, respect it
if [ "$LOCAL_MODE" = false ] && [ "$EXEMEM_MODE" = false ] && [ -f "$CONFIG_FILE" ]; then
    SAVED_DB_TYPE=$(python3 -c "import json; print(json.load(open('$CONFIG_FILE')).get('database',{}).get('type',''))" 2>/dev/null || echo "")
    if [ "$SAVED_DB_TYPE" = "exemem" ]; then
        echo "Detected saved Exemem config — preserving it (use --local to override)"
        EXEMEM_MODE=true
        # Read credentials from saved config so the EXEMEM_MODE branch doesn't fail on empty key
        EXEMEM_API_KEY=$(python3 -c "import json; print(json.load(open('$CONFIG_FILE')).get('database',{}).get('api_key',''))" 2>/dev/null || echo "")
        export EXEMEM_API_KEY
        # Align the persisted schema URL with the effective runtime URL for this
        # invocation's flags (--local-schema / --dev / prod default).
        python3 -c "
import json
with open('$CONFIG_FILE') as f: cfg = json.load(f)
cfg['schema_service_url'] = '${CONFIG_SCHEMA_URL}'
with open('$CONFIG_FILE', 'w') as f: json.dump(cfg, f, indent=2)
" 2>/dev/null
    fi
fi

# Set up configuration based on mode
if [ "$LOCAL_MODE" = true ]; then
    echo "Setting up LOCAL configuration (Sled storage)..."

    cat > "$CONFIG_FILE" <<EOF
{
  "database": {
    "type": "local",
    "path": "$FOLDDB_HOME/data"
  },
  "storage_path": "$FOLDDB_HOME/data",
  "default_trust_distance": 1,
  "network_listen_address": "/ip4/0.0.0.0/tcp/0",
  "security_config": {
    "require_tls": false,
    "encrypt_at_rest": false
  },
  "schema_service_url": "$CONFIG_SCHEMA_URL"
}
EOF
    CARGO_FEATURES=""
    SERVER_TIMEOUT=60
elif [ "$EXEMEM_MODE" = true ]; then
    echo "Setting up EXEMEM configuration (Sled + encrypted cloud sync)..."

    if [ -z "$EXEMEM_API_KEY" ]; then
        echo "ERROR: EXEMEM_API_KEY environment variable is required for --exemem mode."
        echo "Set it in your shell profile or export it before running:"
        echo "  export EXEMEM_API_KEY=your_api_key"
        exit 1
    fi

    EXEMEM_API_URL="https://jdsx4ixk2i.execute-api.us-east-1.amazonaws.com"
    if [ "$DEV_MODE" = true ]; then
        EXEMEM_API_URL="https://ygyu7ritx8.execute-api.us-west-2.amazonaws.com"
    fi

    # Build optional JSON fields
    if [ -n "$EXEMEM_SESSION_TOKEN" ]; then
        SESSION_TOKEN_JSON="\"$EXEMEM_SESSION_TOKEN\""
    else
        SESSION_TOKEN_JSON="null"
    fi

    if [ -n "$EXEMEM_USER_HASH" ]; then
        USER_HASH_JSON="\"$EXEMEM_USER_HASH\""
    else
        USER_HASH_JSON="null"
    fi

    cat > "$CONFIG_FILE" <<EOF
{
  "database": {
    "type": "exemem",
    "api_url": "$EXEMEM_API_URL",
    "api_key": "$EXEMEM_API_KEY",
    "session_token": $SESSION_TOKEN_JSON,
    "user_hash": $USER_HASH_JSON
  },
  "storage_path": "$FOLDDB_HOME/data",
  "default_trust_distance": 1,
  "network_listen_address": "/ip4/0.0.0.0/tcp/0",
  "security_config": {
    "require_tls": false,
    "encrypt_at_rest": false
  },
  "schema_service_url": "$CONFIG_SCHEMA_URL"
}
EOF
    CARGO_FEATURES=""
    SERVER_TIMEOUT=60

    # Wire up discovery service (same API Gateway as Exemem)
    export DISCOVERY_SERVICE_URL="${EXEMEM_API_URL}/api"

    # Derive DISCOVERY_MASTER_KEY from node's private key if identity exists
    # Identity is created during registration, not on startup
    IDENTITY_FILE="$FOLDDB_HOME/config/node_identity.json"
    if [ -f "$IDENTITY_FILE" ]; then
        NODE_PRIV_KEY=$(python3 -c "import json; print(json.load(open('$IDENTITY_FILE'))['private_key'])" 2>/dev/null || echo "")
        if [ -n "$NODE_PRIV_KEY" ]; then
            export DISCOVERY_MASTER_KEY=$(printf '%s' "$NODE_PRIV_KEY" | shasum -a 256 | cut -d' ' -f1)
        fi
    fi

    echo "Exemem API: $EXEMEM_API_URL"
    echo "Discovery: $([ -n "$DISCOVERY_MASTER_KEY" ] && echo "configured" || echo "no identity yet — register to create one")"
else
    # Default: local Sled storage (same as --local)
    echo "Setting up LOCAL configuration (Sled storage)..."

    cat > "$CONFIG_FILE" <<EOF
{
  "database": {
    "type": "local",
    "path": "$FOLDDB_HOME/data"
  },
  "storage_path": "$FOLDDB_HOME/data",
  "default_trust_distance": 1,
  "network_listen_address": "/ip4/0.0.0.0/tcp/0",
  "security_config": {
    "require_tls": false,
    "encrypt_at_rest": false
  },
  "schema_service_url": "$CONFIG_SCHEMA_URL"
}
EOF
    CARGO_FEATURES=""
    SERVER_TIMEOUT=60
fi

echo "Configuration saved to $CONFIG_FILE"

# Build project
build_project "$CARGO_FEATURES"

# Generate OpenAPI spec
generate_openapi "$CARGO_FEATURES"

# Install frontend dependencies
install_frontend_deps

# Load API keys
load_api_keys

# Schema service setup — reuse the URL already resolved for the persisted config.
SCHEMA_SERVICE_URL="$CONFIG_SCHEMA_URL"
SCHEMA_SERVICE_PID=""

if [ "$LOCAL_SCHEMA" = true ]; then
    start_local_schema_service
else
    if [ "$DEV_MODE" = true ]; then
        echo "Using DEV schema service at: $SCHEMA_SERVICE_URL"
    else
        echo "Using global schema service at: $SCHEMA_SERVICE_URL"
    fi
    if ! check_schema_service "$SCHEMA_SERVICE_URL"; then
        echo ""
        echo "ERROR: Schema service at $SCHEMA_SERVICE_URL is not reachable."
        echo ""
        echo "The schema service is required for FoldDB to operate."
        echo "Options:"
        echo "  1. Check your internet connection"
        echo "  2. Use --local-schema flag for offline development:"
        echo "     ./run.sh --local --local-schema"
        echo ""
        exit 1
    fi
fi

# Start HTTP server
if ! start_http_server "$CARGO_FEATURES" "$SCHEMA_SERVICE_URL" "$SERVER_TIMEOUT" "$DEMO_MODE"; then
    exit 1
fi

# Print summary
echo ""
echo "=========================================="
echo "FoldDB Development Server Running"
echo "=========================================="
if [ "$EXEMEM_MODE" = true ]; then
    STORAGE_LABEL="EXEMEM (Sled + cloud sync)"
else
    STORAGE_LABEL="LOCAL (Sled)"
fi
echo "Storage: $STORAGE_LABEL"
echo "FOLDDB_HOME: $FOLDDB_HOME"
echo "HTTP Port: $HTTP_PORT"
echo "Schema Service: DEV - $SCHEMA_SERVICE_URL"
echo "=========================================="

# Start Vite dev server (foreground — on_exit trap handles cleanup)
start_vite_dev
