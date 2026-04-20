#!/usr/bin/env bash
#
# qa-harness-dogfood-verdict-test.sh — unit test for the verdict-computation
# block in scripts/qa-harness-dogfood.sh. Covers aee77: a FAIL org-sync leg
# must flip VERDICT to FAIL; before the fix it was a silent skip.
#
# Runs the verdict case-block against canned (RESULT_LINES, SET_ORG_HASH_RESULT,
# ORG_RESULT) tuples and asserts the expected VERDICT. No nodes booted.

set -euo pipefail

compute_verdict() {
  # Mirrors the verdict block in scripts/qa-harness-dogfood.sh. Any change to
  # the harness verdict logic MUST be mirrored here, and vice versa.
  local fail_count="$1" set_org_hash_result="$2" org_result="$3"
  local verdict="PASS"
  [ "$fail_count" -gt 0 ] && verdict="FAIL"
  case "$set_org_hash_result" in
    FAIL\|*) verdict="FAIL" ;;
  esac
  case "$org_result" in
    FAIL\|*) verdict="FAIL" ;;
  esac
  printf '%s\n' "$verdict"
}

PASS_COUNT=0
FAIL_COUNT=0

expect() {
  local name="$1" want="$2" got="$3"
  if [ "$want" = "$got" ]; then
    printf 'ok  %s\n' "$name"
    PASS_COUNT=$((PASS_COUNT + 1))
  else
    printf 'FAIL %s: want=%s got=%s\n' "$name" "$want" "$got"
    FAIL_COUNT=$((FAIL_COUNT + 1))
  fi
}

# Baseline: everything clean → PASS.
expect "all-pass" "PASS" \
  "$(compute_verdict 0 "PASS|..." "PENDING|...")"

# Per-source failure → FAIL.
expect "source-fail" "FAIL" \
  "$(compute_verdict 1 "PASS|..." "PENDING|...")"

# Set-org-hash FAIL → FAIL (249d8 gate).
expect "set-org-hash-fail" "FAIL" \
  "$(compute_verdict 0 "FAIL|3e063 regressed" "PENDING|...")"

# Org-leg FAIL (node B boot) → FAIL. This is the aee77 regression the test
# is here to pin: before the fix, this case slipped through silently.
expect "org-leg-fail-boot" "FAIL" \
  "$(compute_verdict 0 "PASS|..." "FAIL|node B boot failed: run.sh exited before slot info appeared; tail: error ...")"

# Org-leg FAIL (backend unreachable after boot) → FAIL.
expect "org-leg-fail-unreachable" "FAIL" \
  "$(compute_verdict 0 "PASS|..." "FAIL|node B backend not reachable after boot")"

# Org-leg PENDING (happy path: second node booted, real assertion in e2e) → PASS.
expect "org-leg-pending" "PASS" \
  "$(compute_verdict 0 "PASS|..." "PENDING|node B live; real two-node org round-trip runs in e2e")"

# Org-leg SKIP (--skip-org) → PASS. An explicit opt-out is not a regression.
expect "org-leg-skip" "PASS" \
  "$(compute_verdict 0 "PASS|..." "SKIP|org leg skipped (--skip-org)")"

# Set-org-hash SKIP (source prerequisites missing) → PASS. SKIP is not FAIL.
expect "set-org-hash-skip" "PASS" \
  "$(compute_verdict 0 "SKIP|prerequisites missing" "PENDING|...")"

# Multiple failures compose to FAIL.
expect "all-fail" "FAIL" \
  "$(compute_verdict 2 "FAIL|..." "FAIL|...")"

printf '\n%d passed, %d failed\n' "$PASS_COUNT" "$FAIL_COUNT"
[ "$FAIL_COUNT" -eq 0 ]
