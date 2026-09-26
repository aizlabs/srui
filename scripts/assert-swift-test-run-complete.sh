#!/usr/bin/env bash
# Run the client-macos suite and refuse to call it green unless every test it started reported.
#
# Why this exists: `swift test` can exit 0 with hundreds of tests still in flight. The test host's
# `main` is `swift_task_asyncMainDrainQueue`, which under Swift 6.2 runs `CFMainExecutor.run()` ->
# `CFRunLoopRun()`. Anything that stops or nests that run loop (a `performClick(_:)` in a test, an
# AppKit modal session, a main-thread `waitUntilExit()`) can make `CFRunLoopRun()` return; `main`
# then returns and the process exits 0. swift-testing never prints its summary, the tests still
# running never report a result, and SwiftPM calls the run a success. A run measured on this repo
# started 749 tests and reported 414 - a false green that hid two real defects in an open PR.
#
# `scripts/check-test-main-runloop-nesting.sh` bans the known triggers. This script is the
# backstop: it makes the *symptom* fail loudly, whatever the next trigger turns out to be.
#
# Usage: scripts/assert-swift-test-run-complete.sh [extra swift-test args...]
#   SRUI_TEST_TIMEOUT   forwarded to scripts/run-swift-tests.sh (default 300)
set -uo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

log=$(mktemp "${TMPDIR:-/tmp}/srui-swift-test-log.XXXXXX")
plain=$(mktemp "${TMPDIR:-/tmp}/srui-swift-test-plain.XXXXXX")
trap 'rm -f "$log" "$plain"' EXIT

# `script` gives the run a pty so its output stays line-buffered. A wedged run that the watchdog
# kills would otherwise lose everything still sitting in stdio's block buffer, and the accounting
# below would have nothing to read.
script -q "$log" scripts/run-swift-tests.sh "$@" >/dev/null 2>&1
status=$?

# The pty also makes swift-testing colourise its output, so strip CRs and ANSI SGR sequences
# before matching. `cat` replays the original, colour and all, for the human reading the log.
cat "$log"
tr -d '\r' < "$log" | LC_ALL=C sed -E $'s/\x1b\\[[0-9;]*[A-Za-z]//g' > "$plain"

# Names of tests that announced themselves. A parameterized test case is an argument of its test,
# not a test of its own, so `Test case` lines are excluded from both sides.
started=$(LC_ALL=C grep -a 'Test .* started\.$' "$plain" \
    | LC_ALL=C grep -av 'Test case ' \
    | LC_ALL=C sed -E -e 's/^\xe2\x97\x87 Test //' -e 's/^Test Case //' -e 's/ started\.$//' \
    | sort -u)

# Names of tests that reported a result. swift-testing reports a parameterized test once, as
# `name(arg:) with N test cases passed after ...`; XCTest reports `Test Case 'x' passed (0.1s)`.
reported=$(LC_ALL=C grep -aE 'Test .* (passed|failed) (after|\()' "$plain" \
    | LC_ALL=C grep -av 'Test case ' \
    | LC_ALL=C sed -E \
        -e 's/^(\xe2\x9c\x94|\xe2\x9c\x98) Test run with .*$/run/' \
        -e 's/^(\xe2\x9c\x94|\xe2\x9c\x98) Test //' \
        -e 's/^Test Case //' \
        -e 's/ with [0-9]+ test cases? (passed|failed) after.*$//' \
        -e 's/ (passed|failed) after.*$//' \
        -e 's/ (passed|failed) \(.*$//' \
    | sort -u)

missing=$(comm -23 <(printf '%s\n' "$started") <(printf '%s\n' "$reported"))
started_count=$(printf '%s\n' "$started" | grep -c . || true)
reported_count=$(printf '%s\n' "$reported" | grep -c . || true)
echo "run accounting: ${started_count} test(s) started, ${reported_count} reported."

problems=0
if [ -n "$missing" ]; then
    echo "MISSING RESULT - these tests started and never reported:" >&2
    printf '%s\n' "$missing" | sed 's/^/  /' >&2
    problems=1
fi
if [ "$started_count" -eq 0 ]; then
    echo "NO TESTS RAN: refusing to report a green run." >&2
    problems=1
fi
if ! LC_ALL=C grep -aqE 'Test run with [0-9]+ test' "$plain"; then
    echo 'MISSING SUMMARY: swift-testing never printed its "Test run with N tests" line.' >&2
    problems=1
fi
if ! LC_ALL=C grep -aqE 'Executed [0-9]+ tests?,' "$plain"; then
    echo 'MISSING SUMMARY: XCTest never printed its "Executed N tests" line.' >&2
    problems=1
fi

if [ "$problems" -ne 0 ]; then
    echo >&2
    echo "swift test exited ${status}, but the run did not account for every test it started." >&2
    echo "The test host terminated or stalled while tests were in flight; see" >&2
    echo "scripts/check-test-main-runloop-nesting.sh for the mechanism." >&2
    exit 1
fi

exit "$status"
