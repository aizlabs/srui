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

# Anything a test fixture starts, matched by the /tmp fixture paths the fixtures use.
survivor_pattern='sshd_config|/tmp/srui-|target/debug/counter|srui-sessiond --socket|coding-agent-demo --socket'

sweep_survivors() {
    local label=$1 pids
    pids=$(pgrep -f "$survivor_pattern" 2>/dev/null | tr '\n' ' ')
    if [ -n "${pids// /}" ]; then
        echo "note: killing $label test servers:$(echo " $pids" | sed 's/ *$//')" >&2
        # shellcheck disable=SC2086
        kill -9 $pids 2>/dev/null || true
    fi
}

# A wedge in a previous run leaves servers holding sockets and ports; clear them so this run is
# not diagnosed for someone else's mess.
sweep_survivors "leftover"

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
    # reader's PEER as its own DEVICE, so match this process's peers against every survivor's
    # device column.
    local peers
    peers=$(lsof -p "$pid" 2>/dev/null | awk '$5=="PIPE" {sub(/^->/,"",$NF); print $NF}')
    if [ -z "$peers" ]; then
        echo "  (none open — the stall is not pipe inheritance)" >&2
    else
        local found=0 sp
        for sp in $(pgrep -f "$survivor_pattern" 2>/dev/null); do
            local devices peer
            devices=$(lsof -p "$sp" 2>/dev/null | awk '$5=="PIPE" {print $6}')
            for peer in $peers; do
                if echo "$devices" | grep -qx "$peer"; then
                    echo "  HOLDER pid=$sp $(ps -o command= -p "$sp" 2>/dev/null | cut -c1-90)" >&2
                    found=1
                    break
                fi
            done
        done
        [ "$found" -eq 0 ] && echo "  (no surviving test server holds them)" >&2
    fi

    echo >&2
    echo "--- swift-test stacks (threads parked in read() confirm the pipe wedge) ---" >&2
    sample "$pid" 1 -mayDie 2>/dev/null | grep -E "^ +[0-9]+ (Thread|read|__psynch|wait4)" | head -20 >&2
}

swift test --package-path "$package" "$@" &
test_pid=$!

(
    sleep "$timeout_seconds"
    kill -0 "$test_pid" 2>/dev/null || exit 0
    swift_test_pid=$(pgrep -x swift-test 2>/dev/null | head -1)
    diagnose_wedge "${swift_test_pid:-$test_pid}"
    kill -9 "$test_pid" 2>/dev/null
    [ -n "$swift_test_pid" ] && kill -9 "$swift_test_pid" 2>/dev/null
) &
watchdog_pid=$!

wait "$test_pid"
status=$?
kill "$watchdog_pid" 2>/dev/null
wait "$watchdog_pid" 2>/dev/null

# Fixtures can still leak a listener when the binary exits before every `defer` completes. They are
# harmless once their stdio is detached, but they hold ports and /tmp directories, so clear them.
sweep_survivors "leftover"

if [ "$status" -ne 0 ]; then
    echo "swift test failed with status $status." >&2
fi
exit "$status"
