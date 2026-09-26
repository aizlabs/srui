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

echo
if [ "$failures" -eq 0 ]; then
    echo "reap-test-servers selection rules: all cases passed."
    exit 0
fi
echo "reap-test-servers selection rules: $failures assertion(s) failed." >&2
exit 1
