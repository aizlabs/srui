#!/usr/bin/env bash
#
# test_assert_swift_test_run_complete.sh
# Exit-status and accounting tests for scripts/assert-swift-test-run-complete.sh.
#
# The wrapper's whole job is to refuse to call a run green unless it really was. Two ways it could
# fail open, both covered here: swallowing the test runner's non-zero exit status, and never
# learning that status at all. Each case runs the *real* wrapper inside a sandbox git repository
# whose `scripts/run-swift-tests.sh` is a stand-in that prints a chosen transcript and exits with a
# chosen status, so no Swift is built and nothing on the host is touched.
#
# Cases 3 and 4 put a `script(1)` stand-in ahead of the real one on `PATH`: one that discards the
# child's status (util-linux's behaviour without `-e`, and what macOS's man page promises in the
# absence of an EXIT STATUS contract), and one that never runs the child at all. The wrapper must
# report a failing run in both.
#
# Run: bash scripts/test_assert_swift_test_run_complete.sh

set -uo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
wrapper="$repo_root/scripts/assert-swift-test-run-complete.sh"

sandbox=$(mktemp -d "${TMPDIR:-/tmp}/assert-run-complete-selftest.XXXXXX")
trap 'rm -rf -- "$sandbox"' EXIT
mkdir -p "$sandbox/scripts" "$sandbox/bin"
git -C "$sandbox" init -q
cp "$wrapper" "$sandbox/scripts/assert-swift-test-run-complete.sh"

failures=0
pass() { echo "  ok: $1"; }
fail() {
    echo "  FAIL: $1" >&2
    failures=$((failures + 1))
}

# --- sandbox doubles ----------------------------------------------------------------------------

# Stands in for `scripts/run-swift-tests.sh`. `FAKE_RUNNER_SHAPE` picks the transcript:
#   complete-pass  every test that started reported, both summary lines printed
#   complete-fail  the same, with the failing verdicts a real failing run prints
#   truncated      a test started and the host died before it reported anything
cat >"$sandbox/scripts/run-swift-tests.sh" <<'SH'
#!/bin/sh
# The build banner is not decoration: `script` writes the pty's VEOF when its own stdin is not a
# terminal (CI, and this test), and the pty echoes `^D\b\b` into the first line of the typescript.
# A real run spends that line on SwiftPM's build output, so the transcript does too.
printf '%s\n' "Building for debugging..."
printf '%s\n' "Build complete! (0.42s)"
case ${FAKE_RUNNER_SHAPE:-complete-pass} in
    complete-pass)
        printf '%s\n' "Test Case '-[SRUITests.FakeTests testOne]' started."
        printf '%s\n' "Test Case '-[SRUITests.FakeTests testOne]' passed (0.001 seconds)."
        printf '%s\n' "Executed 1 test, with 0 failures (0 unexpected) in 0.001 (0.001) seconds"
        printf '%s\n' "◇ Test fakeExample() started."
        printf '%s\n' "✔ Test fakeExample() passed after 0.001 seconds."
        printf '%s\n' "✔ Test run with 1 test passed after 0.002 seconds."
        ;;
    complete-fail)
        printf '%s\n' "Test Case '-[SRUITests.FakeTests testOne]' started."
        printf '%s\n' "Test Case '-[SRUITests.FakeTests testOne]' failed (0.001 seconds)."
        printf '%s\n' "Executed 1 test, with 1 failure (0 unexpected) in 0.001 (0.001) seconds"
        printf '%s\n' "◇ Test fakeExample() started."
        printf '%s\n' "✘ Test fakeExample() failed after 0.001 seconds."
        printf '%s\n' "✘ Test run with 1 test failed after 0.002 seconds."
        ;;
    swift-testing-died)
        # The shape of the real failure this guard was built for: the XCTest half finishes and
        # summarises, while the swift-testing host dies mid-run and never prints its own summary.
        # Measured on CI at the time: 723 started, 149 reported, XCTest's `Executed 149 tests` line
        # present, no `Test run with N tests` line at all.
        printf '%s\n' "Test Case '-[SRUITests.FakeTests testOne]' started."
        printf '%s\n' "Test Case '-[SRUITests.FakeTests testOne]' passed (0.001 seconds)."
        printf '%s\n' "Executed 1 test, with 0 failures (0 unexpected) in 0.001 (0.001) seconds"
        printf '%s\n' "\xe2\x97\x87 Test fakeExample() started."
        printf '%s\n' "\xe2\x9c\x94 Test fakeExample() passed after 0.001 seconds."
        printf '%s\n' "\xe2\x97\x87 Test fakeSecond() started."
        ;;
    dropped-result)
        # Both tests ran and both summaries were printed; one result line never made it into the
        # typescript, exactly as the capture layer loses them under load.
        printf '%s\n' "Test Case '-[SRUITests.FakeTests testOne]' started."
        printf '%s\n' "Test Case '-[SRUITests.FakeTests testOne]' passed (0.001 seconds)."
        printf '%s\n' "Executed 1 test, with 0 failures (0 unexpected) in 0.001 (0.001) seconds"
        printf '%s\n' "\xe2\x97\x87 Test fakeExample() started."
        printf '%s\n' "\xe2\x97\x87 Test fakeDropped() started."
        printf '%s\n' "\xe2\x9c\x94 Test fakeExample() passed after 0.001 seconds."
        printf '%s\n' "\xe2\x9c\x94 Test run with 2 tests passed after 0.002 seconds."
        ;;
    truncated)
        printf '%s\n' "Test Case '-[SRUITests.FakeTests testOne]' started."
        printf '%s\n' "◇ Test fakeExample() started."
        ;;
esac
exit "${FAKE_RUNNER_STATUS:-0}"
SH
chmod +x "$sandbox/scripts/run-swift-tests.sh"

# The wrapper samples a stalled test host. Nothing is stalled here, and the host's own
# `swiftpm-testing-helper` must never be sampled by a self-test, so `sample` is a no-op.
printf '#!/bin/sh\nexit 0\n' >"$sandbox/bin/sample"
chmod +x "$sandbox/bin/sample"

# `script(1)` that runs the child and always exits 0 - the status-discarding behaviour the wrapper
# must not depend on.
cat >"$sandbox/bin/script-discarding" <<'SH'
#!/bin/sh
while [ $# -gt 0 ] && [ "${1#-}" != "$1" ]; do shift; done
log=$1
shift
"$@" >"$log" 2>&1
exit 0
SH
chmod +x "$sandbox/bin/script-discarding"

# `script(1)` that writes a plausible typescript without ever running the child, so the wrapper's
# status side channel stays empty.
cat >"$sandbox/bin/script-silent" <<'SH'
#!/bin/sh
while [ $# -gt 0 ] && [ "${1#-}" != "$1" ]; do shift; done
log=$1
{
    printf '%s\n' "Building for debugging..."
    printf '%s\n' "Build complete! (0.42s)"
    printf '%s\n' "Test Case '-[SRUITests.FakeTests testOne]' started."
    printf '%s\n' "Test Case '-[SRUITests.FakeTests testOne]' passed (0.001 seconds)."
    printf '%s\n' "Executed 1 test, with 0 failures (0 unexpected) in 0.001 (0.001) seconds"
    printf '%s\n' "◇ Test fakeExample() started."
    printf '%s\n' "✔ Test fakeExample() passed after 0.001 seconds."
    printf '%s\n' "✔ Test run with 1 test passed after 0.002 seconds."
} >"$log"
exit 0
SH
chmod +x "$sandbox/bin/script-silent"

# A faithful util-linux `script(1)`: `script [options] [file]`, command *only* through `-c`, and a
# trailing positional command rejected as bad usage. Linux is where the wrapper's BSD spelling used
# to die before the runner ever started, and a macOS CI runner has no util-linux to catch it - so
# the dialect is emulated here rather than left to whoever next runs the suite on Linux.
cat >"$sandbox/bin/script-utillinux" <<'SH'
#!/bin/sh
command=""
file=""
return_child_status=0
while [ $# -gt 0 ]; do
    case $1 in
        --version) echo "script from util-linux 2.38.1"; exit 0 ;;
        -e) return_child_status=1; shift ;;
        -c) command=$2; shift 2 ;;
        -q|-a|-f) shift ;;
        -*) shift ;;
        *)
            if [ -z "$file" ]; then
                file=$1; shift
            else
                # This is the BSD spelling. util-linux does not accept it.
                echo "script: bad usage: unexpected argument '$1'" >&2
                exit 1
            fi
            ;;
    esac
done
if [ -z "$command" ] || [ -z "$file" ]; then
    echo "script: bad usage" >&2
    exit 1
fi
/bin/sh -c "$command" >"$file" 2>&1
child=$?
[ "$return_child_status" -eq 1 ] && exit "$child"
exit 0
SH
chmod +x "$sandbox/bin/script-utillinux"

# Installs one of the stand-ins as `script`; `real` restores the system one.
use_script() {
    rm -f "$sandbox/bin/script"
    [ "$1" = real ] && return 0
    ln -sf "$sandbox/bin/script-$1" "$sandbox/bin/script"
}

# Runs the wrapper under test. Output goes to a file, never a command substitution: the wrapper
# leaves its sampler's `sleep` running, and a pipe would hold this script open until that sleep
# exited - the very stdio wedge this suite's scripts exist to prevent.
out="$sandbox/wrapper.out"
run_wrapper() {
    local shape=$1 runner_status=$2
    (
        cd "$sandbox" || exit 1
        PATH="$sandbox/bin:$PATH" \
            SRUI_TEST_TIMEOUT=2 \
            FAKE_RUNNER_SHAPE="$shape" \
            FAKE_RUNNER_STATUS="$runner_status" \
            bash scripts/assert-swift-test-run-complete.sh
    ) >"$out" 2>&1
}

assert_status() {
    local actual=$1 expected=$2 label=$3
    if [ "$actual" = "$expected" ]; then
        pass "$label"
    else
        fail "$label (wrapper exited $actual, expected $expected)"
        sed 's/^/    /' "$out" >&2
    fi
}

assert_nonzero_status() {
    local actual=$1 label=$2
    if [ "$actual" -ne 0 ]; then
        pass "$label"
    else
        fail "$label (wrapper exited 0)"
        sed 's/^/    /' "$out" >&2
    fi
}

assert_output_contains() {
    if grep -qF -- "$1" "$out"; then
        pass "$2"
    else
        fail "$2 (output lacked '$1')"
        sed 's/^/    /' "$out" >&2
    fi
}

# --- cases --------------------------------------------------------------------------------------

echo "case 1: a complete, passing run is reported green"
use_script real
run_wrapper complete-pass 0
assert_status $? 0 "wrapper exited 0 for a complete passing run"
# Three reported, not two: swift-testing's "Test run with N tests" summary reports as `run`.
assert_output_contains "run accounting: 2 test(s) started, 3 reported." "accounting counted both tests"

echo "case 2: a complete run whose tests failed is never reported green"
use_script real
run_wrapper complete-fail 1
assert_nonzero_status $? "wrapper propagated the failing runner's status through the real script(1)"

echo "case 3: a script(1) that discards the child's status cannot make a failing run green"
use_script discarding
run_wrapper complete-fail 1
assert_nonzero_status $? "wrapper reported the failing run from its own status side channel"

echo "case 4: a runner that never reports a status fails closed"
use_script silent
run_wrapper complete-pass 0
assert_nonzero_status $? "wrapper refused a run whose status never arrived"
assert_output_contains "MISSING STATUS" "wrapper said why it refused"

echo "case 5: a truncated run is refused even when the runner exits 0"
use_script real
run_wrapper truncated 0
assert_nonzero_status $? "wrapper refused a run that left tests unreported"
# A truncated host prints no summary at all, and that - not the per-test tally - is what refuses the
# run: the tally is advisory because the pty capture layer provably drops lines under load.
assert_output_contains "MISSING SUMMARY" "wrapper named the missing summary"
assert_output_contains "started without a matching result line" "wrapper still reported the tally"

echo "case 5b: a dropped result line is advisory while both summaries are present"
# Measured on a macos-15 runner: 588 started, 537 result lines, summary `578 tests passed`, no skips
# and no interleaved writes - `script(1)` dropped the rest. Failing on that would make this guard
# trip on every CI run.
use_script real
run_wrapper dropped-result 0
assert_status $? 0 "wrapper accepted a run whose capture lost a result line"
assert_output_contains "started without a matching result line" "wrapper noted the loss"
assert_output_contains "advisory only" "wrapper said the note is advisory"

echo "case 5c: a swift-testing host that dies after XCTest finished is refused"
# Isolates the swift-testing summary check: XCTest's own summary is present, so only the
# swift-testing one can reject this. Without a case of this shape, removing that check passes the
# suite - which is how it was found.
use_script real
run_wrapper swift-testing-died 0
assert_nonzero_status $? "wrapper refused a run whose swift-testing host never summarised"
assert_output_contains 'swift-testing never printed' "wrapper named the missing swift-testing summary"

echo "case 6: the util-linux script(1) dialect runs the same accounting"
use_script utillinux
run_wrapper complete-pass 0
assert_status $? 0 "wrapper exited 0 under util-linux script"
assert_output_contains "run accounting: 2 test(s) started, 3 reported." "accounting survived the -c invocation"

echo "case 7: util-linux reports a failing run through -e, not through the typescript"
use_script utillinux
run_wrapper complete-fail 1
assert_nonzero_status $? "wrapper reported the failing run under util-linux script"

echo
if [ "$failures" -eq 0 ]; then
    echo "assert-swift-test-run-complete: all cases passed."
    exit 0
fi
echo "assert-swift-test-run-complete: $failures assertion(s) failed." >&2
exit 1
