#!/usr/bin/env bash
# Memory-agent dogfood harness.
#
# Boots an isolated fold_db_node + schema service on dedicated ports,
# registers the Memory schema, seeds a fixture set, and exposes commands
# for iterating on the memory agent design end-to-end.
#
# Usage:
#   ./dogfood-memory.sh start          # build + boot + seed (idempotent-ish)
#   ./dogfood-memory.sh stop           # kill processes, keep data dir
#   ./dogfood-memory.sh reset          # stop + wipe + start + seed
#   ./dogfood-memory.sh status         # is it running? which ports?
#   ./dogfood-memory.sh add <body> [kind]
#   ./dogfood-memory.sh list
#   ./dogfood-memory.sh search <query>
#   ./dogfood-memory.sh get <memory_id>
#   ./dogfood-memory.sh clusters       # (Phase 1a) query TopicClusters view
#   ./dogfood-memory.sh eval           # score clusters against fixture labels
#   ./dogfood-memory.sh logs [svc]     # tail node.log or schema.log
#
# Ports and data live under $HARNESS_DIR (default: /tmp/folddb-memory-dogfood).
# Override any of these via env vars before invocation:
#   FOLDDB_DOGFOOD_DIR, FOLDDB_NODE_PORT, FOLDDB_SCHEMA_PORT
#
# This harness reuses the stock folddb_server + schema_service binaries;
# everything it does is doable over HTTP so the same commands work against
# any running node that exposes /api/memory/register.

set -u
set -o pipefail

# ── Locations ───────────────────────────────────────────────────────────
HARNESS_DIR="${FOLDDB_DOGFOOD_DIR:-/tmp/folddb-memory-dogfood}"
NODE_PORT="${FOLDDB_NODE_PORT:-19700}"
SCHEMA_PORT="${FOLDDB_SCHEMA_PORT:-19701}"

NODE_DATA="$HARNESS_DIR/node"
SCHEMA_DATA="$HARNESS_DIR/schema"
LOG_DIR="$HARNESS_DIR/logs"
PID_DIR="$HARNESS_DIR/pids"
STATE_DIR="$HARNESS_DIR/state"

NODE_URL="http://127.0.0.1:${NODE_PORT}"
SCHEMA_URL="http://127.0.0.1:${SCHEMA_PORT}"

# Workspace root (walk up from this script's dir until we hit fold_db_node)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
NODE_REPO="$(cd "$SCRIPT_DIR/../.." && pwd)"
FIXTURES="$SCRIPT_DIR/fixtures.json"

# ── Helpers ─────────────────────────────────────────────────────────────
log()  { echo "[dogfood-memory] $*"; }
die()  { echo "[dogfood-memory] ERROR: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "missing dependency: $1"; }

ensure_deps() {
    need cargo
    need curl
    need jq
}

ensure_dirs() {
    mkdir -p "$NODE_DATA" "$SCHEMA_DATA" "$LOG_DIR" "$PID_DIR" "$STATE_DIR"
}

is_up() {
    # $1 = URL prefix; checks /api/health
    curl -sf --max-time 2 "$1/api/health" >/dev/null 2>&1
}

wait_up() {
    # $1 = URL; $2 = label; waits up to ~30s
    local attempt=0
    until is_up "$1"; do
        attempt=$((attempt + 1))
        [ "$attempt" -gt 60 ] && die "$2 at $1 did not come up within 30s"
        sleep 0.5
    done
}

pid_file() { echo "$PID_DIR/$1.pid"; }

kill_pid_file() {
    local pf="$1"
    if [ -f "$pf" ]; then
        local p
        p="$(cat "$pf")"
        if kill -0 "$p" 2>/dev/null; then
            kill "$p" 2>/dev/null || true
            sleep 0.3
            kill -9 "$p" 2>/dev/null || true
        fi
        rm -f "$pf"
    fi
}

# ── Commands ────────────────────────────────────────────────────────────

cmd_build() {
    log "building folddb_server + schema_service (release not needed for dogfood)"
    (cd "$NODE_REPO" && cargo build --bin folddb_server --bin schema_service --quiet) \
        || die "cargo build failed"
}

cmd_start() {
    ensure_deps
    ensure_dirs

    if is_up "$NODE_URL" && is_up "$SCHEMA_URL"; then
        log "already running (node=$NODE_URL schema=$SCHEMA_URL)"
        cmd_seed_if_needed
        return 0
    fi

    cmd_build

    # Start schema service
    if ! is_up "$SCHEMA_URL"; then
        log "starting schema_service on :$SCHEMA_PORT"
        (
            cd "$NODE_REPO"
            nohup cargo run --quiet --bin schema_service -- \
                --port "$SCHEMA_PORT" \
                --db-path "$SCHEMA_DATA" \
                >"$LOG_DIR/schema.log" 2>&1 &
            echo $! > "$(pid_file schema)"
        )
        wait_up "$SCHEMA_URL" "schema_service"
    fi

    # Start folddb_server
    if ! is_up "$NODE_URL"; then
        log "starting folddb_server on :$NODE_PORT"
        (
            cd "$NODE_REPO"
            nohup cargo run --quiet --bin folddb_server -- \
                --port "$NODE_PORT" \
                --data-dir "$NODE_DATA" \
                --schema-service-url "$SCHEMA_URL" \
                >"$LOG_DIR/node.log" 2>&1 &
            echo $! > "$(pid_file node)"
        )
        wait_up "$NODE_URL" "folddb_server"
    fi

    # Register memory schema + seed fixtures if not seeded yet
    cmd_seed_if_needed

    log "ready"
    log "  node:   $NODE_URL"
    log "  schema: $SCHEMA_URL"
    log "  data:   $HARNESS_DIR"
}

cmd_stop() {
    log "stopping node + schema_service"
    kill_pid_file "$(pid_file node)"
    kill_pid_file "$(pid_file schema)"
}

cmd_reset() {
    cmd_stop
    log "wiping data + state dir: $HARNESS_DIR"
    rm -rf "$HARNESS_DIR"
    cmd_start
}

cmd_status() {
    local node_up schema_up
    is_up "$NODE_URL" && node_up="UP" || node_up="DOWN"
    is_up "$SCHEMA_URL" && schema_up="UP" || schema_up="DOWN"
    echo "node   ($NODE_URL): $node_up"
    echo "schema ($SCHEMA_URL): $schema_up"
    echo "data:   $HARNESS_DIR"
    echo "seeded: $([ -f "$STATE_DIR/seeded" ] && echo yes || echo no)"
    if [ -f "$STATE_DIR/canonical_name" ]; then
        echo "canonical memory schema: $(cat "$STATE_DIR/canonical_name")"
    fi
}

register_schema() {
    local resp
    resp="$(curl -sf -X POST "$NODE_URL/api/memory/register" \
        -H 'Content-Type: application/json' \
        -H 'X-User-Hash: dogfood' \
        -d '{}')" \
        || die "failed to register memory schema against $NODE_URL"
    local canonical
    canonical="$(echo "$resp" | jq -r '.canonical_name // .data.canonical_name')"
    [ -n "$canonical" ] && [ "$canonical" != "null" ] || die "register_memory_schema returned empty canonical_name: $resp"
    echo "$canonical" > "$STATE_DIR/canonical_name"
    log "memory schema canonical_name: $canonical"
}

get_canonical() {
    [ -f "$STATE_DIR/canonical_name" ] || die "no canonical name cached — run 'start' first"
    cat "$STATE_DIR/canonical_name"
}

post_memory() {
    # $1 = id, $2 = body, $3 = kind, $4 = tags JSON array string, $5 = source
    local id="$1" body="$2" kind="$3" tags="$4" source="$5"
    local canonical
    canonical="$(get_canonical)"
    local payload
    payload="$(jq -n \
        --arg schema "$canonical" \
        --arg id "$id" --arg body "$body" --arg kind "$kind" \
        --arg source "$source" \
        --argjson tags "$tags" \
        '{
            type: "mutation",
            schema: $schema,
            key_value: { hash: $id, range: null },
            mutation_type: "Create",
            fields_and_values: {
                id: $id, body: $body, kind: $kind,
                status: "live", tags: $tags, source: $source,
                created_at: (now | todate),
                derived_from: []
            }
        }')"
    curl -sf -X POST "$NODE_URL/api/mutation" \
        -H 'Content-Type: application/json' \
        -H 'X-User-Hash: dogfood' \
        -d "$payload" > /dev/null \
        || die "mutation failed for $id"
}

cmd_seed_if_needed() {
    if [ ! -f "$STATE_DIR/canonical_name" ]; then
        register_schema
    fi
    if [ -f "$STATE_DIR/seeded" ]; then
        return 0
    fi
    cmd_seed
}

cmd_seed() {
    if [ ! -f "$STATE_DIR/canonical_name" ]; then
        register_schema
    fi
    [ -f "$FIXTURES" ] || die "fixtures.json not found at $FIXTURES"
    local n
    n="$(jq '.memories | length' "$FIXTURES")"
    log "seeding $n fixture memories"

    local i=0
    while [ "$i" -lt "$n" ]; do
        local id body kind tags
        id="$(jq -r ".memories[$i].id" "$FIXTURES")"
        body="$(jq -r ".memories[$i].body" "$FIXTURES")"
        kind="$(jq -r ".memories[$i].kind" "$FIXTURES")"
        tags="$(jq -c ".memories[$i].tags" "$FIXTURES")"
        post_memory "$id" "$body" "$kind" "$tags" "fixture"
        i=$((i + 1))
    done
    touch "$STATE_DIR/seeded"
    log "seeded."
}

cmd_add() {
    local body="${1:-}" kind="${2:-note}"
    [ -n "$body" ] || die "usage: add <body> [kind]"
    local id="mem_$(date +%s)_$RANDOM"
    post_memory "$id" "$body" "$kind" '[]' "cli"
    echo "$id"
}

cmd_list() {
    local canonical
    canonical="$(get_canonical)"
    curl -sf -X POST "$NODE_URL/api/query" \
        -H 'Content-Type: application/json' \
        -H 'X-User-Hash: dogfood' \
        -d "$(jq -n --arg s "$canonical" '{
            schema_name: $s,
            fields: ["id", "body", "kind", "status", "tags", "source", "created_at"],
            filter: null
        }')" | jq '(.results // .data.results // []) | map({id: .fields.id, body: .fields.body, kind: .fields.kind, tags: .fields.tags, source: .fields.source})'
}

cmd_search() {
    local q="${1:-}"
    [ -n "$q" ] || die "usage: search <query>"
    local canonical
    canonical="$(get_canonical)"
    curl -sf --get "$NODE_URL/api/native-index/search" \
        --data-urlencode "term=$q" \
        -H 'X-User-Hash: dogfood' \
        | jq --arg canonical "$canonical" '
            (.results // .data.results // [])
            | map(select(.schema_name == $canonical))
            | map({
                id: .key_value.hash,
                field: .field,
                score: .metadata.score
            })
        '
}

cmd_get() {
    local id="${1:-}"
    [ -n "$id" ] || die "usage: get <memory_id>"
    local canonical
    canonical="$(get_canonical)"
    # HashRangeFilter by hash key
    curl -sf -X POST "$NODE_URL/api/query" \
        -H 'Content-Type: application/json' \
        -H 'X-User-Hash: dogfood' \
        -d "$(jq -n --arg s "$canonical" --arg id "$id" '{
            schema_name: $s,
            fields: ["id", "body", "kind", "status", "tags", "source", "created_at", "derived_from"],
            filter: { hash: $id }
        }')" | jq '(.results // .data.results // []) | map(.fields)'
}

cmd_clusters() {
    log "clusters command is a Phase 1a placeholder — will query TopicClusters view once registered"
    log "try: ./dogfood-memory.sh search '<topic>'  in the meantime"
    exit 2
}

cmd_eval() {
    # Baseline eval: for each non-noise fixture, run a semantic search using
    # its body and check that at least 2 other members of the same expected
    # cluster surface in the top-10 hits. Reports a per-cluster recall score.
    #
    # Not a substitute for Phase 1a's clustering test — this just validates
    # that raw semantic retrieval is working.
    local canonical
    canonical="$(get_canonical)"

    local clusters
    clusters="$(jq -r '[.memories[] | .expected_cluster] | unique | .[]' "$FIXTURES" | grep -v '^noise$')"

    echo "Cluster   | Members | Avg cluster-mates recalled in top 10"
    echo "----------|---------|-------------------------------------"

    while IFS= read -r cluster; do
        [ -z "$cluster" ] && continue
        local members
        members="$(jq -r --arg c "$cluster" '.memories[] | select(.expected_cluster == $c) | .id' "$FIXTURES")"
        local total=0 hits_total=0 count=0
        while IFS= read -r member_id; do
            [ -z "$member_id" ] && continue
            local body
            body="$(jq -r --arg id "$member_id" '.memories[] | select(.id == $id) | .body' "$FIXTURES")"
            local search_hits
            search_hits="$(curl -sf --get "$NODE_URL/api/native-index/search" \
                --data-urlencode "term=$body" \
                -H 'X-User-Hash: dogfood' \
                | jq --arg canonical "$canonical" --arg self "$member_id" \
                    '[(.results // .data.results // [])[] | select(.schema_name == $canonical) | .key_value.hash]
                     | unique
                     | map(select(. != $self))
                     | .[0:10]')"
            local this_hits
            this_hits="$(jq --argjson members "$(echo "$members" | jq -R . | jq -s .)" \
                '[.[] | select(. as $k | $members | index($k))] | length' \
                <<< "$search_hits")"
            count=$((count + 1))
            hits_total=$((hits_total + this_hits))
            total=$((total + ($(echo "$members" | wc -l | tr -d ' ') - 1)))
        done <<< "$members"
        if [ "$count" -gt 0 ]; then
            local avg
            avg="$(awk -v h="$hits_total" -v c="$count" 'BEGIN { if (c>0) printf "%.2f", h/c; else print "0" }')"
            echo "$(printf '%-10s| %-8s| %s' "$cluster" "$count" "$avg")"
        fi
    done <<< "$clusters"
}

cmd_logs() {
    local svc="${1:-node}"
    local f="$LOG_DIR/${svc}.log"
    [ -f "$f" ] || die "no log at $f (svc = node | schema)"
    tail -f "$f"
}

usage() {
    sed -n '2,22p' "${BASH_SOURCE[0]}"
}

# ── Dispatch ────────────────────────────────────────────────────────────
main() {
    local cmd="${1:-}"
    [ -n "$cmd" ] || { usage; exit 2; }
    shift || true
    case "$cmd" in
        start)    cmd_start ;;
        stop)     cmd_stop ;;
        reset)    cmd_reset ;;
        status)   cmd_status ;;
        seed)     cmd_seed ;;
        add)      cmd_add "$@" ;;
        list)     cmd_list ;;
        search)   cmd_search "$@" ;;
        get)      cmd_get "$@" ;;
        clusters) cmd_clusters ;;
        eval)     cmd_eval ;;
        logs)     cmd_logs "$@" ;;
        -h|--help|help) usage ;;
        *)        echo "unknown command: $cmd"; usage; exit 2 ;;
    esac
}

main "$@"
