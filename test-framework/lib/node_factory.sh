#!/usr/bin/env bash
# Node factory: create invite codes, spawn nodes, register, configure.
# Requires: jq, aws, curl, openssl.

set -euo pipefail

nf_create_invite_codes() {
  local n="$1"
  local i
  for ((i=0; i<n; i++)); do
    local code
    code="test-$(date +%s)-$RANDOM-$i"
    aws dynamodb put-item \
      --table-name ExememInviteCodes-dev \
      --item "{\"code\":{\"S\":\"$code\"},\"used\":{\"BOOL\":false},\"created_at\":{\"N\":\"$(date +%s)\"}}" \
      >/dev/null
    echo "$code"
  done
}

nf_find_binary() {
  local dir
  dir="$(pwd)"
  while [[ "$dir" != "/" ]]; do
    if [[ -x "$dir/target/release/folddb_server" ]]; then
      echo "$dir/target/release/folddb_server"; return 0
    fi
    if [[ -x "$dir/target/debug/folddb_server" ]]; then
      echo "$dir/target/debug/folddb_server"; return 0
    fi
    dir="$(dirname "$dir")"
  done
  echo "folddb_server binary not found" >&2
  return 1
}

nf_spawn_node() {
  local name="$1" port="$2" session_dir="$3"
  local node_dir="$session_dir/nodes/$name"
  mkdir -p "$node_dir/data"

  # Generate unique Ed25519 keypair
  openssl genpkey -algorithm ed25519 -out "$node_dir/node.key" 2>/dev/null

  cat > "$node_dir/config.json" <<EOF
{
  "bind_port": $port,
  "data_dir": "$node_dir/data",
  "schema_service_url": "${FOLDDB_TEST_DEV_SCHEMA:-https://schema-dev.folddb.com}",
  "exemem_api_url": "${FOLDDB_TEST_DEV_API:-https://api-dev.exemem.com}"
}
EOF

  local bin
  bin="$(nf_find_binary)"
  nohup "$bin" --config "$node_dir/config.json" \
    >"$node_dir/stdout.log" 2>"$node_dir/stderr.log" &
  local pid=$!
  echo "$pid" > "$node_dir/pid"
  echo "$pid"
}

nf_wait_healthy() {
  local port="$1" timeout="${2:-30}"
  local deadline=$(( $(date +%s) + timeout ))
  while (( $(date +%s) < deadline )); do
    if curl -fsS "http://127.0.0.1:$port/api/system/auto-identity" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.5
  done
  echo "node on port $port not healthy after ${timeout}s" >&2
  return 1
}

nf_register_node() {
  local port="$1" invite="$2"
  local resp
  resp="$(curl -fsS -X POST "http://127.0.0.1:$port/api/auth/register" \
    -H 'content-type: application/json' \
    -d "{\"invite_code\":\"$invite\"}")"
  # Node may restart on Sled lock; retry healthz then re-fetch identity
  sleep 1
  nf_wait_healthy "$port" 30 >/dev/null || true
  echo "$resp"
}

nf_set_display_name() {
  local port="$1" hash="$2" name="$3"
  curl -fsS -X PUT "http://127.0.0.1:$port/api/identity/card" \
    -H 'content-type: application/json' \
    -H "X-User-Hash: $hash" \
    -d "{\"display_name\":\"$name\"}" >/dev/null
}

nf_shutdown_node() {
  local name="$1" session_dir="$2"
  local pidfile="$session_dir/nodes/$name/pid"
  if [[ -f "$pidfile" ]]; then
    local pid
    pid="$(cat "$pidfile")"
    kill "$pid" 2>/dev/null || true
    sleep 0.2
    kill -9 "$pid" 2>/dev/null || true
  fi
}
