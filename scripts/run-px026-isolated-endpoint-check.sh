#!/usr/bin/env bash
# PX-026: the automation test's strict process-wide listener diff, run where nothing else shares
# the test process.
#
# `automationLocatesControlsSemanticallyAndTakesTheNativeClicksWireAndSettlementPath` lists this
# test process's listening sockets with lsof before it makes the T35 inspector and after its last
# automated action. In a full `swift test` other suites run concurrently in the same process and
# open their own listeners (ConnectionFakeSSHServerTests' TCP server, TransportTests' unix
# sockets), so there the test only reports the diff. This script runs the three PX-026 automation
# tests alone, sets SRUI_PX026_ISOLATED_ENDPOINT_CHECK=1 so the test asserts the diff is empty,
# and then fails unless the run is provably isolated: exactly those 3 swift-testing tests in the
# one suite `ProcessExplorerShellTests`, no XCTest, and the strict check's own log line once.
#
# Usage: scripts/run-px026-isolated-endpoint-check.sh [extra swift-test args...]
#   SRUI_TEST_TIMEOUT   forwarded to scripts/run-swift-tests.sh (default 600 here)
set -uo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

tests=(
    automationLocatesControlsSemanticallyAndTakesTheNativeClicksWireAndSettlementPath
    aDisabledOrUnsupportedControlRefusesAutomationExactlyAsItRefusesANativeClick
    automationSelectsAProcessRowAndReadsTheInspectorSemantically
)
filter="ProcessExplorerShellTests/($(IFS='|'; echo "${tests[*]}"))"
strict_line='PX-026 isolated endpoint check: strict process-wide listener diff asserted'

log=$(mktemp "${TMPDIR:-/tmp}/srui-px026-endpoint.XXXXXX")
trap 'rm -f "$log"' EXIT

# Into a file, not a pipe: run-swift-tests.sh's watchdog `sleep` outlives a finished run while
# still holding the stdout it inherited, so a reader such as `tee` would wait out the whole timeout.
SRUI_PX026_ISOLATED_ENDPOINT_CHECK=1 SRUI_TEST_TIMEOUT="${SRUI_TEST_TIMEOUT:-600}" \
    bash scripts/run-swift-tests.sh --filter "$filter" "$@" > "$log" 2>&1
status=$?
cat "$log"

problems=()
[ "$status" -eq 0 ] || problems+=("the run exited $status")
grep -q "Test run with ${#tests[@]} tests in 1 suite passed" "$log" \
    || problems+=("the run was not exactly ${#tests[@]} passing tests in 1 suite")
# Every swift-testing suite that started or finished in this process, display names included.
suites=$(grep -oE '(◇|✔|✘) Suite ("[^"]*"|[^ ]+) (started|passed|failed)' "$log" \
    | sed -E 's/^[^ ]+ Suite //; s/ (started|passed|failed)$//' | sort -u | tr '\n' ' ')
[ "$suites" = "ProcessExplorerShellTests " ] \
    || problems+=("suites in this process: '${suites}', expected only ProcessExplorerShellTests")
if grep -E 'Executed [0-9]+ tests' "$log" | grep -qvE 'Executed 0 tests'; then
    problems+=("XCTest tests ran in the same process")
fi
for name in "${tests[@]}"; do
    grep -qE "Test ${name}\(\) passed" "$log" || problems+=("${name} did not pass")
done
strict_count=$(grep -c "$strict_line" "$log" || true)
[ "$strict_count" -eq 1 ] || problems+=("the strict listener check ran ${strict_count} times, expected once")

if [ "${#problems[@]}" -ne 0 ]; then
    printf 'error: PX-026 isolated endpoint check failed: %s\n' "${problems[@]}" >&2
    exit 1
fi
echo "PX-026 isolated endpoint check passed: ${#tests[@]} tests, suite ProcessExplorerShellTests only, no XCTest, strict listener diff asserted once."
