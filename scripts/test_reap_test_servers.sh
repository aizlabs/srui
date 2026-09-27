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
# The reaper under test is a *copy*, placed in a sandbox git repository with a linked worktree of
# its own. That is not indirection for its own sake: rule 6 only reaps binaries that live inside a
# checkout of the repository the script itself sits in, so the sandbox has to be that repository
# for a marker to be reapable at all - and markers outside it are how rule 6 is tested.
#
# Run: bash scripts/test_reap_test_servers.sh

set -uo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

sandbox=$(mktemp -d "${TMPDIR:-/tmp}/reap-selftest.XXXXXX")
mkdir -p "$sandbox/bin" "$sandbox/tmp" "$sandbox/repo/scripts" "$sandbox/repo/target/debug" \
    "$sandbox/elsewhere/target/debug" "$sandbox/usr-local-bin"
git -C "$sandbox/repo" init -q
git -C "$sandbox/repo" -c user.email=selftest@example.invalid -c user.name=selftest \
    commit -q --allow-empty -m "sandbox root"
git -C "$sandbox/repo" worktree add -q "$sandbox/wt" -b selftest
mkdir -p "$sandbox/wt/target/debug"
cp "$repo_root/scripts/reap-test-servers.sh" "$sandbox/repo/scripts/reap-test-servers.sh"
reaper="$sandbox/repo/scripts/reap-test-servers.sh"
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

# An executable whose argv[0] is a path we control, under the sandbox repository's own
# `target/debug` - the layout a real fixture server has, and inside a checkout, so rule 6 lets it
# be reaped. A copied binary would be SIGKILLed by code signing on Apple silicon and a shebang
# script would report /bin/sh as argv[0], which is exactly the column the reaper matches on.
make_marker() {
    local name=$1
    ln -sf /bin/sleep "$sandbox/repo/target/debug/$name"
    printf '%s' "$sandbox/repo/target/debug/$name"
}

# The same marker in the sandbox repository's linked worktree: reapable only if the reaper reads
# `git worktree list` rather than just its own toplevel.
make_worktree_marker() {
    local name=$1
    ln -sf /bin/sleep "$sandbox/wt/target/debug/$name"
    printf '%s' "$sandbox/wt/target/debug/$name"
}

# A marker at a path that belongs to no checkout of this repository: `$1` is a directory under the
# sandbox but outside `repo` and `wt`, standing in for `/usr/local/bin` or another project's
# `target/debug`.
make_foreign_marker() {
    local dir=$1 name=$2
    ln -sf /bin/sleep "$dir/$name"
    printf '%s' "$dir/$name"
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

# Both assertions read the process state rather than calling `kill -0`, because `kill -0` succeeds
# for a zombie: under an init that does not reap (a container running a plain shell as pid 1) it
# would call a killed process alive, failing every "was killed" case and passing every "survived"
# case for the wrong reason. Alive means a state that is not `Z`; terminated means gone from the
# table or `Z`.
process_state() {
    ps -o state= -p "$1" 2>/dev/null | tr -d '[:space:]'
}
assert_alive() {
    local state
    state=$(process_state "$1")
    case $state in
        '') fail "$2 (pid $1 is gone)" ;;
        Z*) fail "$2 (pid $1 is a zombie)" ;;
        *) pass "$2" ;;
    esac
}
assert_terminated() {
    local state
    state=$(process_state "$1")
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
    assert_terminated "$pid" "orphaned, old-enough marker was killed"
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
echo "case 9: a directory whose socket is bound in-process, named on no command line, is kept"
# Short names on purpose: an absolute unix socket path is capped at 104 bytes, and the sandbox
# already spends 70 of them.
bound_dir="$sandbox/tmp/srui-b"
mkdir -p "$bound_dir"
# The socket path is baked into the script body, so nothing on the binder's command line mentions
# it: the only evidence that the directory is live is the kernel's socket table. This is the shape
# of `srui-sessiond` started with no `--socket`, which computes its default path internally.
cat >"$sandbox/bin/socket-binder.py" <<PY
import socket, time

sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.bind("$bound_dir/s")
sock.listen(1)
time.sleep(600)
PY
python3 "$sandbox/bin/socket-binder.py" >/dev/null 2>&1 &
binder=$!
spawned_pids+=("$binder")
disown "$binder" 2>/dev/null
for _ in $(seq 1 50); do
    [ -S "$bound_dir/s" ] && break
    sleep 0.1
done
if [ ! -S "$bound_dir/s" ]; then
    fail "could not bind a unix socket for the in-process case"
else
    run_reaper 'NEVER_MATCHES_ANY_EXECUTABLE' 0 >/dev/null
    assert_dir_present "$bound_dir" "directory holding a live, unnamed bound socket was kept"
fi
kill -9 "$binder" 2>/dev/null

echo "case 10: the default runtime directory is never swept, and its siblings still are"
runtime_root="$sandbox/tmproot"
default_dir="$runtime_root/srui-$(id -u)"
sibling_dir="$runtime_root/srui-fixture-leftover"
mkdir -p "$default_dir" "$sibling_dir"
# TMPDIR is what the servers resolve their default socket path against on macOS; the sweep is
# pointed at that root so the guard is exercised without touching the real /tmp.
SRUI_REAP_PATTERN='NEVER_MATCHES_ANY_EXECUTABLE' \
    SRUI_REAP_AGE_MINUTES=0 \
    SRUI_REAP_TMP_GLOBS="$runtime_root/srui-*" \
    TMPDIR="$runtime_root" \
    bash "$reaper" >/dev/null 2>&1
assert_dir_present "$default_dir" "the default runtime directory survived a zero-age sweep"
assert_dir_absent "$sibling_dir" "an unreferenced sibling directory was still removed"

echo "case 11: an orphaned process serving the default runtime socket is never killed"
# The shape of a `srui-sessiond` a human detached on purpose: ppid 1, old enough, argv[0] matching
# the fixture pattern, and bound to the default runtime socket. `nc -lU` stands in for the daemon
# through a symlink, so argv[0] is a path this case controls.
default_root="$sandbox/tmp2"
default_socket_dir="$default_root/srui-$(id -u)"
mkdir -p "$default_socket_dir"
if ! command -v nc >/dev/null 2>&1; then
    fail "nc is unavailable; cannot stage a default-socket server"
else
    listener=$(make_marker fixture-default-socket)
    ln -sf "$(command -v nc)" "$listener"
    ("$listener" -lU "$default_socket_dir/s" >/dev/null 2>&1 &) 2>/dev/null
    listener_pid=""
    for _ in $(seq 1 50); do
        listener_pid=$(pgrep -f "^$listener -lU" 2>/dev/null | head -1)
        [ -n "$listener_pid" ] && [ -S "$default_socket_dir/s" ] && break
        sleep 0.1
    done
    if [ -z "$listener_pid" ] || [ ! -S "$default_socket_dir/s" ]; then
        fail "could not stage a server bound to the default runtime socket"
    else
        spawned_pids+=("$listener_pid")
        output=$(SRUI_REAP_PATTERN="$(marker_pattern "$listener")" \
            SRUI_REAP_AGE_MINUTES=0 \
            SRUI_REAP_TMP_GLOBS="$default_root/srui-*" \
            TMPDIR="$default_root" \
            bash "$reaper" 2>&1)
        assert_alive "$listener_pid" "the daemon on the default runtime socket survived a zero-age sweep"
        assert_dir_present "$default_socket_dir" "its runtime directory survived with it"
        if printf '%s' "$output" | grep -qF "killing orphaned fixture server pid $listener_pid"; then
            fail "the reaper announced a kill for the default-socket daemon"
            printf '%s\n' "    reaper said: $output" >&2
        else
            pass "the reaper never selected it"
        fi
    fi
    kill -9 "${listener_pid:-0}" 2>/dev/null
fi

echo "case 12: an orphan in a linked worktree of this repository is reaped"
marker=$(make_worktree_marker fixture-worktree)
spawn_orphan "$marker"
pid=$spawned_pid
if [ -z "$pid" ]; then
    fail "could not spawn an orphan marker in the linked worktree"
else
    run_reaper "$(marker_pattern "$marker")" 0 >/dev/null
    assert_terminated "$pid" "orphan under a linked worktree was killed"
fi

echo "case 13: an identically named orphan outside every checkout is never signalled"
# Line 3 and line 4 of the rule: a system install, and a sibling project's build output. Same
# binary name, same orphanhood, same age - only the path differs.
for spec in "$sandbox/usr-local-bin:srui-sessiond" "$sandbox/elsewhere/target/debug:srui-sessiond"; do
    foreign_dir=${spec%:*}
    foreign_name=${spec##*:}
    marker=$(make_foreign_marker "$foreign_dir" "$foreign_name")
    spawn_orphan "$marker"
    pid=$spawned_pid
    if [ -z "$pid" ]; then
        fail "could not spawn an orphan marker at $marker"
        continue
    fi
    output=$(run_reaper "$(marker_pattern "$marker")" 0 2>&1)
    assert_alive "$pid" "orphan at $foreign_dir survived a zero-age sweep"
    if printf '%s' "$output" | grep -qF "killing orphaned fixture server pid $pid"; then
        fail "the reaper announced a kill for $marker"
        printf '%s\n' "    reaper said: $output" >&2
    else
        pass "the reaper never selected $foreign_dir"
    fi
    assert_contains "$output" "1 outside this repository" "the sweep accounted for it as foreign"
    kill -9 "$pid" 2>/dev/null
done

echo "case 14: without a socket inventory, a sweep kills nothing while a default socket exists"
# `lsof` missing or denied empties rule 5's evidence, so a detached daemon on the default socket
# would be indistinguishable from debris. The stand-in prints nothing and fails, like a denied or
# absent lsof; the real one stays untouched outside this case.
mkdir -p "$sandbox/nolsof"
printf '#!/bin/sh\nexit 1\n' >"$sandbox/nolsof/lsof"
chmod +x "$sandbox/nolsof/lsof"
blind_root="$sandbox/tmp3"
blind_default="$blind_root/srui-$(id -u)"
mkdir -p "$blind_default"
# A socket file in the default runtime directory, left behind by a process that has exited: the
# reaper must not need to know *who* holds it to decide to keep its hands off.
python3 -c "import socket; socket.socket(socket.AF_UNIX).bind('$blind_default/s')" 2>/dev/null
marker=$(make_marker fixture-blind)
spawn_orphan "$marker"
pid=$spawned_pid
if [ -z "$pid" ] || [ ! -S "$blind_default/s" ]; then
    fail "could not stage a blind sweep (pid='${pid:-}', socket present: $([ -S "$blind_default/s" ] && echo yes || echo no))"
else
    output=$(PATH="$sandbox/nolsof:$PATH" \
        SRUI_REAP_PATTERN="$(marker_pattern "$marker")" \
        SRUI_REAP_AGE_MINUTES=0 \
        SRUI_REAP_TMP_GLOBS="$blind_root/srui-*" \
        TMPDIR="$blind_root" \
        bash "$reaper" 2>&1)
    assert_alive "$pid" "the orphan survived a sweep that could not read the socket table"
    assert_dir_present "$blind_default" "the default runtime directory survived it too"
    assert_contains "$output" "killing nothing" "the sweep said why it killed nothing"

    # The gate is the missing evidence plus something to protect, not the missing evidence alone:
    # with no socket in the default runtime directory there is no daemon to confuse with debris.
    rm -f "$blind_default/s"
    output=$(PATH="$sandbox/nolsof:$PATH" \
        SRUI_REAP_PATTERN="$(marker_pattern "$marker")" \
        SRUI_REAP_AGE_MINUTES=0 \
        SRUI_REAP_TMP_GLOBS="$blind_root/srui-*" \
        TMPDIR="$blind_root" \
        bash "$reaper" 2>&1)
    assert_terminated "$pid" "with no default socket on disk, the same orphan was killed"
    assert_contains "$output" "nothing to protect" "the sweep said why it proceeded"
fi
kill -9 "${pid:-0}" 2>/dev/null

echo
if [ "$failures" -eq 0 ]; then
    echo "reap-test-servers selection rules: all cases passed."
    exit 0
fi
echo "reap-test-servers selection rules: $failures assertion(s) failed." >&2
exit 1
