#!/usr/bin/env bash
# Render a per-scenario / per-assertion summary of an E2E nightly run for
# GitHub Actions. Reads each <logs-dir>/run-*/run.log produced by
# run-scenario.sh, emits a markdown table to stdout, and emits ::error::
# workflow annotations to stderr for failed assertions, failed steps, and
# incomplete (e.g. timed-out) runs.
#
# The split is intentional:
#   - stdout → workflow summary file (caller redirects to $GITHUB_STEP_SUMMARY)
#   - stderr → job log, where GitHub Actions parses ::error:: lines and
#              promotes them to top-of-page annotations
#
# A single recurring assertion failure (e.g. bob.shared_record_count
# regressing every nightly for 3 weeks) thus shows up as a named annotation
# on every run instead of a generic "scenario failed" buried in scrollback.
#
# Usage:
#   render-ci-summary.sh <logs-dir>           # human + CI
#   render-ci-summary.sh tests/fixtures/...   # local smoke test
set -euo pipefail

LOGS_DIR="${1:?usage: render-ci-summary.sh <logs-dir>}"

shopt -s nullglob

# Count lines matching a grep ERE pattern. Always prints a non-empty
# integer: grep -c emits "0" on no match but exits 1, so we ||true to keep
# set -e callers happy.
count_matches() {
  local pattern="$1" file="$2"
  grep -cE "$pattern" "$file" || true
}

# Emit a workflow annotation. GitHub parses these from the job log; routing
# to stderr keeps them out of the markdown summary on stdout.
annotate_error() {
  local file="$1" message="$2"
  echo "::error file=${file}::${message}" >&2
}

print_run() {
  local run_dir="$1"
  local log="$run_dir/run.log"
  if [[ ! -f "$log" ]]; then
    echo "### $(basename "$run_dir") — ⚠️ no run.log"
    echo ""
    return 0
  fi

  local scenario overall failing_step pass_count fail_count scenario_path

  # First [run] running ... line names the scenario for this run dir.
  scenario=$(grep -m1 -E '^\[run\] running ' "$log" | sed -E 's/.*running //' || true)
  scenario="${scenario:-(unknown)}"
  scenario_path="test-framework/${scenario}"

  if grep -q '^\[run\] ✅ SCENARIO PASSED' "$log"; then
    overall="✅ PASS"
  elif grep -q '^\[run\] ❌ SCENARIO FAILED' "$log"; then
    overall="❌ FAIL"
  else
    # No PASSED/FAILED marker means the script was killed mid-run (timeout,
    # SIGTERM, or hard crash before assertion phase). Surface as INCOMPLETE
    # so it's not silently classified as PASS.
    overall="⚠️ INCOMPLETE"
  fi

  failing_step=""
  if grep -q '^\[run\] STEPS FAILED' "$log"; then
    failing_step=$(grep -m1 -E '^\[steps\] step .* FAILED on role=' "$log" \
      | sed -E 's/^\[steps\] step ([^ ]+) FAILED on role=([^ ]+) action\[([0-9]+)\].*/\1 (role=\2 action[\3])/' || true)
  fi

  pass_count=$(count_matches '^\[PASS\] ' "$log")
  fail_count=$(count_matches '^\[FAIL\] ' "$log")

  echo "### $scenario — $overall"
  echo ""
  echo "| Item | Detail |"
  echo "|---|---|"
  echo "| Run dir | \`$(basename "$run_dir")\` |"
  if [[ -n "$failing_step" ]]; then
    echo "| Step failure | \`$failing_step\` |"
  elif [[ "$overall" == "⚠️ INCOMPLETE" ]]; then
    echo "| Steps | run aborted before completion (no SCENARIO PASSED/FAILED marker) |"
  else
    echo "| Steps | all complete |"
  fi
  echo "| Assertions | $pass_count passed, $fail_count failed |"
  echo ""

  if grep -qE '^\[(PASS|FAIL)\] ' "$log"; then
    echo "<details><summary>Assertion lines</summary>"
    echo ""
    echo '```'
    grep -E '^\[(PASS|FAIL)\] ' "$log"
    echo '```'
    echo "</details>"
    echo ""
  fi

  # Workflow annotations: one per failed step + one per failed assertion +
  # one for incomplete runs. file= points at the scenario YAML so the GH UI
  # links there.
  if [[ -n "$failing_step" ]]; then
    annotate_error "$scenario_path" "step failed: $failing_step"
  fi
  if [[ "$overall" == "⚠️ INCOMPLETE" ]]; then
    annotate_error "$scenario_path" "scenario INCOMPLETE — no PASSED/FAILED marker (likely timeout or SIGTERM)"
  fi
  while IFS= read -r line; do
    [[ -z "$line" ]] && continue
    annotate_error "$scenario_path" "assertion failed: ${line#\[FAIL\] }"
  done < <(grep -E '^\[FAIL\] ' "$log" || true)
}

ANY=0
for run_dir in "$LOGS_DIR"/run-*; do
  [[ -d "$run_dir" ]] || continue
  ANY=1
  print_run "$run_dir"
done

if [[ "$ANY" == "0" ]]; then
  echo "_No \`run-*\` directories under \`$LOGS_DIR\` — scenarios may not have started._"
fi
