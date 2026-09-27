#!/usr/bin/env bash
# Reap fixture servers and socket directories that a dead test run left behind.
#
# The leak: a test spawns a fixture server (`counter`, `srui-sessiond`, `coding-agent-demo`,
# `srtop`) and shuts it down in a `defer`. When the test binary wedges on inherited stdio, or is
# killed (^C, watchdog, `pkill swift-test`), that `defer` never runs. The server survives, is
# reparented to init, and keeps holding its `/tmp` socket directory. Nothing ever collects it:
# `scripts/run-swift-tests.sh` reaps the process *group* it created, but a bare `swift test` —
# what CI runs and what most people type — leaks everything. Measured once on a dev machine:
# 379 orphaned servers from five worktrees, 14 days old, and 855 stale directories holding
# 49.3 MB of sockets.
#
# Selection rules, and why each guard exists:
#
#   1. orphaned only (`ppid 1`). A fixture process with a live parent belongs to a test run that
#      is still going — possibly in another worktree or another checkout on this machine, run by
#      someone else's agent. Killing it turns someone's passing suite into an inexplicable
#      connection failure. Orphanhood is the one signal that says "the run that owned me is gone",
#      because a process keeps no other link to its creator once it is reparented to init.
#   2. old enough (`SRUI_REAP_AGE_MINUTES`, default 30). Orphanhood alone is not enough: some
#      fixtures are deliberately double-forked, so they read as `ppid 1` seconds after a healthy
#      test spawned them. An age floor well above any single test's runtime keeps those alive.
#   3. our own uid only. Never signal another user's processes, even if the path matches.
#   4. executable path match (`SRUI_REAP_PATTERN`). Matched against argv[0] — the binary's path,
#      not the whole command line — so a grep over arguments cannot make an editor or a log
#      tailer look like a fixture server.
#   5. not bound to the default runtime socket. `ppid 1` is not proof of debris on its own: a
#      `srui-sessiond` a human detached deliberately also reads as `ppid 1` — it ignores SIGHUP
#      precisely so it survives an SSH disconnect (§17, §20.2) — and it is the process every client
#      reaches through `${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}}/srui-<uid>/srui-sessiond.sock`. Killing
#      it discards the authoritative in-memory session. A fixture never binds that path (tests pass
#      an explicit `--socket` under a per-run directory), so holding it is taken as proof that the
#      process is not debris.
#   6. launched from a checkout of *this* repository. The binary's path must lie inside the main
#      checkout or one of its linked worktrees (`git worktree list`), which is the one thing that
#      makes a process *this* repository's test debris rather than some other program with a
#      familiar name. `/usr/local/bin/srui-sessiond`, a sibling project's
#      `target/debug/srui-sessiond`, and anything whose path cannot be determined are never
#      signalled, however leak-shaped they look. Both spellings of the candidate's path are
#      accepted — as `ps` reports it and with its directory resolved through symlinks — because a
#      `target` directory symlinked into a shared build cache must not make a real fixture
#      unreapable; a fixture launched *through* the cache path rather than through the checkout is
#      out of scope, since nothing in this repository launches one that way.
#
#      The cost, deliberately accepted: an orphan left by a run in a worktree that has since been
#      deleted is no longer reapable, because its path is now inside no checkout. Those pids
#      survive every sweep and have to be killed by hand. Stale-worktree pruning makes this a
#      one-way ratchet, and it is the right trade — the alternative is a sweep that can reach
#      binaries this repository never built.
#
#      Ownership evidence finer than rules 5 and 6 — a marker each test run writes for its own
#      fixtures — is still missing; a daemon detached by hand from inside a checkout, onto some
#      explicit socket other than the default, is still read as debris once it is old enough.
#
# Directories (`/tmp/srui-*`, `/tmp/px0*`, `/tmp/srtop-*`) are removed only when no live process
# references them. "Referenced" has two independent sources, and either one spares a directory:
#
#   a. the command lines of every live process — their `--socket` arguments and any other absolute
#      path they carry;
#   b. the kernel's unix-socket table (`lsof -U`), which names the socket a server actually holds
#      even when nothing on its command line does. `srui-sessiond` with no `--socket` computes
#      `${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}}/srui-<uid>/srui-sessiond.sock` internally
#      (`unix_security::default_socket_path`), so a developer's deliberately detached daemon is
#      invisible to (a) alone — and unlinking its socket takes the authoritative session away from
#      every new and reconnecting client. If no socket inventory can be read at all, directory
#      removal is skipped rather than guessed at.
#
# The default runtime directory itself is never removed, whatever the evidence says: no fixture
# uses it (tests always pass an explicit `--socket` under a per-run directory), so there is nothing
# to gain and a live daemon to lose.
#
# Never from mtime: a bound unix socket's mtime does not advance while it is in use, so mtime says
# nothing about whether a server is still serving on it. mtime is used for one narrower purpose
# only — a directory touched within the age window is left alone regardless, because a test that
# has just created its runtime directory may not have spawned the server that names it yet.
#
# Safe to run while tests are running, here or in another checkout: it takes a `ps` snapshot,
# signals only processes no live run can own, and tolerates losing every race (a pid that exits
# first, a directory a second reaper removes first).
#
# Usage: scripts/reap-test-servers.sh [--dry-run]
#   SRUI_REAP_AGE_MINUTES   minimum age, in minutes, of a reapable orphan (default 30); 0 also
#                           disables the directory freshness guard below
#   SRUI_REAP_PATTERN       ERE matched against argv[0] (default: the four fixture binaries)
#   SRUI_REAP_TMP_GLOBS     space-separated globs of socket directories (default: the three /tmp
#                           families above). Overridable so the tests can run in a sandbox.
set -uo pipefail

dry_run=0
while [ $# -gt 0 ]; do
    case $1 in
        --dry-run) dry_run=1 ;;
        -h | --help)
            sed -n '/^# Usage:/,/sandbox\./p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "usage: $0 [--dry-run]" >&2
            exit 2
            ;;
    esac
    shift
done

age_minutes=${SRUI_REAP_AGE_MINUTES:-30}
if ! printf '%s' "$age_minutes" | grep -Eq '^[0-9]+$'; then
    echo "error: SRUI_REAP_AGE_MINUTES must be a non-negative integer, got '$age_minutes'" >&2
    exit 2
fi
age_seconds=$((age_minutes * 60))

pattern=${SRUI_REAP_PATTERN:-'/target/debug/(counter|srui-sessiond|coding-agent-demo|srtop)$'}
tmp_globs=${SRUI_REAP_TMP_GLOBS:-'/tmp/srui-* /tmp/px0* /tmp/srtop-*'}
my_uid=$(id -u)
failures=0

process_snapshot() {
    ps -eo pid=,ppid=,uid=,etime=,command= 2>/dev/null
}

# True when a pid we signalled is no longer running: either gone from the process table, or a
# zombie its parent has not reaped yet. `kill -0` cannot tell those apart from "still serving" —
# it succeeds for a zombie on macOS and on Linux — and every orphan this script kills is a child
# of pid 1, so whether the zombie disappears in microseconds or never is entirely up to that
# process: launchd reaps immediately, a container whose pid 1 is a plain shell never does. Using
# `kill -0` there reported every successfully killed fixture server as having survived SIGKILL,
# which made the sweep exit 1 and `run-swift-tests.sh` print a spurious pre-test warning.
process_terminated() {
    local state
    state=$(ps -o state= -p "$1" 2>/dev/null | tr -d '[:space:]')
    case $state in
        '') return 0 ;; # gone from the process table
        Z*) return 0 ;; # dead, waiting to be reaped
        *) return 1 ;;
    esac
}

# The checkouts of this repository: the main one plus every linked worktree, each in both the
# spelling git reports and its symlink-resolved form. Derived from where this script lives, not from
# the caller's cwd, so a sweep run from anywhere still means "debris of *this* repository". Empty
# when the script is not inside a git checkout, which rule 6 treats as "nothing is reapable".
repo_checkout_roots() {
    local script_dir root line path physical
    script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd) || return 0
    root=$(git -C "$script_dir" rev-parse --show-toplevel 2>/dev/null) || return 0
    [ -n "$root" ] || return 0
    git -C "$root" worktree list --porcelain 2>/dev/null | while IFS= read -r line; do
        case $line in
            worktree\ *) path=${line#worktree } ;;
            *) continue ;;
        esac
        [ -n "$path" ] || continue
        printf '%s\n' "${path%/}"
        physical=$(cd "$path" 2>/dev/null && pwd -P) || continue
        [ "${physical%/}" = "${path%/}" ] || printf '%s\n' "${physical%/}"
    done
}

# Rule 6: is this binary inside a checkout of this repository? An argv[0] that is not an absolute
# path tells us nothing about where the binary lives, so it is not reapable.
inside_repo_checkout() {
    local exe=$1 dir resolved root
    case $exe in
        /*) ;;
        *) return 1 ;;
    esac
    resolved=
    if dir=$(cd "$(dirname "$exe")" 2>/dev/null && pwd -P); then
        resolved="${dir%/}/$(basename "$exe")"
    fi
    while IFS= read -r root; do
        [ -n "$root" ] || continue
        case $exe in "$root"/*) return 0 ;; esac
        [ -n "$resolved" ] || continue
        case $resolved in "$root"/*) return 0 ;; esac
    done <<<"$repo_roots"
    return 1
}

# pid<TAB>argv[0] for every fixture process that is orphaned, old enough, ours, and not holding the
# default runtime socket (rule 5).
select_orphans() {
    awk -v pattern="$pattern" -v min_age="$age_seconds" -v my_uid="$my_uid" -v self="$$" \
        -v spared="$default_socket_pids" '
        BEGIN { n = split(spared, list, " "); for (i = 1; i <= n; i++) keep[list[i] + 0] = 1 }
        function age_seconds(e,   days, part, n, secs, split_day) {
            days = 0
            if (e ~ /-/) { split(e, split_day, "-"); days = split_day[1] + 0; e = split_day[2] }
            n = split(e, part, ":")
            if (n == 3)      secs = part[1] * 3600 + part[2] * 60 + part[3]
            else if (n == 2) secs = part[1] * 60 + part[2]
            else             secs = part[1] + 0
            return days * 86400 + secs
        }
        $1 + 0 == self + 0 { next }
        $1 + 0 in keep { next }                 # serving the default socket: a human started this
        $3 + 0 != my_uid + 0 { next }
        $2 + 0 != 1 { next }                    # live parent: a running test owns this process
        age_seconds($4) < min_age + 0 { next }  # too young to be debris
        $5 ~ pattern { print $1 "\t" $5 }
    '
}

# Absolute paths named on the command line of every live process, minus the ones we just killed.
# `--socket /tmp/...`, `--socket=/tmp/...` and bare path arguments all land here.
referenced_paths() {
    local killed_pids=$1
    awk -v killed="$killed_pids" '
        BEGIN { n = split(killed, list, " "); for (i = 1; i <= n; i++) dead[list[i] + 0] = 1 }
        $1 + 0 in dead { next }
        {
            for (i = 5; i <= NF; i++) {
                token = $i
                sub(/^--[A-Za-z0-9-]+=/, "", token)
                if (token ~ /^\//) print token
            }
        }
    '
}

# pid<TAB>socket path for every unix socket a live process holds, read from the kernel's socket
# table rather than from any command line: a server that computed its socket path internally is
# invisible to a command-line scan but not to this. Relative names (a bind against a directory fd)
# carry no directory and are dropped. Empty output means "no evidence available", not "nothing is
# held", and every caller below fails safe on it.
socket_holders() {
    lsof -n -P -U -F pn 2>/dev/null | awk '
        /^p/ { pid = substr($0, 2) + 0; next }
        /^n\// { print pid "\t" substr($0, 2) }
    '
}

# The socket paths from that snapshot, minus the ones held by processes we just killed.
held_socket_paths() {
    local killed_pids=$1
    awk -F'\t' -v killed="$killed_pids" '
        BEGIN { n = split(killed, list, " "); for (i = 1; i <= n; i++) dead[list[i] + 0] = 1 }
        $1 + 0 in dead { next }
        { print $2 }
    '
}

# The runtime directory the servers pick when no socket is given. Both spellings are listed because
# `std::env::temp_dir()` is `$TMPDIR` where it is set (macOS) and `/tmp` where it is not (Linux).
default_tmp=${TMPDIR:-/tmp}
default_runtime_dirs="${XDG_RUNTIME_DIR:-} ${default_tmp%/}/srui-$my_uid /tmp/srui-$my_uid"

# One socket-table snapshot, taken before any kill, serves both rule 5 and the directory sweep.
socket_snapshot=$(socket_holders)
default_socket_pids=$(printf '%s\n' "$socket_snapshot" |
    awk -F'\t' -v dirs="$default_runtime_dirs" '
        BEGIN { n = split(dirs, list, " ") }
        {
            for (i = 1; i <= n; i++) {
                if (list[i] != "" && index($2, list[i] "/") == 1) { print $1; next }
            }
        }
    ' | sort -u | tr '\n' ' ')

repo_roots=$(repo_checkout_roots)
if [ -z "$repo_roots" ]; then
    echo "note: not inside a git checkout; no process is reapable (rule 6)" >&2
fi

snapshot=$(process_snapshot)
if [ -z "$snapshot" ]; then
    echo "error: could not read the process table (ps produced nothing)" >&2
    exit 1
fi

matched_total=$(printf '%s\n' "$snapshot" |
    awk -v pattern="$pattern" -v my_uid="$my_uid" '$3 + 0 == my_uid + 0 && $5 ~ pattern' |
    wc -l | tr -d ' ')
orphans=$(printf '%s\n' "$snapshot" | select_orphans)

# Rule 6, applied here rather than in the awk above because resolving a path through symlinks needs
# a shell. A candidate outside every checkout of this repository is not this repository's debris.
foreign=0
reapable=
while IFS=$'\t' read -r pid exe; do
    [ -n "${pid:-}" ] || continue
    if inside_repo_checkout "$exe"; then
        reapable="${reapable}${pid}"$'\t'"${exe}"$'\n'
    else
        foreign=$((foreign + 1))
    fi
done <<<"$orphans"
orphans=${reapable%$'\n'}

killed=0
killed_pids=
while IFS=$'\t' read -r pid exe; do
    [ -n "${pid:-}" ] || continue
    if [ "$dry_run" -eq 1 ]; then
        echo "would kill pid $pid $exe"
        killed=$((killed + 1))
        killed_pids="$killed_pids $pid"
        continue
    fi
    echo "killing orphaned fixture server pid $pid $exe"
    kill -TERM "$pid" 2>/dev/null
    for _ in $(seq 1 20); do
        process_terminated "$pid" && break
        sleep 0.1
    done
    if ! process_terminated "$pid"; then
        kill -KILL "$pid" 2>/dev/null
        sleep 0.2
    fi
    if ! process_terminated "$pid"; then
        echo "error: pid $pid survived SIGKILL" >&2
        failures=$((failures + 1))
        continue
    fi
    killed=$((killed + 1))
    killed_pids="$killed_pids $pid"
done <<<"$orphans"

# Re-read the process table after the kills so a directory held only by a server we just reaped
# becomes collectable in this same pass. `--dry-run` reuses the original snapshot minus the pids it
# reported, so its directory list is what a real run would remove, not what is collectable today.
if [ "$dry_run" -eq 1 ]; then
    live_snapshot=$snapshot
else
    live_snapshot=$(process_snapshot)
fi
referenced=$(printf '%s\n' "$live_snapshot" | referenced_paths "$killed_pids")
held_sockets=$(printf '%s\n' "$socket_snapshot" | held_socket_paths "$killed_pids")
socket_evidence=1
if [ -z "$socket_snapshot" ]; then
    socket_evidence=0
    echo "note: no unix socket inventory (lsof); leaving every socket directory in place" >&2
fi
referenced=$(printf '%s\n%s\n' "$referenced" "$held_sockets")

# shellcheck disable=SC2086 # deliberate word splitting: tmp_globs is a list of globs
set -- $tmp_globs
candidates=()
for glob in "$@"; do
    for path in $glob; do
        [ -d "$path" ] || continue
        candidates+=("$path")
    done
done

removed=0
removed_kb=0
held=0
fresh=0
for dir in "${candidates[@]+"${candidates[@]}"}"; do
    if [ "$socket_evidence" -eq 0 ]; then
        held=$((held + 1))
        continue
    fi
    # The default runtime directory belongs to whatever daemon a human started, never to a test.
    # shellcheck disable=SC2086 # deliberate word splitting: a space-separated list of directories
    if printf '%s\n' $default_runtime_dirs |
        awk -v dir="$dir" '$0 != "" && $0 == dir { found = 1 } END { exit !found }'; then
        held=$((held + 1))
        continue
    fi
    if printf '%s\n' "$referenced" |
        awk -v dir="$dir" '$0 == dir || index($0, dir "/") == 1 { found = 1 } END { exit !found }'; then
        held=$((held + 1))
        continue
    fi
    # Setup-race guard, not an idle signal: a directory touched inside the age window may belong to
    # a test that has not spawned its server yet. `SRUI_REAP_AGE_MINUTES=0` disables it along with
    # the process age floor (BSD find's `-mmin -0` matches sub-second-old paths, so the zero case
    # is spelled out rather than left to find).
    if [ "$age_minutes" -gt 0 ] &&
        [ -n "$(find "$dir" -maxdepth 0 -mmin -"$age_minutes" 2>/dev/null)" ]; then
        fresh=$((fresh + 1))
        continue
    fi
    size_kb=$(du -sk "$dir" 2>/dev/null | awk '{print $1 + 0}')
    if [ "$dry_run" -eq 1 ]; then
        echo "would remove $dir"
    else
        echo "removing unreferenced socket directory $dir"
        if ! rm -rf -- "$dir" 2>/dev/null && [ -e "$dir" ]; then
            echo "error: could not remove $dir" >&2
            failures=$((failures + 1))
            continue
        fi
    fi
    removed=$((removed + 1))
    removed_kb=$((removed_kb + ${size_kb:-0}))
done

spared=$((matched_total - killed - foreign))
verb_killed=killed
verb_removed=removed
if [ "$dry_run" -eq 1 ]; then
    verb_killed="would kill"
    verb_removed="would remove"
fi
printf 'reap-test-servers: %s %d fixture server(s), %s %d socket director(y|ies) (%d.%d MB); left %d fixture process(es) with a live parent or too young, %d outside this repository, %d referenced director(y|ies), %d recently touched.\n' \
    "$verb_killed" "$killed" "$verb_removed" "$removed" \
    "$((removed_kb / 1024))" "$(((removed_kb % 1024) * 10 / 1024))" \
    "$spared" "$foreign" "$held" "$fresh"

if [ "$failures" -gt 0 ]; then
    echo "reap-test-servers: $failures failure(s)" >&2
    exit 1
fi
exit 0
