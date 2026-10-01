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

# A controlled `lsof`, because the reaper's decisions are only as portable as its evidence.
#
# `reap-test-servers.sh` fails closed without a socket table: no inventory means rule 5 cannot be
# enforced and *no* socket directory is removed. So on a host where `lsof` is missing or denied -
# plenty of Linux containers - every "unreferenced directory was removed" case here asserts an
# outcome the machine cannot produce. Measured on such a host: cases 4, 6 and 10 failed for that
# reason alone, with the reaper working exactly as designed.
#
# The stand-in emulates the single invocation the reaper makes, `lsof -n -P -U -F pn`, and reads
# its table from a file this test writes. Every entry staged below is true at the moment it is
# staged: a real process really is holding that socket. Case 15 still exercises the real `lsof`
# wherever the host has one, so the parser is not left untested.
lsof_table="$sandbox/lsof-table"
: >"$lsof_table"
mkdir -p "$sandbox/lsofbin"
cat >"$sandbox/lsofbin/lsof" <<'SH'
#!/bin/sh
# Only `-F pn` output is emulated; the reaper asks for nothing else.
[ -r "${SRUI_TEST_LSOF_TABLE:-}" ] || exit 1
awk -F'\t' '$1 != "" { printf "p%s\nn%s\n", $1, $2 }' "$SRUI_TEST_LSOF_TABLE"
SH
chmod +x "$sandbox/lsofbin/lsof"

# Record that `pid` holds `path`, the way the kernel's socket table would report it.
socket_table_add() {
    printf '%s\t%s\n' "$1" "$2" >>"$lsof_table"
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
    PATH="$sandbox/lsofbin:$PATH" \
        SRUI_TEST_LSOF_TABLE="$lsof_table" \
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
socket_table_add "$holder" "$referenced/sessiond.sock"
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
    # The whole point of the case: this pairing exists *only* in the socket table. Nothing on the
    # binder's command line mentions the path.
    socket_table_add "$binder" "$bound_dir/s"
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
# A third directory, genuinely held, so the sweep has socket evidence to act on at all: without it
# the reaper would keep the sibling too, and the case would pass for the wrong reason.
held_dir="$runtime_root/srui-held"
mkdir -p "$held_dir"
spawn_socket_holder "$held_dir/s"
held_holder=$spawned_pid
sleep 0.3
socket_table_add "$held_holder" "$held_dir/s"
PATH="$sandbox/lsofbin:$PATH" \
    SRUI_TEST_LSOF_TABLE="$lsof_table" \
    SRUI_REAP_PATTERN='NEVER_MATCHES_ANY_EXECUTABLE' \
    SRUI_REAP_AGE_MINUTES=0 \
    SRUI_REAP_TMP_GLOBS="$runtime_root/srui-*" \
    TMPDIR="$runtime_root" \
    bash "$reaper" >/dev/null 2>&1
assert_dir_present "$default_dir" "the default runtime directory survived a zero-age sweep"
assert_dir_present "$held_dir" "a directory whose socket is held survived it"
assert_dir_absent "$sibling_dir" "an unreferenced sibling directory was still removed"
kill -9 "${held_holder:-0}" 2>/dev/null

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
        # Rule 5's actual input: the daemon is protected because the socket table shows it holding
        # a socket in a default runtime directory. Without this entry the sweep would still spare
        # it - but through the no-evidence gate, which is case 14's subject, not this one's.
        socket_table_add "$listener_pid" "$default_socket_dir/s"
        output=$(PATH="$sandbox/lsofbin:$PATH" \
            SRUI_TEST_LSOF_TABLE="$lsof_table" \
            SRUI_REAP_PATTERN="$(marker_pattern "$listener")" \
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

echo "case 20: only directories holding nothing but sockets are removed"
# A name match is not ownership: a sweep runs automatically before tests, so anything matching the
# glob and old enough used to be `rm -rf`'d - including a developer's own `/tmp/srui-notes` or
# `/tmp/px0-cache`. The sweep is for the runtime directories of leaked servers, which hold sockets
# and nothing else; an empty directory qualifies too, being what a server that died before binding
# leaves behind.
own_dir="$sandbox/tmp/srui-mine"
sock_dir="$sandbox/tmp/srui-sockets"
empty_dir="$sandbox/tmp/srui-empty"
mkdir -p "$own_dir" "$sock_dir" "$empty_dir"
printf 'notes a developer would miss\n' >"$own_dir/notes.txt"
mkdir -p "$own_dir/subdir"
python3 -c "import socket, sys; socket.socket(socket.AF_UNIX).bind(sys.argv[1])" "$sock_dir/s" 2>/dev/null
# One real holder so the sweep has socket evidence to act on at all.
spawn_socket_holder "$sandbox/tmp/srui-held20/s"
mkdir -p "$sandbox/tmp/srui-held20"
holder20=$spawned_pid
socket_table_add "$holder20" "$sandbox/tmp/srui-held20/s"
if [ ! -S "$sock_dir/s" ]; then
    echo "  skipped: could not bind a stale socket (path length?)"
else
    output=$(run_reaper 'NEVER_MATCHES_ANY_EXECUTABLE' 0 2>&1)
    assert_dir_present "$own_dir" "a matching directory holding a developer's files was kept"
    assert_dir_absent "$sock_dir" "a directory holding only a stale socket was removed"
    assert_dir_absent "$empty_dir" "an empty runtime directory was removed"
    assert_contains "$output" "1 not socket director" "the sweep accounted for the one it refused"
fi
kill -9 "${holder20:-0}" 2>/dev/null

echo "case 21: the default glob no longer matches an unrelated px0 directory"
# `/tmp/px0*` also matched `/tmp/px0-cache`; the fixtures use `px0NN-`.
default_globs=$(awk -F"'" '/^tmp_globs=/ { print $2 }' "$reaper")
matched_cache=no
matched_fixture=no
for pattern in $default_globs; do
    case /tmp/px0-cache in $pattern) matched_cache=yes ;; esac
    case /tmp/px001-shell in $pattern) matched_fixture=yes ;; esac
done
[ "$matched_cache" = no ] && pass "the default glob does not match /tmp/px0-cache" ||
    fail "the default glob still matches /tmp/px0-cache"
[ "$matched_fixture" = yes ] && pass "the default glob still matches /tmp/px001-shell" ||
    fail "the default glob no longer matches the fixture prefix /tmp/px001-shell"

echo "case 17: an absolute path that escapes the checkout through .. is never signalled"
# `/…/repo/../elsewhere/target/debug/x` matches the `/…/repo/` prefix by spelling while resolving
# into a sibling project. Launched that way, an unrelated project's daemon looked like this
# repository's debris.
mkdir -p "$sandbox/elsewhere/target/debug"
# Built from the checkout root exactly as `git worktree list` spells it: $sandbox can contain a
# doubled slash, and that alone would make the pre-fix prefix test miss - hiding the defect this
# case exists to catch.
# `git rev-parse --show-toplevel`, not `pwd`: that is the exact spelling the reaper derives its
# roots from, and on macOS /var is a symlink to /private/var - so a path built any other way would
# not share a prefix with the root, and the pre-fix defect would hide again.
escaping_root=$(git -C "$sandbox/repo" rev-parse --show-toplevel)
escaping="$escaping_root/../elsewhere/target/debug/fixture-dotdot"
ln -sf /bin/sleep "$sandbox/elsewhere/target/debug/fixture-dotdot"
spawn_orphan "$escaping"
pid=$spawned_pid
if [ -z "$pid" ]; then
    fail "could not spawn an orphan behind a .. path"
else
    output=$(run_reaper "$(marker_pattern "$escaping")" 0 2>&1)
    assert_alive "$pid" "an orphan whose path escapes the checkout through .. survived"
    assert_contains "$output" "1 outside this repository" "the sweep accounted for it as foreign"
    if printf '%s' "$output" | grep -qF "killing orphaned fixture server pid $pid"; then
        fail "the reaper announced a kill for a path outside the checkout"
        printf '%s\n' "    reaper said: $output" >&2
    else
        pass "the reaper never selected it"
    fi
fi
kill -9 "${pid:-0}" 2>/dev/null

echo "case 16: a pid recycled between selection and signalling is never touched"
# The window is real: each candidate can hold the kill loop for up to 2.2s, so a later candidate
# has seconds in which to exit and have its number reissued to an unrelated process of this user.
# Staging a genuine recycle is not possible to order, so the identity the reaper checks is what
# changes here: the process-table snapshot passes through untouched - the marker is selected, with
# its real start time - while every `-o lstart= -p <pid>` identity probe reports a different start
# time, which is exactly what the reaper sees once the number has been reissued. Whether the
# reissue happened before or after the reaper recorded the identity is indistinguishable from here;
# case 18 is what pins down *which* observation the recorded value comes from.
recycle_bin="$sandbox/recyclebin"
mkdir -p "$recycle_bin"
cat >"$recycle_bin/ps" <<'SH'
#!/bin/sh
# `-o lstart= -p N` is the identity probe, and it never agrees with the snapshot. Everything else,
# the `-eo` snapshot included, passes through untouched.
if [ "$1" = "-o" ] && [ "$2" = "lstart=" ] && [ "$3" = "-p" ]; then
    echo "Thu Jan  1 00:00:00 2037"
    exit 0
fi
exec /bin/ps "$@"
SH
chmod +x "$recycle_bin/ps"
marker=$(make_marker fixture-recycled)
spawn_orphan "$marker"
pid=$spawned_pid
if [ -z "$pid" ]; then
    fail "could not spawn an orphan marker"
else
    output=$(PATH="$recycle_bin:$sandbox/lsofbin:$PATH" \
        SRUI_TEST_LSOF_TABLE="$lsof_table" \
        SRUI_REAP_PATTERN="$(marker_pattern "$marker")" \
        SRUI_REAP_AGE_MINUTES=0 \
        SRUI_REAP_TMP_GLOBS="$sandbox/tmp/srui-*" \
        bash "$reaper" 2>&1)
    assert_alive "$pid" "a pid whose identity changed was not signalled"
    assert_contains "$output" "no longer the process selected" "the sweep said why it held off"
    assert_contains "$output" "1 replaced before signalling" "the summary accounted for it"
    if printf '%s' "$output" | grep -qF "killing orphaned fixture server pid $pid"; then
        fail "the reaper announced a kill for a replaced pid"
        printf '%s\n' "    reaper said: $output" >&2
    else
        pass "the reaper never announced a kill for it"
    fi
fi
kill -9 "${pid:-0}" 2>/dev/null

echo "case 18: the identity a signal is gated on comes from the snapshot that selected the pid"
# Case 16 proves a changed identity stops the signal; this one proves *where* the identity the
# reaper compares against was read. It has to be the same `ps` snapshot that selected the pid: an
# identity read a second time, after selection, leaves a window in which the candidate exits, its
# number is reissued, and the replacement's start time is recorded as the selected identity - after
# which every later check agrees and the reaper SIGTERMs, then SIGKILLs, an unrelated process of
# this user. The only way to close that window is to make no second read, so that is what is
# asserted: one identity probe per candidate on the way to a kill, not two.
#
# Mutation to confirm this case bites: record the identity in the rule 6 loop with
# `start=$(process_start "$pid")` instead of taking it from the snapshot. The probe count becomes 2
# and this case fails.
probe_bin="$sandbox/probebin"
mkdir -p "$probe_bin"
cat >"$probe_bin/ps" <<'SH'
#!/bin/sh
# Truthful throughout - the point is not what `ps` answers but how often the reaper asks. Every
# `-o lstart= -p N` identity probe is logged with the pid it asked about.
if [ "$1" = "-o" ] && [ "$2" = "lstart=" ] && [ "$3" = "-p" ]; then
    echo "$4" >>"$SRUI_TEST_PROBE_LOG"
fi
exec /bin/ps "$@"
SH
chmod +x "$probe_bin/ps"
probe_log="$sandbox/identity-probes"
: >"$probe_log"
marker=$(make_marker fixture-one-probe)
spawn_orphan "$marker"
pid=$spawned_pid
if [ -z "$pid" ]; then
    fail "could not spawn an orphan marker"
else
    output=$(PATH="$probe_bin:$sandbox/lsofbin:$PATH" \
        SRUI_TEST_PROBE_LOG="$probe_log" \
        SRUI_TEST_LSOF_TABLE="$lsof_table" \
        SRUI_REAP_PATTERN="$(marker_pattern "$marker")" \
        SRUI_REAP_AGE_MINUTES=0 \
        SRUI_REAP_TMP_GLOBS="$sandbox/tmp/srui-*" \
        bash "$reaper" 2>&1)
    # The kill has to happen, or the probe count below would be satisfied by a sweep that selected
    # nothing - and it also proves the identity the snapshot built matches what `process_start`
    # reports, byte for byte, since a mismatch would stop the signal.
    assert_terminated "$pid" "the orphan was still killed with its identity taken from the snapshot"
    assert_contains "$output" "killing orphaned fixture server pid $pid" "the reaper announced the kill"
    probes=$(grep -c "^$pid\$" "$probe_log" 2>/dev/null || true)
    if [ "${probes:-0}" -eq 1 ]; then
        pass "the identity was read once, immediately before the signal"
    else
        fail "the reaper made ${probes:-0} identity probes for pid $pid, not 1"
        printf '%s\n' "    reaper said: $output" >&2
    fi
fi
kill -9 "${pid:-0}" 2>/dev/null

echo "case 19: a daemon on a default socket outside this shell's runtime directory is never killed"
# The finding: the directory list rule 5 matches against is derived from the *reaper's*
# `XDG_RUNTIME_DIR`/`TMPDIR`, so a `srui-sessiond` detached from a shell with different values holds
# a default socket that appears under none of those directories - and the sweep killed it, taking
# the authoritative session with it. Both shapes of `unix_security::default_socket_path` are staged
# under runtime directories this sweep is pointed away from: the `srui-<uid>` directory a daemon gets
# under any `TMPDIR`, and the `srui-sessiond.sock` file name it gets under any `XDG_RUNTIME_DIR`,
# whose directory carries no recognizable spelling at all.
#
# `nc -lU` through a symlink stands in for the daemon, as in case 11: ppid 1, old enough, argv[0]
# matching the fixture pattern, and really holding the socket it is recorded as holding.
away_tmp="$sandbox/t4"
mkdir -p "$away_tmp"
if ! command -v nc >/dev/null 2>&1; then
    fail "nc is unavailable; cannot stage a daemon on a foreign default socket"
else
    probe=0
    for spec in "$sandbox/srui-$(id -u)|s|a foreign TMPDIR's srui-$(id -u) directory" \
        "$sandbox/x|srui-sessiond.sock|a foreign XDG_RUNTIME_DIR's default socket name"; do
        probe=$((probe + 1))
        foreign_dir=${spec%%|*}
        rest=${spec#*|}
        socket_name=${rest%%|*}
        label=${rest#*|}
        socket_path="$foreign_dir/$socket_name"
        mkdir -p "$foreign_dir"
        # An absolute unix socket path is capped at 104 bytes and the sandbox already spends most
        # of them; a host with a long TMPDIR cannot stage this, and says so rather than passing.
        if [ "${#socket_path}" -gt 100 ]; then
            echo "  skipped: $socket_path is ${#socket_path} bytes, too long to bind here"
            continue
        fi
        listener=$(make_marker "fixture-foreign-runtime-$probe")
        ln -sf "$(command -v nc)" "$listener"
        ("$listener" -lU "$socket_path" >/dev/null 2>&1 &) 2>/dev/null
        listener_pid=""
        for _ in $(seq 1 50); do
            listener_pid=$(pgrep -f "^$listener -lU" 2>/dev/null | head -1)
            [ -n "$listener_pid" ] && [ -S "$socket_path" ] && break
            sleep 0.1
        done
        if [ -z "$listener_pid" ] || [ ! -S "$socket_path" ]; then
            fail "could not stage a daemon bound to $socket_path"
            kill -9 "${listener_pid:-0}" 2>/dev/null
            continue
        fi
        spawned_pids+=("$listener_pid")
        socket_table_add "$listener_pid" "$socket_path"
        # TMPDIR and XDG_RUNTIME_DIR both point somewhere the staged socket is *not*, so the
        # directory list the reaper derives cannot cover it. Without that the case would pass
        # through the pre-existing rule and prove nothing.
        output=$(PATH="$sandbox/lsofbin:$PATH" \
            SRUI_TEST_LSOF_TABLE="$lsof_table" \
            SRUI_REAP_PATTERN="$(marker_pattern "$listener")" \
            SRUI_REAP_AGE_MINUTES=0 \
            SRUI_REAP_TMP_GLOBS="$away_tmp/srui-*" \
            TMPDIR="$away_tmp" \
            XDG_RUNTIME_DIR="$away_tmp/xdg" \
            bash "$reaper" 2>&1)
        assert_alive "$listener_pid" "the daemon on $label survived a zero-age sweep"
        if printf '%s' "$output" | grep -qF "killing orphaned fixture server pid $listener_pid"; then
            fail "the reaper announced a kill for the daemon on $label"
            printf '%s\n' "    reaper said: $output" >&2
        else
            pass "the reaper never selected the daemon on $label"
        fi
        kill -9 "$listener_pid" 2>/dev/null
        rm -f "$socket_path"
    done
fi

echo "case 15: the host's own lsof, where it has one, yields the same decision"
# Every case above drives a stand-in, which tests the reaper's *logic* but not its reading of real
# `lsof -F pn` output. This case closes that gap wherever the host can: a real holder, the real
# binary, no stand-in on PATH. It is skipped - loudly, never silently - where `lsof` is absent or
# denied, which is precisely the environment the stand-in exists for.
real_lsof_dir="$sandbox/tmp/srui-real"
mkdir -p "$real_lsof_dir"
if ! command -v lsof >/dev/null 2>&1; then
    echo "  skipped: no lsof on this host (the stand-in above covered the logic)"
else
    # A *bound* socket, not merely a path on a command line: `spawn_socket_holder` names its path
    # in argv and binds nothing, so the kernel's table would never mention it and this case would
    # skip itself for the wrong reason.
    cat >"$sandbox/bin/real-binder.py" <<PY
import socket, time

sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.bind("$real_lsof_dir/s")
sock.listen(1)
time.sleep(600)
PY
    python3 "$sandbox/bin/real-binder.py" >/dev/null 2>&1 &
    real_holder=$!
    spawned_pids+=("$real_holder")
    disown "$real_holder" 2>/dev/null
    for _ in $(seq 1 50); do
        [ -S "$real_lsof_dir/s" ] && break
        sleep 0.1
    done
    # Captured and matched with `case`, never `lsof | grep -q`: under `set -o pipefail` that
    # pipeline reports 141 when grep exits early on a match and lsof dies of SIGPIPE, so the case
    # would skip itself exactly when the socket *was* found. Measured here before this form.
    real_holders=$(lsof -n -P -U -F pn 2>/dev/null)
    if ! case $real_holders in *"$real_lsof_dir/s"*) true ;; *) false ;; esac; then
        echo "  skipped: lsof is present but reports no unix sockets here (denied, or sandboxed)"
    else
        # No stand-in and no table: the reaper reads the kernel's table through the real binary.
        SRUI_REAP_PATTERN='NEVER_MATCHES_ANY_EXECUTABLE' \
            SRUI_REAP_AGE_MINUTES=0 \
            SRUI_REAP_TMP_GLOBS="$sandbox/tmp/srui-real*" \
            bash "$reaper" >/dev/null 2>&1
        assert_dir_present "$real_lsof_dir" "real lsof output kept the directory of a held socket"
    fi
    kill -9 "${real_holder:-0}" 2>/dev/null
fi

echo
if [ "$failures" -eq 0 ]; then
    echo "reap-test-servers selection rules: all cases passed."
    exit 0
fi
echo "reap-test-servers selection rules: $failures assertion(s) failed." >&2
exit 1
