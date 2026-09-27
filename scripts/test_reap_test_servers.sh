#!/usr/bin/env bash
#
# test_reap_test_servers.sh
# Selection-rule tests for scripts/reap-test-servers.sh.
#
# Every case runs the real reaper against a sandbox: `SRUI_REAP_PATTERN` points at a marker
# process built for the case (a symlink to /bin/sleep, so argv[0] is a path we chose) and
# `SRUI_REAP_TMP_GLOBS` points inside a private temp directory. No real fixture server and no
# real /tmp/srui-* directory is ever in scope, so this is safe to run while tests are running.
#
# Run: bash scripts/test_reap_test_servers.sh

set -uo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
reaper="$repo_root/scripts/reap-test-servers.sh"

sandbox=$(mktemp -d "${TMPDIR:-/tmp}/reap-selftest.XXXXXX")
mkdir -p "$sandbox/bin" "$sandbox/tmp"
failures=0
spawned_pids=()
spawned_pid=""

cleanup() {
    local pid
    for pid in "${spawned_pids[@]+"${spawned_pids[@]}"}"; do
        kill -9 "$pid" 2>/dev/null
    done
    pkill -9 -f "$sandbox" 2>/dev/null
    rm -rf -- "$sandbox"
}
trap cleanup EXIT

# --- helpers ------------------------------------------------------------------------------------

pass() { echo "  ok: $1"; }
fail() {
    echo "  FAIL: $1" >&2
    failures=$((failures + 1))
}

# An executable whose argv[0] is a path we control. A copied binary would be SIGKILLed by code
# signing on Apple silicon and a shebang script would report /bin/sh as argv[0], which is exactly
# the column the reaper matches on.
make_marker() {
    local name=$1
    ln -sf /bin/sleep "$sandbox/bin/$name"
    printf '%s' "$sandbox/bin/$name"
}

# ERE that matches only this marker's path.
marker_pattern() {
    printf '%s$' "$(printf '%s' "$1" | sed 's/[.[\*^$+?(){}|]/\\&/g')"
}

# The spawn helpers publish `spawned_pid` rather than echoing it: a `$(...)` substitution would
# both reparent the marker to init (defeating case 1) and hang forever, because the background
# process inherits the substitution's stdout pipe and the shell waits for EOF. Marker stdio goes
# to /dev/null for that same reason -- this is the very leak the reaper exists for.
spawn_with_live_parent() {
    "$1" 600 >/dev/null 2>&1 &
    spawned_pid=$!
    spawned_pids+=("$spawned_pid")
    disown "$spawned_pid" 2>/dev/null # keep bash from narrating the kills as job status
}

# Double-fork: the intermediate shell exits immediately, so the marker is reparented to init and
# reads as ppid 1 -- exactly what a fixture server left by a dead test run looks like.
spawn_orphan() {
    ("$1" 600 >/dev/null 2>&1 &) 2>/dev/null
    local waited=0
    spawned_pid=""
    while [ "$waited" -lt 50 ]; do
        spawned_pid=$(pgrep -f "^$1 600$" 2>/dev/null | head -1)
        [ -n "$spawned_pid" ] && break
        sleep 0.1
        waited=$((waited + 1))
    done
    [ -n "$spawned_pid" ] && spawned_pids+=("$spawned_pid")
}

# A live process whose command line names a socket inside a directory, without matching the kill
# pattern -- the shape of a real fixture server holding a runtime directory.
spawn_socket_holder() {
    local holder="$sandbox/bin/socket-holder"
    cat >"$holder" <<'SH'
#!/bin/sh
sleep 600
SH
    chmod +x "$holder"
    "$holder" --socket "$1" >/dev/null 2>&1 &
    spawned_pid=$!
    spawned_pids+=("$spawned_pid")
    disown "$spawned_pid" 2>/dev/null
}

# A marker whose parent never calls `wait`: the intermediate shell `exec`s into `sleep`, so it
# keeps the same pid and the marker keeps the same parent, and when the marker dies it stays in the
# process table as a zombie instead of vanishing. That is exactly the state a killed orphan is left
# in on a host whose pid 1 does not reap (a container running a plain shell as init). It cannot be
# staged with a *real* orphan here, because launchd reaps one in microseconds; `make_fake_ps` below
# supplies the missing half. Publishes `spawned_pid` and `unreaping_parent_pid`.
spawn_unreaped_child() {
    local marker=$1 pidfile="$sandbox/unreaped.pid"
    rm -f "$pidfile"
    cat >"$sandbox/bin/unreaping-parent.sh" <<'SH'
#!/bin/sh
"$1" 600 >/dev/null 2>&1 &
echo "$!" >"$2"
exec /bin/sleep 900
SH
    chmod +x "$sandbox/bin/unreaping-parent.sh"
    "$sandbox/bin/unreaping-parent.sh" "$marker" "$pidfile" >/dev/null 2>&1 &
    unreaping_parent_pid=$!
    spawned_pids+=("$unreaping_parent_pid")
    disown "$unreaping_parent_pid" 2>/dev/null
    spawned_pid=""
    local waited=0
    while [ "$waited" -lt 50 ]; do
        spawned_pid=$(cat "$pidfile" 2>/dev/null)
        [ -n "$spawned_pid" ] && break
        sleep 0.1
        waited=$((waited + 1))
    done
    [ -n "$spawned_pid" ] && spawned_pids+=("$spawned_pid")
}

# A `ps` stand-in that reports one pid as reparented to init in the process-table snapshot the
# reaper takes, and passes every other query (notably the `-o state= -p` liveness check) straight
# through to the real `ps`. Rewriting that one column is what lets the sandbox present a process
# the selection rules accept while its real parent is alive and not reaping it. Prints its bin dir.
make_fake_ps() {
    local dir="$sandbox/fakebin"
    mkdir -p "$dir"
    cat >"$dir/ps" <<'SH'
#!/bin/sh
case $1 in
    -eo) /bin/ps "$@" | awk -v p="${SRUI_FAKE_ORPHAN_PID:-0}" '{ if ($1 + 0 == p + 0) $2 = 1; print }' ;;
    *) exec /bin/ps "$@" ;;
esac
SH
    chmod +x "$dir/ps"
    printf '%s' "$dir"
}

run_reaper() {
    local pattern=$1 age=$2
    shift 2
    SRUI_REAP_PATTERN="$pattern" \
        SRUI_REAP_AGE_MINUTES="$age" \
        SRUI_REAP_TMP_GLOBS="$sandbox/tmp/srui-*" \
        bash "$reaper" "$@"
}

assert_alive() {
    if kill -0 "$1" 2>/dev/null; then pass "$2"; else fail "$2 (pid $1 is gone)"; fi
}
assert_dead() {
    if kill -0 "$1" 2>/dev/null; then fail "$2 (pid $1 still alive)"; else pass "$2"; fi
}
# Terminated means gone from the process table *or* a zombie: a killed process whose parent has not
# reaped it is dead, whatever `kill -0` says.
assert_terminated() {
    local state
    state=$(ps -o state= -p "$1" 2>/dev/null | tr -d '[:space:]')
    case $state in
        '' | Z*) pass "$2" ;;
        *) fail "$2 (pid $1 is in state '$state')" ;;
    esac
}
assert_dir_present() {
    if [ -d "$1" ]; then pass "$2"; else fail "$2 ($1 was removed)"; fi
}
assert_dir_absent() {
    if [ -d "$1" ]; then fail "$2 ($1 still exists)"; else pass "$2"; fi
}
assert_contains() {
    if printf '%s' "$1" | grep -qF -- "$2"; then
        pass "$3"
    else
        fail "$3 (output lacked '$2')"
        printf '%s\n' "    reaper said: $1" >&2
    fi
}

# --- cases --------------------------------------------------------------------------------------

echo "case 1: a fixture process with a live parent is never killed"
marker=$(make_marker fixture-live)
spawn_with_live_parent "$marker"
pid=$spawned_pid
run_reaper "$(marker_pattern "$marker")" 0 >/dev/null
assert_alive "$pid" "live-parent marker survived a zero-age sweep"
kill -9 "$pid" 2>/dev/null

echo "case 2: an orphaned fixture process past the age threshold is killed"
marker=$(make_marker fixture-orphan-old)
spawn_orphan "$marker"
pid=$spawned_pid
if [ -z "$pid" ]; then
    fail "could not spawn an orphan marker"
else
    run_reaper "$(marker_pattern "$marker")" 0 >/dev/null
    assert_dead "$pid" "orphaned, old-enough marker was killed"
fi

echo "case 3: an orphan younger than the threshold is not killed"
marker=$(make_marker fixture-orphan-young)
spawn_orphan "$marker"
pid=$spawned_pid
if [ -z "$pid" ]; then
    fail "could not spawn an orphan marker"
else
    run_reaper "$(marker_pattern "$marker")" 60 >/dev/null
    assert_alive "$pid" "young orphan survived a 60-minute threshold"
    kill -9 "$pid" 2>/dev/null
fi

echo "case 4: a directory a live process references is kept; an unreferenced one is removed"
referenced="$sandbox/tmp/srui-referenced"
unreferenced="$sandbox/tmp/srui-unreferenced"
mkdir -p "$referenced" "$unreferenced"
spawn_socket_holder "$referenced/sessiond.sock"
holder=$spawned_pid
sleep 0.3
run_reaper 'NEVER_MATCHES_ANY_EXECUTABLE' 0 >/dev/null
assert_dir_present "$referenced" "directory named by a live --socket argument was kept"
assert_dir_absent "$unreferenced" "unreferenced directory was removed"
kill -9 "$holder" 2>/dev/null

echo "case 5: an unreferenced directory touched inside the age window is kept"
fresh="$sandbox/tmp/srui-fresh"
mkdir -p "$fresh"
run_reaper 'NEVER_MATCHES_ANY_EXECUTABLE' 60 >/dev/null
assert_dir_present "$fresh" "just-created directory survived the setup-race guard"

echo "case 6: --dry-run changes nothing"
marker=$(make_marker fixture-dry-run)
spawn_orphan "$marker"
pid=$spawned_pid
dry_dir="$sandbox/tmp/srui-dry-run"
mkdir -p "$dry_dir"
if [ -z "$pid" ]; then
    fail "could not spawn an orphan marker"
else
    output=$(run_reaper "$(marker_pattern "$marker")" 0 --dry-run)
    assert_alive "$pid" "--dry-run left the reapable orphan running"
    assert_dir_present "$dry_dir" "--dry-run left the reapable directory in place"
    assert_contains "$output" "would kill pid $pid" "--dry-run reported the orphan it would kill"
    assert_contains "$output" "would remove $dry_dir" "--dry-run reported the directory"
    kill -9 "$pid" 2>/dev/null
fi

echo "case 7: an empty sweep succeeds"
run_reaper 'NEVER_MATCHES_ANY_EXECUTABLE' 0 >/dev/null
if [ $? -eq 0 ]; then pass "nothing to reap is not a failure"; else fail "empty sweep exited non-zero"; fi

echo "case 8: a fixture server that dies into an unreaped zombie counts as killed, not survived"
marker=$(make_marker fixture-zombie)
fake_bin=$(make_fake_ps)
unreaping_parent_pid=""
spawn_unreaped_child "$marker"
pid=$spawned_pid
if [ -z "$pid" ]; then
    fail "could not spawn a marker under a non-reaping parent"
else
    # The PATH override lives inside the command substitution's subshell, so the stand-in `ps` is
    # visible to this invocation of the reaper and to nothing else.
    output=$(PATH="$fake_bin:$PATH" SRUI_FAKE_ORPHAN_PID="$pid" \
        run_reaper "$(marker_pattern "$marker")" 0 2>&1)
    reaper_status=$?
    assert_terminated "$pid" "the staged orphan was terminated"
    if [ "$reaper_status" -eq 0 ]; then
        pass "the reaper exited 0 after killing a process its parent never reaped"
    else
        fail "the reaper exited $reaper_status after killing a process its parent never reaped"
        printf '%s\n' "    reaper said: $output" >&2
    fi
    if printf '%s' "$output" | grep -qF "survived SIGKILL"; then
        fail "the reaper called an unreaped zombie a survivor of SIGKILL"
        printf '%s\n' "    reaper said: $output" >&2
    else
        pass "no spurious 'survived SIGKILL' for an unreaped zombie"
    fi
    kill -9 "$unreaping_parent_pid" 2>/dev/null
fi
echo
if [ "$failures" -eq 0 ]; then
    echo "reap-test-servers selection rules: all cases passed."
    exit 0
fi
echo "reap-test-servers selection rules: $failures assertion(s) failed." >&2
exit 1
