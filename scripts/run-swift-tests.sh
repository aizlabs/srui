#!/usr/bin/env bash
# Run the macOS client test suite under a watchdog that reports *why* a run wedged.
#
# The failure this exists for: swift-test reads the test binary's stdout/stderr through pipes and
# returns only on EOF. A server spawned by a test that inherits those pipes and outlives the binary
# keeps them open forever, so swift-test sits at 0% CPU with its child already reaped as <defunct>,
# every test passed, and nothing printed. Waiting it out does not help and there is no failing test
# to point at. `scripts/check-test-process-stdio.sh` prevents new instances of that; this script
# makes an existing one diagnose itself in seconds instead of costing an afternoon.
#
# Usage: scripts/run-swift-tests.sh [extra swift-test args...]
#   SRUI_TEST_TIMEOUT   seconds before the watchdog fires (default 300)
set -uo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

timeout_seconds=${SRUI_TEST_TIMEOUT:-300}
package=client-macos

# Job control puts the test run in its own process group. Everything a fixture spawns inherits
# that group, and — crucially — a process keeps its group when it is orphaned and reparented to
# init. So the group is an exact handle on "processes this invocation created", which a
# `pgrep -f` pattern is not: matching command lines would also kill another checkout's servers, a
# developer's sshd, or a concurrent test run on the same host.
set -m
swift test --package-path "$package" "$@" &
test_pid=$!
set +m

# Members of our own process group only, excluding the leader.
group_survivors() {
    pgrep -g "$test_pid" 2>/dev/null | grep -vx "$test_pid" || true
}

reap_group() {
    local pids
    pids=$(group_survivors | tr '\n' ' ')
    if [ -n "${pids// /}" ]; then
        echo "note: killing test servers left by this run:$(echo " $pids" | sed 's/ *$//')" >&2
        kill -9 -- "-$test_pid" 2>/dev/null || true
    fi
}

diagnose_wedge() {
    local pid=$1
    echo "=================================================================" >&2
    echo "swift test exceeded ${timeout_seconds}s. Diagnosing before killing it." >&2
    echo "=================================================================" >&2

    echo >&2
    echo "--- swift-test process tree (a <defunct> child means the tests already finished) ---" >&2
    ps -eo pid,ppid,state,%cpu,etime,command 2>/dev/null |
        awk -v p="$pid" 'NR==1 || $1==p || $2==p' >&2

    echo >&2
    echo "--- who holds swift-test's stdout/stderr pipes ---" >&2
    # lsof prints a pipe as: FD TYPE DEVICE SIZE NODE ->PEER. The holder of the write end shows the
    # reader's PEER as its own DEVICE, so match this process's peers against each survivor's
    # device column.
    local peers
    peers=$(lsof -p "$pid" 2>/dev/null | awk '$5=="PIPE" {sub(/^->/,"",$NF); print $NF}')
    if [ -z "$peers" ]; then
        echo "  (none open — the stall is not pipe inheritance)" >&2
    else
        local found=0 sp devices peer
        for sp in $(group_survivors); do
            devices=$(lsof -p "$sp" 2>/dev/null | awk '$5=="PIPE" {print $6}')
            for peer in $peers; do
                if echo "$devices" | grep -qx "$peer"; then
                    echo "  HOLDER pid=$sp $(ps -o command= -p "$sp" 2>/dev/null | cut -c1-90)" >&2
                    found=1
                    break
                fi
            done
        done
        [ "$found" -eq 0 ] && echo "  (no surviving test server in this run's process group holds them)" >&2
    fi

    echo >&2
    echo "--- swift-test stacks (threads parked in read() confirm the pipe wedge) ---" >&2
    sample "$pid" 1 -mayDie 2>/dev/null | grep -E "^ +[0-9]+ (Thread|read|__psynch|wait4)" | head -20 >&2
}

(
    sleep "$timeout_seconds"
    kill -0 "$test_pid" 2>/dev/null || exit 0
    # The leader is `swift test`; the process actually parked on the pipes is the `swift-test`
    # child it execs, so prefer that one for the stack and descriptor dump.
    swift_test_pid=$(pgrep -g "$test_pid" -x swift-test 2>/dev/null | head -1)
    diagnose_wedge "${swift_test_pid:-$test_pid}"
    kill -9 -- "-$test_pid" 2>/dev/null
) &
watchdog_pid=$!

wait "$test_pid"
status=$?
kill "$watchdog_pid" 2>/dev/null
wait "$watchdog_pid" 2>/dev/null

# Fixtures can still strand a server when the binary exits before every `defer` completes. They
# are harmless once their stdio is detached, but they hold ports and /tmp directories.
reap_group

if [ "$status" -ne 0 ]; then
    echo "swift test failed with status $status." >&2
fi
exit "$status"
