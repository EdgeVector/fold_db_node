#!/usr/bin/env bash
#
# sweep-dead-slots-test.sh — unit test for sweep_dead_slots() in run.sh.
#
# Two reaper branches were added on top of PR #994: a listener-liveness probe
# that catches "wrapper alive but folddb_server died" zombies, plus a startup
# grace window that keeps the probe from racing a sibling cargo build. The
# cases below exercise both of those plus the original PR #994 branches so
# the older behavior doesn't regress.
#
# Each case:
#   1. Stands up an isolated $HOME with a synthetic ~/.folddb-slots/<port>.json,
#   2. Optionally spawns a wrapper-like proc (argv matches `run\.sh`) and/or
#      a TCP listener bound to the slot's port,
#   3. Optionally backdates the slot's mtime so the past-grace branch fires,
#   4. Calls sweep_dead_slots (sourced from run.sh), then asserts whether
#      the slot file survived and whether the wrapper got killed.
#
# Standalone — no nodes booted, no cargo build. Each case runs in a subshell
# but persists results via $TMP/_results so the parent can tally pass/fail.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TMP="$(mktemp -d -t sweep-dead-slots-test.XXXXXX)"
export TMP
: > "$TMP/_results"
: > "$TMP/_pids"

# Extract run.sh's function-definition block once, up to (but not including)
# the active-execution `trap on_exit EXIT` line. Sourcing via a temp file is
# more reliable than `source <(awk ...)` — process substitution under `()`
# subshells silently fails to populate functions on some bash builds.
RUNSH_FUNCS="$TMP/runsh-funcs.sh"
awk '/^trap on_exit EXIT$/{exit} {print}' "$ROOT/run.sh" > "$RUNSH_FUNCS"
export RUNSH_FUNCS

trap 'cleanup_all_test_state' EXIT

cleanup_all_test_state() {
    if [ -f "$TMP/_pids" ]; then
        while IFS= read -r pid; do
            [ -n "$pid" ] && kill -KILL "$pid" 2>/dev/null || true
        done < "$TMP/_pids"
    fi
    rm -rf "$TMP"
}

record_pid() { echo "$1" >> "$TMP/_pids"; }

reserve_free_port() {
    # Ask the kernel for a free port, then release. Tiny race window — fine for
    # serial tests.
    python3 -c '
import socket
s = socket.socket(); s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
'
}

# NOTE on spawn helpers:
#  - Each writes the spawned PID to a global (LAST_LISTENER_PID, LAST_WRAPPER_PID,
#    LAST_UNRELATED_PID). They deliberately do NOT echo and get captured via $(...)
#    — under command substitution, the backgrounded child gets detached/reaped
#    before the test can ps it.
#  - All FDs are redirected to /dev/null. If a long-running helper inherited the
#    test's stdout, it would hold the test runner's pipe open after the test
#    itself exited, hanging the harness (a pipeline like `bash test.sh | tail -3`
#    waits for the pipe write end to close, which is exactly what's stuck).
spawn_listener() {
    local port="$1"
    python3 -c "
import socket, time
s = socket.socket(); s.bind(('127.0.0.1', $port)); s.listen()
time.sleep(120)
" </dev/null >/dev/null 2>&1 &
    LAST_LISTENER_PID=$!
    record_pid "$LAST_LISTENER_PID"
    local i
    for i in 1 2 3 4 5 6 7 8 9 10; do
        if lsof -iTCP:"$port" -sTCP:LISTEN -t >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.2
    done
    echo "ERROR: listener on $port failed to bind" >&2
    return 1
}

spawn_run_sh_lookalike() {
    # Argv must contain literal "run.sh" so sweep_dead_slots's allowlist matches.
    local stub="$TMP/run.sh-stub"
    if [ ! -x "$stub" ]; then
        cat > "$stub" <<'EOF'
#!/usr/bin/env bash
sleep 120
EOF
        chmod +x "$stub"
    fi
    bash "$stub" </dev/null >/dev/null 2>&1 &
    LAST_WRAPPER_PID=$!
    record_pid "$LAST_WRAPPER_PID"
}

spawn_unrelated_proc() {
    # ps -p will show `sleep 120` — no run.sh substring. Models the PR #994 case.
    sleep 120 </dev/null >/dev/null 2>&1 &
    LAST_UNRELATED_PID=$!
    record_pid "$LAST_UNRELATED_PID"
}

write_slot() {
    local port="$1" pid="$2" home_dir="$3" age="${4:-young}"
    local slot_file="$HOME/.folddb-slots/$port.json"
    mkdir -p "$HOME/.folddb-slots"
    cat > "$slot_file" <<EOF
{"port": $port, "schema_port": $((port+1)), "vite_port": 5173, "home": "$home_dir", "pid": $pid}
EOF
    if [ "$age" = "old" ]; then
        # Backdate by 10 minutes — well past SLOT_STARTUP_GRACE_SECONDS=180.
        local stamp
        stamp="$(date -v -10M '+%Y%m%d%H%M' 2>/dev/null || date -d '10 minutes ago' '+%Y%m%d%H%M')"
        touch -t "$stamp" "$slot_file"
    fi
}

expect_eq() {
    local name="$1" want="$2" got="$3"
    if [ "$want" = "$got" ]; then
        printf 'ok   %s\n' "$name"
        echo "P" >> "$TMP/_results"
    else
        printf 'FAIL %s: want=%s got=%s\n' "$name" "$want" "$got"
        echo "F" >> "$TMP/_results"
    fi
}

# Run one case in its own subshell with an isolated $HOME and a freshly
# sourced copy of run.sh's function definitions. Each case appends pass/fail
# markers to $TMP/_results, which the parent tallies at the end.
run_case() {
    local case_name="$1" setup_fn="$2" assertion_fn="$3"
    (
        local case_home
        case_home="$(mktemp -d "$TMP/home-${case_name}.XXXXXX")"
        export HOME="$case_home"

        # Source the pre-extracted run.sh function block. (See note at the
        # top about why we don't `source <(awk ...)` here directly.)
        # shellcheck disable=SC1090
        source "$RUNSH_FUNCS"

        "$setup_fn"
        # Surface sweep output during a failing run by setting SWEEP_DEBUG=1.
        if [ "${SWEEP_DEBUG:-0}" = "1" ]; then
            sweep_dead_slots
        else
            sweep_dead_slots >/dev/null 2>&1 || true
        fi
        "$assertion_fn"
    )
}

# ----------------------------------------------------------------------------
# Case 1: wrapper alive + home present + listener bound → preserve
# ----------------------------------------------------------------------------
case1_setup() {
    CASE1_PORT="$(reserve_free_port)"
    CASE1_HOME="$HOME/c1-home"
    mkdir -p "$CASE1_HOME"
    spawn_listener "$CASE1_PORT"
    spawn_run_sh_lookalike
    CASE1_WRAPPER="$LAST_WRAPPER_PID"
    write_slot "$CASE1_PORT" "$CASE1_WRAPPER" "$CASE1_HOME" young
}
case1_assert() {
    if [ -f "$HOME/.folddb-slots/$CASE1_PORT.json" ]; then
        expect_eq "case1-listener-bound-young: slot preserved" "yes" "yes"
    else
        expect_eq "case1-listener-bound-young: slot preserved" "yes" "no"
    fi
    if kill -0 "$CASE1_WRAPPER" 2>/dev/null; then
        expect_eq "case1-listener-bound-young: wrapper alive" "yes" "yes"
    else
        expect_eq "case1-listener-bound-young: wrapper alive" "yes" "no"
    fi
}

# ----------------------------------------------------------------------------
# Case 2: wrapper alive + home present + listener NOT bound + slot young →
#         preserve (startup grace covers sibling cargo build)
# ----------------------------------------------------------------------------
case2_setup() {
    CASE2_PORT="$(reserve_free_port)"
    CASE2_HOME="$HOME/c2-home"
    mkdir -p "$CASE2_HOME"
    spawn_run_sh_lookalike
    CASE2_WRAPPER="$LAST_WRAPPER_PID"
    write_slot "$CASE2_PORT" "$CASE2_WRAPPER" "$CASE2_HOME" young
}
case2_assert() {
    if [ -f "$HOME/.folddb-slots/$CASE2_PORT.json" ]; then
        expect_eq "case2-listener-unbound-young: slot preserved (in grace)" "yes" "yes"
    else
        expect_eq "case2-listener-unbound-young: slot preserved (in grace)" "yes" "no"
    fi
    if kill -0 "$CASE2_WRAPPER" 2>/dev/null; then
        expect_eq "case2-listener-unbound-young: wrapper alive" "yes" "yes"
    else
        expect_eq "case2-listener-unbound-young: wrapper alive" "yes" "no"
    fi
}

# ----------------------------------------------------------------------------
# Case 3: wrapper alive + home present + listener NOT bound + slot old → REAP
#         (the zombie-wrapper case — this is the new branch we're adding)
# ----------------------------------------------------------------------------
case3_setup() {
    CASE3_PORT="$(reserve_free_port)"
    CASE3_HOME="$HOME/c3-home"
    mkdir -p "$CASE3_HOME"
    spawn_run_sh_lookalike
    CASE3_WRAPPER="$LAST_WRAPPER_PID"
    write_slot "$CASE3_PORT" "$CASE3_WRAPPER" "$CASE3_HOME" old
}
case3_assert() {
    if [ -f "$HOME/.folddb-slots/$CASE3_PORT.json" ]; then
        expect_eq "case3-zombie-wrapper: slot reaped" "yes" "no"
    else
        expect_eq "case3-zombie-wrapper: slot reaped" "yes" "yes"
    fi
    # Give TERM→KILL a moment to settle.
    sleep 1
    if kill -0 "$CASE3_WRAPPER" 2>/dev/null; then
        expect_eq "case3-zombie-wrapper: wrapper killed" "yes" "no"
    else
        expect_eq "case3-zombie-wrapper: wrapper killed" "yes" "yes"
    fi
}

# ----------------------------------------------------------------------------
# Case 4: PR #994 regression — PID gone → reap
# ----------------------------------------------------------------------------
case4_setup() {
    CASE4_PORT="$(reserve_free_port)"
    CASE4_HOME="$HOME/c4-home"
    mkdir -p "$CASE4_HOME"
    # 999998 should be very unlikely to map to anything live.
    write_slot "$CASE4_PORT" "999998" "$CASE4_HOME" young
}
case4_assert() {
    if [ -f "$HOME/.folddb-slots/$CASE4_PORT.json" ]; then
        expect_eq "case4-dead-pid: slot reaped" "yes" "no"
    else
        expect_eq "case4-dead-pid: slot reaped" "yes" "yes"
    fi
}

# ----------------------------------------------------------------------------
# Case 5: PR #994 regression — PID reused by unrelated proc → reap
# ----------------------------------------------------------------------------
case5_setup() {
    CASE5_PORT="$(reserve_free_port)"
    CASE5_HOME="$HOME/c5-home"
    mkdir -p "$CASE5_HOME"
    spawn_unrelated_proc
    CASE5_PID="$LAST_UNRELATED_PID"
    write_slot "$CASE5_PORT" "$CASE5_PID" "$CASE5_HOME" young
}
case5_assert() {
    if [ -f "$HOME/.folddb-slots/$CASE5_PORT.json" ]; then
        expect_eq "case5-pid-reused: slot reaped" "yes" "no"
    else
        expect_eq "case5-pid-reused: slot reaped" "yes" "yes"
    fi
}

# ----------------------------------------------------------------------------
# Case 6: wrapper alive + home present + listener bound + slot OLD → preserve
#         (long-running healthy session should not be reaped just for age)
# ----------------------------------------------------------------------------
case6_setup() {
    CASE6_PORT="$(reserve_free_port)"
    CASE6_HOME="$HOME/c6-home"
    mkdir -p "$CASE6_HOME"
    spawn_listener "$CASE6_PORT"
    spawn_run_sh_lookalike
    CASE6_WRAPPER="$LAST_WRAPPER_PID"
    write_slot "$CASE6_PORT" "$CASE6_WRAPPER" "$CASE6_HOME" old
}
case6_assert() {
    if [ -f "$HOME/.folddb-slots/$CASE6_PORT.json" ]; then
        expect_eq "case6-long-running-healthy: slot preserved" "yes" "yes"
    else
        expect_eq "case6-long-running-healthy: slot preserved" "yes" "no"
    fi
    if kill -0 "$CASE6_WRAPPER" 2>/dev/null; then
        expect_eq "case6-long-running-healthy: wrapper alive" "yes" "yes"
    else
        expect_eq "case6-long-running-healthy: wrapper alive" "yes" "no"
    fi
}

run_case "case1" case1_setup case1_assert
run_case "case2" case2_setup case2_assert
run_case "case3" case3_setup case3_assert
run_case "case4" case4_setup case4_assert
run_case "case5" case5_setup case5_assert
run_case "case6" case6_setup case6_assert

PASS_COUNT="$(grep -c '^P$' "$TMP/_results" || true)"
FAIL_COUNT="$(grep -c '^F$' "$TMP/_results" || true)"
echo "---"
echo "passed: $PASS_COUNT  failed: $FAIL_COUNT"
[ "$FAIL_COUNT" -eq 0 ]
