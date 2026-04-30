#!/bin/bash

set -e

# ============================================================================
# FoldDB Production Local Server script
#
# This script builds the React frontend, builds the Rust backend in release mode,
# and runs the resulting binary on a different port and using a different schema.
# ============================================================================

PORT=9005
DATA_DIR="prod_data"
CONFIG_DIR="prod_config"
CONFIG_FILE="$CONFIG_DIR/node_config.json"

echo "=========================================="
echo "Building Production Local Instance"
echo "=========================================="

# 1. Build the React frontend
echo "-> Building React Frontend"
cd src/server/static-react
if [ ! -d "node_modules" ] || [ ! -x "node_modules/.bin/vite" ]; then
    echo "Installing frontend dependencies..."
    npm install
fi
npm run build
cd ../../..

# 2. Build the Rust binary in release mode
echo "-> Building Rust Backend (Release Mode)"
cargo build --release

# 3. Ensure config and data directories exist
mkdir -p "$DATA_DIR"
mkdir -p "$CONFIG_DIR"

# 4. Generate a default local node_config if none exists
if [ ! -f "$CONFIG_FILE" ]; then
    echo "-> Creating default production configuration in $CONFIG_FILE"
    # Using the production schema service (URL from environments.json registry).
    CONFIG_SCHEMA_URL="$(./scripts/get-env-url.sh prod schema_service)"
    
    cat > "$CONFIG_FILE" <<EOF
{
  "database": {
    "type": "local",
    "path": "$DATA_DIR"
  },
  "storage_path": "$DATA_DIR",
  "default_trust_distance": 1,
  "network_listen_address": "/ip4/0.0.0.0/tcp/0",
  "security_config": {
    "require_tls": false,
    "encrypt_at_rest": false
  },
  "schema_service_url": "$CONFIG_SCHEMA_URL"
}
EOF
fi

# Load Anthropic API key if needed
set +e
source ~/.zshrc 2>/dev/null || source ~/.bashrc 2>/dev/null || true
set -e

# 5. Run the server
echo "=========================================="
echo "Starting Production Local Instance"
echo "UI URL:           http://localhost:$PORT"
echo "Data Directory:   $DATA_DIR"
echo "Config Directory: $CONFIG_DIR"
echo "Config File:      $CONFIG_FILE"
echo "To stop:          Ctrl+C"
echo "=========================================="

export NODE_CONFIG="$CONFIG_FILE"
export RUST_LOG=info

# Execute the binary in the foreground
./target/release/folddb_server --port $PORT --data-dir "$DATA_DIR"
