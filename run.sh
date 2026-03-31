#!/bin/bash

set -e

#######################################
# FoldDB Development Server
#
# Usage:
#   ./run.sh [OPTIONS]
#
# Options:
#   --local          Use local Sled storage (default: cloud/DynamoDB)
#   --exemem         Exemem cloud sync mode (local Sled + encrypted sync)
#   --local-schema   Run local schema service (for offline development)
#   --dev            Use dev schema service (default: prod)
#   --reset-db       Reset database from test_db template
#   --empty-db       Start with empty database
#   --demo           Use isolated demo directories (~/.folddb/demo-data, demo-config)
#   --region=REGION  AWS region for cloud mode (default: us-west-2)
#
# Examples:
#   ./run.sh                           # Cloud mode with prod schema service
#   ./run.sh --dev                     # Cloud mode with dev schema service
#   ./run.sh --local                   # Local storage with global schema service
#   ./run.sh --local --local-schema    # Fully offline development
#   ./run.sh --local --empty-db        # Local with fresh database
#   ./run.sh --exemem                  # Exemem cloud sync mode (requires EXEMEM_API_KEY)
#######################################

# ============================================================================
# Shared Functions
# ============================================================================

cleanup_processes() {
    echo "Checking for existing fold_db processes..."

    # Kill any existing processes (try multiple patterns)
    pkill -f folddb_server 2>/dev/null || true
    pkill -f fold_db 2>/dev/null || true
    pkill -f "cargo run.*fold_db" 2>/dev/null || true
    pkill -f "cargo run.*fold_db" 2>/dev/null || true
    pkill -f schema_service 2>/dev/null || true
    pkill -f "cargo run.*schema" 2>/dev/null || true
    # Kill Vite and related frontend processes
    pkill -f "vite" 2>/dev/null || true
    pkill -f "esbuild.*fold_db" 2>/dev/null || true

    # Kill by port if something is listening
    lsof -ti:9001 | xargs kill -9 2>/dev/null || true
    lsof -ti:9002 | xargs kill -9 2>/dev/null || true
    lsof -ti:5173 | xargs kill -9 2>/dev/null || true

    # Wait for processes to terminate
    sleep 2

    # Force kill if still running
    pkill -9 -f folddb_server 2>/dev/null || true
    pkill -9 -f fold_db 2>/dev/null || true
    pkill -9 -f "cargo run.*fold_db" 2>/dev/null || true
    pkill -9 -f schema_service 2>/dev/null || true
    pkill -9 -f "cargo run.*schema" 2>/dev/null || true
    pkill -9 -f "vite" 2>/dev/null || true

    # Give dying processes time to release locks
    sleep 1

    echo "Cleaned up existing processes."
}

reset_db() {
    echo "Resetting database from test_db template..."
    rm -rf data
    cp -R test_db data
    echo "Database reset complete."
}

empty_db() {
    echo "Initializing empty database directory..."
    rm -rf data
    mkdir -p data
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
    echo "Starting LOCAL schema service on port 9002..."
    nohup cargo run --bin schema_service -- --port 9002 --db-path schema_registry > schema_service.log 2>&1 &
    SCHEMA_SERVICE_PID=$!

    echo "Waiting for local schema service to be ready..."
    for i in {1..30}; do
        if kill -0 $SCHEMA_SERVICE_PID 2>/dev/null; then
            if curl -s http://127.0.0.1:9002/api/health > /dev/null 2>&1; then
                echo "Local schema service started successfully with PID: $SCHEMA_SERVICE_PID"
                echo "Schema service logs: schema_service.log"
                return 0
            fi
            sleep 1
        else
            echo "Schema service process died. Check schema_service.log for details."
            exit 1
        fi
    done

    echo "Local schema service failed to become healthy within 30 seconds."
    kill $SCHEMA_SERVICE_PID 2>/dev/null || true
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
    # Check if node_modules exists AND has a valid vite binary
    if [ ! -d "node_modules" ] || [ ! -x "node_modules/.bin/vite" ]; then
        echo "Installing frontend dependencies..."
        rm -rf node_modules  # Clean up any corrupted state
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

    echo "Starting the HTTP server on port 9001..."
    if [ -n "$features" ]; then
        RUST_LOG=debug nohup cargo run --features "$features" --bin folddb_server -- --port 9001 --schema-service-url "$schema_url" $extra_args > server.log 2>&1 &
    else
        RUST_LOG=debug nohup cargo run --bin folddb_server -- --port 9001 --schema-service-url "$schema_url" $extra_args > server.log 2>&1 &
    fi
    SERVER_PID=$!

    echo "Waiting for HTTP server to be ready..."
    for i in $(seq 1 $timeout); do
        if kill -0 $SERVER_PID 2>/dev/null; then
            if curl -s http://127.0.0.1:9001/api/system/status > /dev/null 2>&1; then
                echo "HTTP server started successfully with PID: $SERVER_PID"
                echo "Server logs: server.log"
                return 0
            fi
            sleep 1
        else
            echo "HTTP server process died. Check server.log for details."
            return 1
        fi
    done

    echo "HTTP server failed to become healthy within $timeout seconds."
    kill $SERVER_PID 2>/dev/null || true
    return 1
}

start_vite_dev() {
    echo ""
    echo "Starting Vite dev server with hot reload..."
    echo "Access app at: http://localhost:5173"
    echo ""

    cd src/server/static-react
    export VITE_ENABLE_SAMPLES=true
    npm run dev
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
REGION="us-west-2"
TABLE_NAME="FoldDBStorage"

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
            REGION="${arg#*=}"
            ;;
        --help|-h)
            head -30 "$0" | tail -25
            exit 0
            ;;
        *)
            ;;
    esac
done

# ============================================================================
# Main Script
# ============================================================================

# Cleanup existing processes
cleanup_processes

# Handle database reset options
if [ "$RESET_DB" = true ]; then
    reset_db
fi

if [ "$EMPTY_DB" = true ]; then
    empty_db
fi

# Ensure config directory exists
mkdir -p config
CONFIG_FILE="config/node_config.json"

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
        # Update schema service URL if --local-schema was passed
        if [ "$LOCAL_SCHEMA" = true ]; then
            python3 -c "
import json
with open('$CONFIG_FILE') as f: cfg = json.load(f)
cfg['schema_service_url'] = 'http://127.0.0.1:9002'
with open('$CONFIG_FILE', 'w') as f: json.dump(cfg, f, indent=2)
" 2>/dev/null
        fi
    fi
fi

# Set up configuration based on mode
if [ "$LOCAL_MODE" = true ]; then
    echo "Setting up LOCAL configuration (Sled storage)..."
    # Determine schema_service_url for config
    CONFIG_SCHEMA_URL="https://y0q3m6vk75.execute-api.us-west-2.amazonaws.com"

    cat > "$CONFIG_FILE" <<EOF
{
  "database": {
    "type": "local",
    "path": "data"
  },
  "storage_path": "data",
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

    EXEMEM_API_URL="https://ygyu7ritx8.execute-api.us-west-2.amazonaws.com"
    CONFIG_SCHEMA_URL="https://y0q3m6vk75.execute-api.us-west-2.amazonaws.com"

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
  "storage_path": "data",
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

    echo "Exemem API: $EXEMEM_API_URL"
    echo "Session token: $([ -n "$EXEMEM_SESSION_TOKEN" ] && echo "configured" || echo "not set")"
    echo "User hash: $([ -n "$EXEMEM_USER_HASH" ] && echo "configured" || echo "not set")"
else
    echo "Setting up CLOUD configuration (DynamoDB storage)..."

    # Get node identity
    echo "Ensuring node identity..."
    USER_ID=$(cargo run --quiet --bin ensure_identity)
    echo "Node Identity (User ID): $USER_ID"

    echo "Region: $REGION"
    echo "Table prefix: $TABLE_NAME"

    CONFIG_SCHEMA_URL="https://y0q3m6vk75.execute-api.us-west-2.amazonaws.com"

    cat > "$CONFIG_FILE" <<EOF
{
  "database": {
    "type": "cloud",
    "region": "$REGION",
    "tables": {
      "main": "${TABLE_NAME}-main",
      "metadata": "${TABLE_NAME}-metadata",
      "permissions": "${TABLE_NAME}-node_id_schema_permissions",
      "transforms": "${TABLE_NAME}-transforms",
      "orchestrator": "${TABLE_NAME}-orchestrator_state",
      "schema_states": "${TABLE_NAME}-schema_states",
      "schemas": "${TABLE_NAME}-schemas",
      "public_keys": "${TABLE_NAME}-public_keys",
      "transform_queue": "${TABLE_NAME}-transform_queue_tree",
      "native_index": "${TABLE_NAME}-native_index",
      "process": "${TABLE_NAME}-process",
      "logs": "${TABLE_NAME}-logs",
      "idempotency": "${TABLE_NAME}-idempotency"
    },
    "auto_create": true,
    "user_id": "$USER_ID"
  },
  "storage_path": "data",
  "default_trust_distance": 1,
  "network_listen_address": "/ip4/0.0.0.0/tcp/0",
  "security_config": {
    "require_tls": false,
    "encrypt_at_rest": false
  },
  "schema_service_url": "$CONFIG_SCHEMA_URL"
}
EOF

    # Export DynamoDB config for ProgressStore
    export FOLD_DYNAMODB_TABLE_PREFIX="$TABLE_NAME"
    export FOLD_DYNAMODB_REGION="$REGION"
    export FOLD_DYNAMODB_USER_ID="$USER_ID"

    CARGO_FEATURES="aws-backend"
    SERVER_TIMEOUT=180

    echo "Note: Ensure AWS credentials are configured"
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

# Schema service setup
# Prod: https://axo709qs11.execute-api.us-east-1.amazonaws.com (TODO: schema.folddb.com once DNS configured)
# Dev:  https://y0q3m6vk75.execute-api.us-west-2.amazonaws.com
SCHEMA_SERVICE_URL="https://y0q3m6vk75.execute-api.us-west-2.amazonaws.com"
SCHEMA_SERVICE_PID=""

if [ "$LOCAL_SCHEMA" = true ]; then
    SCHEMA_SERVICE_URL="http://127.0.0.1:9002"
    start_local_schema_service
else
    echo "Using global schema service at: $SCHEMA_SERVICE_URL"
    if ! check_schema_service "$SCHEMA_SERVICE_URL"; then
        echo ""
        echo "ERROR: Global schema service at $SCHEMA_SERVICE_URL is not reachable."
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
    [ -n "$SCHEMA_SERVICE_PID" ] && kill $SCHEMA_SERVICE_PID 2>/dev/null || true
    exit 1
fi

# Print summary
echo ""
echo "=========================================="
echo "FoldDB Development Server Running"
echo "=========================================="
if [ "$LOCAL_MODE" = true ]; then
    STORAGE_LABEL="LOCAL (Sled)"
elif [ "$EXEMEM_MODE" = true ]; then
    STORAGE_LABEL="EXEMEM (Sled + cloud sync)"
else
    STORAGE_LABEL="CLOUD (DynamoDB)"
fi
echo "Storage: $STORAGE_LABEL"
echo "Schema Service: DEV - $SCHEMA_SERVICE_URL"
[ "$LOCAL_MODE" = false ] && [ "$EXEMEM_MODE" = false ] && echo "AWS Region: $REGION"
echo "=========================================="

# Start Vite dev server (foreground)
start_vite_dev

# Cleanup on exit
echo "Shutting down..."
kill $SERVER_PID 2>/dev/null || true
[ -n "$SCHEMA_SERVICE_PID" ] && kill $SCHEMA_SERVICE_PID 2>/dev/null || true
