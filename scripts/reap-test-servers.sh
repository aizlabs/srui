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
#
# Directories (`/tmp/srui-*`, `/tmp/px0*`, `/tmp/srtop-*`) are removed only when no live process
# references them. "Referenced" is derived from the command lines of every live process (their
# `--socket` arguments and any other absolute path they carry), never from mtime: a bound unix
# socket's mtime does not advance while it is in use, so mtime says nothing about whether a
# server is still serving on it. mtime is used for one narrower purpose only — a directory
# touched within the age window is left alone regardless, because a test that has just created
# its runtime directory may not have spawned the server that names it yet, and some tests bind
# the socket in-process so no command line ever mentions the path.
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

# pid<TAB>argv[0] for every fixture process that is orphaned, old enough, and ours.
select_orphans() {
    awk -v pattern="$pattern" -v min_age="$age_seconds" -v my_uid="$my_uid" -v self="$$" '
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

snapshot=$(process_snapshot)
if [ -z "$snapshot" ]; then
    echo "error: could not read the process table (ps produced nothing)" >&2
    exit 1
fi

matched_total=$(printf '%s\n' "$snapshot" |
    awk -v pattern="$pattern" -v my_uid="$my_uid" '$3 + 0 == my_uid + 0 && $5 ~ pattern' |
    wc -l | tr -d ' ')
orphans=$(printf '%s\n' "$snapshot" | select_orphans)

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
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.1
    done
    if kill -0 "$pid" 2>/dev/null; then
        kill -KILL "$pid" 2>/dev/null
        sleep 0.2
    fi
    if kill -0 "$pid" 2>/dev/null; then
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

spared=$((matched_total - killed))
verb_killed=killed
verb_removed=removed
if [ "$dry_run" -eq 1 ]; then
    verb_killed="would kill"
    verb_removed="would remove"
fi
printf 'reap-test-servers: %s %d fixture server(s), %s %d socket director(y|ies) (%d.%d MB); left %d fixture process(es) with a live parent or too young, %d referenced director(y|ies), %d recently touched.\n' \
    "$verb_killed" "$killed" "$verb_removed" "$removed" \
    "$((removed_kb / 1024))" "$(((removed_kb % 1024) * 10 / 1024))" \
    "$spared" "$held" "$fresh"

if [ "$failures" -gt 0 ]; then
    echo "reap-test-servers: $failures failure(s)" >&2
    exit 1
fi
exit 0
