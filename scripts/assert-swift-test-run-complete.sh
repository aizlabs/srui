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
runner_status_file=$(mktemp "${TMPDIR:-/tmp}/srui-swift-test-status.XXXXXX")
trap 'rm -f "$log" "$plain" "$runner_status_file"' EXIT

# `script` gives the run a pty so its output stays line-buffered. A wedged run that the watchdog
# kills would otherwise lose everything still sitting in stdio's block buffer, and the accounting
# below would have nothing to read.
# `run-swift-tests.sh` samples `swift-test`, the parent that only reads the helper's pipes. When
# the stall is inside the test host itself the useful stacks are in `swiftpm-testing-helper`, so
# sample that process a little before the inner watchdog kills the group.
timeout_seconds=${SRUI_TEST_TIMEOUT:-300}
sample_at=$(( timeout_seconds > 45 ? timeout_seconds - 30 : timeout_seconds ))
(
    sleep "$sample_at"
    helper=$(pgrep -x swiftpm-testing-helper 2>/dev/null | head -1)
    [ -z "$helper" ] && exit 0
    echo "=== stalled test host: sample of swiftpm-testing-helper pid=$helper ===" >&2
    sample "$helper" 2 -mayDie 2>/dev/null >&2
    echo "=== end sample ===" >&2
) &
sampler_pid=$!

# The runner's exit status comes back through a file it writes itself, not through `script`.
# `script(1)` owns its own exit status: it is the *typescript* utility's, and the only reason the
# child's shows through on macOS is an implementation detail of `finish()` (it happens to call
# `done(WEXITSTATUS(status))`, and `done(0)` when the `waitpid` does not match). util-linux needs
# `-e` for the same thing, and macOS accepts `-e` only "for compatibility". A green wrapper is the
# one thing this script must never produce by accident: the accounting below deliberately counts a
# *failing* test as reported, so if `status` were 0 for a run whose tests failed, every check here
# would pass and CI would call the run green.
# `script(1)` has two incompatible dialects, and this repository is developed on both. BSD (macOS)
# takes the command as trailing positional words - `script [-q] file command [args...]` - and has
# no `-c` at all. util-linux takes `script [options] [file]` and rejects that trailing command
# outright, so the BSD spelling fails before the runner ever starts. Pick by asking the binary
# which one it is, and hand util-linux the command through `-c` as a single string; `-e` makes its
# exit status the child's, which BSD does unconditionally.
run_command='scripts/run-swift-tests.sh'
for arg in "$@"; do
    run_command+=" $(printf '%q' "$arg")"
done
run_command+='; echo "$?" >"$SRUI_RUN_STATUS_FILE"'

export SRUI_RUN_STATUS_FILE="$runner_status_file"
# Captured, then matched without a pipe. Under `set -o pipefail` a `cmd | grep -q` reports 141:
# `grep -q` exits on its first match and the producer dies of SIGPIPE, so the pipeline "fails"
# precisely when the match succeeds - which would silently select the BSD spelling on Linux and
# reintroduce the bug this detection exists to avoid. `tr` consumes all of its input, so it cannot
# lose the same way.
script_version=$(script --version 2>/dev/null | tr '[:upper:]' '[:lower:]')
if case $script_version in *util-linux*) true ;; *) false ;; esac; then
    # Invoke bash explicitly: util-linux runs `-c` through `$SHELL`, and the quoting above is
    # bash's own.
    script -q -e -c "/bin/bash -c $(printf '%q' "$run_command")" "$log" >/dev/null 2>&1
else
    script -q "$log" /bin/bash -c "$run_command" >/dev/null 2>&1
fi
script_status=$?

status=$(LC_ALL=C tr -dc '0-9' <"$runner_status_file")
missing_status=0
if [ -z "$status" ]; then
    # The runner never returned: `script` died, or the whole group was killed. Fail closed.
    missing_status=1
    status=$script_status
    [ "$status" -eq 0 ] && status=1
fi
kill "$sampler_pid" 2>/dev/null
wait "$sampler_pid" 2>/dev/null

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
if [ "$missing_status" -ne 0 ]; then
    echo "MISSING STATUS: the test runner never reported an exit status (script exited ${script_status})." >&2
    problems=1
fi
# Advisory, not fatal - and that distinction is measured, not assumed.
#
# The capture layer loses lines. On a macos-15 runner this suite reported 588 started and 537
# result lines for a run whose own summary said `578 tests passed`: no skips, no interleaved
# writes (zero lines carried two markers), no truncated tail (the summary is the last line), so
# ~51 result lines were dropped by `script(1)` while the burst was being written. Failing on that
# would turn this guard into noise that every CI run trips over, which is how a guard gets ignored.
#
# Nothing real is lost by demoting it. The failures this script exists for are all still fatal
# below: a host that exits mid-run prints no swift-testing summary, and a host that is killed
# leaves no runner status. The per-test tally remains the most useful diagnostic when either of
# those fires, so it is still computed and still printed.
if [ -n "$missing" ]; then
    missing_count=$(printf '%s\n' "$missing" | grep -c . || true)
    echo "NOTE: ${missing_count} test(s) started without a matching result line in the capture." >&2
    printf '%s\n' "$missing" | sed 's/^/  /' | head -20 >&2
    [ "$missing_count" -gt 20 ] && echo "  ... and $((missing_count - 20)) more" >&2
    echo "NOTE: advisory only - see the comment above this check. The summaries below are what" >&2
    echo "      decides the run." >&2
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
    echo "swift test exited ${status}, but this run cannot be called green: the evidence above is" >&2
    echo "missing, not merely incomplete. A host that exits mid-run prints no summary; a host that" >&2
    echo "is killed leaves no runner status. See scripts/check-test-main-runloop-nesting.sh for the" >&2
    echo "mechanisms that do this." >&2
    exit 1
fi

exit "$status"
