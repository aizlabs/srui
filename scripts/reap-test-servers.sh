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
#      argv[0] is recovered from the command line `ps` prints, and that recovery is not a column
#      index: `ps` joins argv with spaces, and a checkout path may contain one. Reading argv[0] as
#      awk's `$10` — the first whitespace token after the nine fixed columns — read a real orphan
#      under `…/my checkout/target/debug/counter` as `/tmp/reap`, matched nothing, and swept an
#      all-zeros no-op while the orphan survived (measured; Chrome reads as `/Applications/Google`
#      the same way). So argv[0] is the longest prefix of the command line that names an existing
#      file, which only the filesystem can decide: `/tmp/a b/counter --socket x` and `/tmp/a` run
#      with argument `b/counter` are the same bytes. awk pre-filters on *some* prefix matching, the
#      shell then re-tests the pattern against that one prefix alone, so rule 4's guarantee holds.
#      Out of scope, and the fallback is the first token: a path containing a run of whitespace, or
#      one whose binary has since been deleted.
#   5. not bound to a default runtime socket. `ppid 1` is not proof of debris on its own: a
#      `srui-sessiond` a human detached deliberately also reads as `ppid 1` — it ignores SIGHUP
#      precisely so it survives an SSH disconnect (§17, §20.2) — and it is the process every client
#      reaches through `unix_security::default_socket_path`: `$XDG_RUNTIME_DIR/srui-sessiond.sock`
#      where that variable is set, and `${TMPDIR:-/tmp}/srui-<uid>/srui-sessiond.sock` where it is
#      not. Killing it discards the authoritative in-memory session. A fixture never binds that
#      path (tests pass an explicit `--socket` under a per-run directory), so holding it is taken as
#      proof that the process is not debris.
#
#      Recognized by *shape*, not only by this sweep's own environment. A daemon launched from a
#      shell whose `XDG_RUNTIME_DIR` or `TMPDIR` differs from the one running the sweep serves a
#      default socket that lies outside every directory this script can derive, and matching the
#      reaper-derived directory list alone left exactly that daemon unprotected — the authoritative
#      session, killed because it was started in a different shell. So two shapes in the socket
#      inventory spare their holder as well, wherever the socket lives: a path with a directory
#      component named `srui-<uid>` (the default runtime directory under any `TMPDIR`), and a path
#      whose file name is `srui-sessiond.sock` (the default name under any `XDG_RUNTIME_DIR`, whose
#      directory has no recognizable spelling at all). Neither shape can come from a fixture:
#      `srui-sessiond.sock` is produced by nothing but `default_socket_path`, and no test names it.
#      The environment-derived directory list is still consulted on top of the shapes, because it
#      also protects default sockets that are not sessiond's, such as srtop's.
#
#      This rule needs the socket table: where `lsof` is missing or denied, nothing can be told
#      apart from that daemon, so the sweep kills nothing at all unless it can rule the daemon out
#      independently — no socket in any default runtime directory it can see. That fallback stays
#      environment-bound, and cannot be made otherwise: with no inventory there is nothing to read
#      a foreign `TMPDIR` or `XDG_RUNTIME_DIR` out of, so a daemon serving a default socket under
#      one is invisible to the sweep. Residual exposure, left open deliberately: on a host with no
#      `lsof`, a daemon whose runtime directory the reaper cannot name is spared only if rule 6
#      spares it — an installed `/usr/local/bin/srui-sessiond` is safe, one built in a checkout of
#      this repository and detached by hand is not.
#   6. launched from a checkout of *this* repository. The binary's path, with its directory resolved
#      through symlinks, must lie inside the main checkout or one of its linked worktrees
#      (`git worktree list`), which is the one thing that makes a process *this* repository's test
#      debris rather than some other program with a familiar name. `/usr/local/bin/srui-sessiond`, a
#      sibling project's `target/debug/srui-sessiond`, and anything whose path cannot be determined
#      are never signalled, however leak-shaped they look.
#
#      Only the resolved spelling is compared, never the one `ps` reports. Comparing the reported
#      spelling — even with `.` and `..` folded out of it — admits any foreign path reachable
#      through a symlink inside a checkout: with `repo/cache -> ../sibling`, a candidate
#      `…/repo/cache/target/debug/srui-sessiond` carries the checkout's prefix letter for letter
#      while living in another project, and the sweep killed that project's daemon (measured).
#      The one thing the reported spelling was there to rescue — a worktree whose `target` is a
#      symlink into a shared build cache, where every ordinary fixture's physical path lies outside
#      the checkout — is served instead by naming the cache itself as a root: every `target` symlink
#      inside a checkout contributes its resolved destination to the root set. Residual, accepted
#      deliberately: a binary in such a cache is then reapable however it was launched, including
#      one that got there because a *different* project pointed its own `target` at the same cache.
#      A build cache a checkout of this repository points into is in scope; a symlink anywhere else
#      in the tree is not. A candidate whose directory no longer exists is also spared, since there
#      is nothing left to resolve.
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
#      `$XDG_RUNTIME_DIR/srui-sessiond.sock`, or `${TMPDIR:-/tmp}/srui-<uid>/srui-sessiond.sock`,
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
# An unusable pattern must say so rather than sweep quietly: an invalid ERE made awk fail on every
# line and the run print a clean all-zeros summary, which reads exactly like "nothing to reap".
# Checked by the same engine that will use it, so the verdict cannot disagree with the matcher.
if ! awk -v pattern="$pattern" 'BEGIN { if ("" ~ pattern) exit 0 }' 2>/dev/null; then
    echo "error: SRUI_REAP_PATTERN is not a regular expression awk accepts: '$pattern'" >&2
    exit 2
fi
# `/tmp/px0*` used to be the Process Explorer pattern; it also matches an unrelated `/tmp/px0-cache`.
# The fixtures name their directories `px0NN-...`, so the ticket digits are spelled out.
tmp_globs=${SRUI_REAP_TMP_GLOBS:-'/tmp/srui-* /tmp/px0[0-9][0-9]-* /tmp/srtop-*'}
my_uid=$(id -u)
failures=0

# One snapshot of the process table, carrying everything every rule needs — including each
# process's absolute start time, so that selection and identity come from the *same* observation.
# `lstart=` is exactly five whitespace-separated tokens (`Thu Oct  1 13:54:00 2026`) on both macOS
# and Linux, so the columns awk sees are: $1 pid, $2 ppid, $3 uid, $4 etime, $5..$9 lstart,
# $10 argv[0], $11.. the rest of the command line.
process_snapshot() {
    ps -eo pid=,ppid=,uid=,etime=,lstart=,command= 2>/dev/null
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

# A pid's absolute start time *now*: the one property that distinguishes a process from a later one
# that inherited its number. Empty when the pid is gone.
#
# This matters because selection and signalling are not simultaneous. Each candidate can occupy the
# kill loop for up to 2.2s (SIGTERM, twenty 0.1s liveness polls, SIGKILL, 0.2s), so with several
# candidates a *later* one has seconds in which to exit on its own and have its number handed to an
# unrelated process of this same user - which the unconditional kill would then signal, and the
# escalation below would SIGKILL. Every signal is therefore gated on the identity recorded at
# selection time.
#
# Used only for those pre-signal re-checks, never to record the identity: reading it here would be a
# second observation, and a pid recycled between the selecting snapshot and that read would have the
# *replacement's* start time recorded as the selected identity - after which every later check
# agrees, and the reaper SIGTERMs (then SIGKILLs) an unrelated process. The recorded identity comes
# from `process_snapshot`, and the value built there is byte-identical to this one: five tokens
# joined by single spaces, which is what squeezing the whitespace out of `ps -o lstart=` produces.
process_start() {
    ps -o lstart= -p "$1" 2>/dev/null | tr -s '[:space:]' ' ' | sed 's/^ //; s/ $//'
}

# True when `pid` is still the process selected, identified by its start time.
still_the_selected_process() {
    local pid=$1 recorded=$2 current
    current=$(process_start "$pid")
    [ -n "$current" ] && [ "$current" = "$recorded" ]
}

# The checkouts of this repository, as physical paths: the main one plus every linked worktree, with
# `.`, `..` and every symlink component resolved away by `pwd -P`, plus the destination of every
# `target` symlink inside them (see rule 6). Derived from where this script lives, not from the
# caller's cwd, so a sweep run from anywhere still means "debris of *this* repository". Empty when
# the script is not inside a git checkout, which rule 6 treats as "nothing is reapable".
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
        physical=$(cd "$path" 2>/dev/null && pwd -P) || continue
        printf '%s\n' "${physical%/}"
        target_cache_roots "${physical%/}"
    done
}

# Where a checkout's `target` symlinks actually point: the shared build cache layout, in which the
# physical path of a perfectly ordinary fixture binary lies outside the checkout. Naming the cache as
# a root is what keeps those fixtures reapable now that only resolved paths are compared.
#
# A real `target` directory is pruned rather than descended into - it holds the whole build output,
# and walking it on every sweep (this script runs before each test run) would cost more than the
# sweep does. `-maxdepth 3` covers the layouts this repository has: `target`, `<crate>/target` and
# `<group>/<crate>/target`.
target_cache_roots() {
    local root=$1 link dir
    find "$root" -maxdepth 3 \( -name .git -o -name target \) \
        \( -type l -print -o -prune \) 2>/dev/null | while IFS= read -r link; do
        [ -L "$link" ] || continue
        dir=$(cd "$link" 2>/dev/null && pwd -P) || continue
        printf '%s\n' "${dir%/}"
    done
}

# Rule 6: is this binary inside a checkout of this repository? An argv[0] that is not an absolute
# path tells us nothing about where the binary lives, so it is not reapable.
#
# Only the resolved spelling is compared; see rule 6 for why the reported one cannot be trusted.
# The *directory* is resolved and the final component is not: a binary in a `target/debug` may
# itself be a symlink (in the self-tests it always is), and resolving that would compare some
# altogether different file against the roots.
inside_repo_checkout() {
    local exe=$1 dir resolved root
    case $exe in
        /*) ;;
        *) return 1 ;;
    esac
    dir=$(cd "$(dirname "$exe")" 2>/dev/null && pwd -P) || return 1
    resolved="${dir%/}/$(basename "$exe")"
    while IFS= read -r root; do
        [ -n "$root" ] || continue
        case $resolved in "$root"/*) return 0 ;; esac
    done <<<"$repo_roots"
    return 1
}

# The command line as `ps` printed it, with the nine fixed columns in front of it removed: pid,
# ppid, uid, etime and the five tokens of `lstart`. Stripped token by token rather than with an
# interval expression, which not every awk supports, and the internal spacing of the command line is
# left exactly as it was - which is the whole point, since argv[0] can contain a space.
AWK_COMMAND_LINE='
    function command_line(line,   i) {
        for (i = 1; i <= 9; i++) sub(/^[[:space:]]*[^[:space:]]+/, "", line)
        sub(/^[[:space:]]+/, "", line)
        return line
    }
'

# pid<TAB>ppid<TAB>age in seconds<TAB>start<TAB>command line, for every process of ours whose command
# line has *some* prefix matching the pattern - the pre-filter for rule 4, which the shell then
# narrows to argv[0] alone (see `command_argv0`). A prefix test cannot miss a matching argv[0],
# because argv[0] is itself one of the prefixes.
#
# The start time is read out of the very snapshot that selects the pid, so no window exists in which
# the number could be reissued before its identity is recorded; see `process_start`.
select_pattern_candidates() {
    awk -v pattern="$pattern" -v my_uid="$my_uid" -v self="$$" "$AWK_COMMAND_LINE"'
        function age_seconds(e,   days, part, n, secs, split_day) {
            days = 0
            if (e ~ /-/) { split(e, split_day, "-"); days = split_day[1] + 0; e = split_day[2] }
            n = split(e, part, ":")
            if (n == 3)      secs = part[1] * 3600 + part[2] * 60 + part[3]
            else if (n == 2) secs = part[1] * 60 + part[2]
            else             secs = part[1] + 0
            return days * 86400 + secs
        }
        function prefix_matches(cmd, pat,   i, n, parts, prefix) {
            n = split(cmd, parts, " ")
            for (i = 1; i <= n; i++) {
                prefix = (i == 1 ? parts[1] : prefix " " parts[i])
                if (prefix ~ pat) return 1
            }
            return 0
        }
        $1 + 0 == self + 0 { next }
        $3 + 0 != my_uid + 0 { next }
        {
            cmd = command_line($0)
            if (!prefix_matches(cmd, pattern)) next
            print $1 "\t" $2 "\t" age_seconds($4) "\t" $5 " " $6 " " $7 " " $8 " " $9 "\t" cmd
        }
    '
}

# argv[0] out of a command line, decided by the filesystem: the longest prefix that names something
# that exists. See rule 4 for why no amount of string surgery can do it instead.
command_argv0() {
    local cmd=$1 prefix= best= token
    local -a tokens=()
    read -r -a tokens <<<"$cmd" # `read -a`, never `for token in $cmd`: no glob expansion
    for token in "${tokens[@]+"${tokens[@]}"}"; do
        if [ -z "$prefix" ]; then prefix=$token; else prefix="$prefix $token"; fi
        if [ -e "$prefix" ] || [ -L "$prefix" ]; then best=$prefix; fi
    done
    printf '%s' "${best:-${tokens[0]:-}}"
}

# Absolute paths named on the command line of every live process, minus the ones we just killed.
# `--socket /tmp/...`, `--socket=/tmp/...` and bare path arguments all land here. argv[0] is scanned
# along with the arguments, and a fragment of one containing a space (`/tmp/with`) lands here as a
# path of its own - which can only spare a directory, never remove one.
referenced_paths() {
    local killed_pids=$1
    awk -v killed="$killed_pids" "$AWK_COMMAND_LINE"'
        BEGIN { n = split(killed, list, " "); for (i = 1; i <= n; i++) dead[list[i] + 0] = 1 }
        $1 + 0 in dead { next }
        {
            n = split(command_line($0), parts, " ")
            for (i = 1; i <= n; i++) {
                token = parts[i]
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
#
# Derived from *this* shell's environment, so it only names the directories a daemon started from a
# shell like this one would use. The shapes below are what recognize one started from a shell with a
# different `XDG_RUNTIME_DIR` or `TMPDIR`.
default_tmp=${TMPDIR:-/tmp}
default_runtime_dirs="${XDG_RUNTIME_DIR:-} ${default_tmp%/}/srui-$my_uid /tmp/srui-$my_uid"

# The environment-independent shapes of a default socket (`unix_security::default_socket_path`):
# the `srui-<uid>` directory component it gets under any `TMPDIR`, and the file name it gets under
# any `XDG_RUNTIME_DIR` — where the directory is whatever that variable said and has no
# recognizable spelling. No fixture produces either shape.
default_runtime_leaf="srui-$my_uid"
default_socket_name="srui-sessiond.sock"

# Any socket sitting in a default runtime directory, bound or stale. Used only when the socket
# table cannot be read: it answers "could there be a daemon here to protect?" without naming who
# holds what.
#
# Limited to the directories this shell's environment names, and that cannot be fixed: the shapes
# above need a socket inventory to match against, and without one there is nothing to discover an
# arbitrary `TMPDIR` or `XDG_RUNTIME_DIR` from — a filesystem-wide search for a default socket is
# not something a pre-test sweep can do. So on a host with no `lsof`, a daemon serving a default
# socket under a runtime directory unlike this shell's is invisible here; rule 6 still spares an
# installed one, while one built in this repository and detached by hand stays at risk.
default_socket_present() {
    local dir entry
    # shellcheck disable=SC2086 # deliberate word splitting: a space-separated list of directories
    for dir in $default_runtime_dirs; do
        [ -n "$dir" ] || continue
        [ -d "${dir%/}" ] || continue
        for entry in "${dir%/}"/*; do
            [ -S "$entry" ] && return 0
        done
    done
    return 1
}

# One socket-table snapshot, taken before any kill, serves both rule 5 and the directory sweep.
socket_snapshot=$(socket_holders)
socket_evidence=1
[ -n "$socket_snapshot" ] || socket_evidence=0
default_socket_pids=$(printf '%s\n' "$socket_snapshot" |
    awk -F'\t' -v dirs="$default_runtime_dirs" -v leaf="$default_runtime_leaf" \
        -v sock_name="$default_socket_name" '
        BEGIN { n = split(dirs, list, " ") }
        {
            # A directory this sweep can name: protects every default socket under it, sessiond or
            # not (srtop and the demos have their own default names).
            for (i = 1; i <= n; i++) {
                if (list[i] != "" && index($2, list[i] "/") == 1) { print $1; next }
            }
            # Shape, for a daemon whose runtime directory this sweep cannot name: the default
            # runtime directory under a `TMPDIR` other than ours...
            if (index($2, "/" leaf "/") > 0) { print $1; next }
            # ...and the default socket file name under an `XDG_RUNTIME_DIR` other than ours.
            name = $2
            sub(/^.*\//, "", name)
            if (name == sock_name) { print $1; next }
        }
    ' | sort -u | tr '\n' ' ')

# Rule 5 is only enforceable while the socket table is readable. Without `lsof` — missing, or denied
# — `default_socket_pids` is empty, and a deliberately detached daemon serving the default socket
# becomes indistinguishable from debris, so it would be signalled by rules 1-4 and 6 alone. Killing
# is therefore gated on being able to rule that daemon out independently: no socket in any default
# runtime directory means there is nobody there to protect. A *stale* socket file stops the sweep
# too, which is the safe way round.
kill_allowed=1
if [ "$socket_evidence" -eq 0 ]; then
    if default_socket_present; then
        kill_allowed=0
        echo "note: no unix socket inventory (lsof) and a socket exists in a default runtime" \
            "directory; killing nothing, because rule 5 cannot be enforced" >&2
    else
        echo "note: no unix socket inventory (lsof); no socket in any default runtime directory," \
            "so rule 5 has nothing to protect" >&2
    fi
fi

repo_roots=$(repo_checkout_roots)
if [ -z "$repo_roots" ]; then
    echo "note: not inside a git checkout; no process is reapable (rule 6)" >&2
fi

snapshot=$(process_snapshot)
if [ -z "$snapshot" ]; then
    echo "error: could not read the process table (ps produced nothing)" >&2
    exit 1
fi

# argv[0] for every pre-filtered candidate, which only the filesystem can decide (rule 4). The
# command line stays out of the output from here on: nothing downstream may match on an argument.
resolved=
while IFS=$'\t' read -r pid ppid age start cmd; do
    [ -n "${pid:-}" ] || continue
    resolved="${resolved}${pid}"$'\t'"${ppid}"$'\t'"${age}"$'\t'"${start}"$'\t'"$(command_argv0 "$cmd")"$'\n'
done <<<"$(printf '%s\n' "$snapshot" | select_pattern_candidates)"

# Rule 4 proper, back in awk: the pattern is an environment variable, and awk is the engine that
# pre-filtered with it, so letting a second tool decide would mean two regex dialects (awk processes
# escape sequences in a `-v` assignment; grep does not) and a pattern that selects under one and not
# the other. argv[0] is the last field because it can contain spaces; a tab it cannot.
matched=$(printf '%s' "$resolved" | awk -F'\t' -v pattern="$pattern" 'NF >= 5 && $5 ~ pattern')
matched_total=$(printf '%s\n' "$matched" | grep -c . || true)

# Rules 1, 2, 5 and 6, in that order. Rule 6 is applied here rather than in any awk because
# resolving a path through symlinks needs a filesystem.
foreign=0
reapable=
while IFS=$'\t' read -r pid ppid age start exe; do
    [ -n "${pid:-}" ] || continue
    [ "$ppid" -eq 1 ] || continue             # live parent: a running test owns this process
    [ "$age" -ge "$age_seconds" ] || continue # too young to be debris
    # Serving a default runtime socket: a human started this one (rule 5).
    case " $default_socket_pids " in *" $pid "*) continue ;; esac
    if inside_repo_checkout "$exe"; then
        # pid, then the identity the snapshot recorded for it, then argv[0]: argv[0] can contain
        # spaces, so it has to stay the last field.
        reapable="${reapable}${pid}"$'\t'"${start}"$'\t'"${exe}"$'\n'
    else
        foreign=$((foreign + 1))
    fi
done <<<"$matched"
orphans=${reapable%$'\n'}

if [ "$kill_allowed" -eq 0 ]; then
    ungated=$(printf '%s\n' "$orphans" | grep -c . || true)
    if [ "$ungated" -gt 0 ]; then
        echo "note: leaving $ungated otherwise reapable fixture process(es) alive: no socket" \
            "evidence to tell a detached daemon from debris" >&2
    fi
    orphans=
fi

killed=0
killed_pids=
replaced=0
while IFS=$'\t' read -r pid start exe; do
    [ -n "${pid:-}" ] || continue
    if [ "$dry_run" -eq 1 ]; then
        echo "would kill pid $pid $exe"
        killed=$((killed + 1))
        killed_pids="$killed_pids $pid"
        continue
    fi
    # Compared immediately before the signal against what the selecting snapshot recorded: see
    # `process_start`. A pid that simply exited since the snapshot is not a replacement, so it is
    # skipped silently rather than counted as one.
    current_start=$(process_start "$pid")
    [ -n "$current_start" ] || continue
    if [ "$current_start" != "$start" ]; then
        echo "note: pid $pid is no longer the process selected; not signalling it" >&2
        replaced=$((replaced + 1))
        continue
    fi
    echo "killing orphaned fixture server pid $pid $exe"
    kill -TERM "$pid" 2>/dev/null
    for _ in $(seq 1 20); do
        process_terminated "$pid" && break
        sleep 0.1
    done
    if ! process_terminated "$pid"; then
        # The same check again: SIGKILL is unanswerable, so the escalation needs its own proof that
        # the number still names the process that ignored SIGTERM.
        if ! still_the_selected_process "$pid" "$start"; then
            echo "note: pid $pid was replaced before the escalation; not sending SIGKILL" >&2
            replaced=$((replaced + 1))
            continue
        fi
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
if [ "$socket_evidence" -eq 0 ]; then
    echo "note: no unix socket inventory (lsof); leaving every socket directory in place" >&2
fi
referenced=$(printf '%s\n%s\n' "$referenced" "$held_sockets")

# Does this directory hold a server's sockets and nothing else?
#
# A name match is not ownership. `/tmp/srui-notes` or `/tmp/px0-cache` can be a developer's own
# directory that is old, unreferenced, and - before this check - recursively deleted by a sweep that
# runs automatically before tests. What the sweep is actually for is reclaiming the runtime
# directories of leaked fixture servers, and those contain unix sockets and nothing else. An empty
# directory qualifies too - a server that created its runtime directory and died before binding
# leaves exactly that. Any regular file, subdirectory or symlink means the directory is somebody
# else's and is left alone.
#
# Note for anyone extending this: the coding-agent demo's leftovers in /tmp are `*.sock.lock`
# *files*, not directories (76 of them on the machine this was written on), so the sweep has never
# considered them at all. Collecting those would need its own rule, with its own evidence.
#
# Known limitation, and the reason the automatic sweep reclaims nothing from the suites that
# actually leak. The SSH integration tests build their runtime directory as
# `/tmp/srui-persist-<hex>`, `/tmp/srui-live-<hex>`, `/tmp/srui-restarts-<hex>`, `/tmp/srui-hk-<hex>`
# and `/tmp/srui-fd-<hex>` (`client-macos/Tests/SRUITests/SSHTransport*.swift`), and each holds
# `sessiond.sock` *next to* `host_key`, `host_key.pub`, `user_key`, `user_key.pub`,
# `authorized_keys`, `known_hosts`, `sshd_config` and `sshd.pid`. Those regular files fail the test
# below, so every one of those directories is counted as "not a socket directory" on every sweep and
# stays on disk forever - with an ed25519 private key in it. This is deliberate and is not to be
# "fixed" by widening the test: an automatic `rm -rf` that deletes key material is the worse of the
# two failures. Collect them by hand (`rm -rf /tmp/srui-persist-* /tmp/srui-live-*` and the rest), or
# give those fixtures a teardown that survives a killed run. A rule that collected them would need
# ownership evidence of its own - a marker file written by the run that created the directory - and
# never a wider name match.
socket_only_directory() {
    local dir=$1
    # A symlink first, and before `find`: `find -P` does not descend a symlinked start point, so
    # `-mindepth 1` yields nothing for one and the test below would certify a developer's symlink
    # (to a directory full of their files) as socket-only - after which `rm -rf` unlinks it.
    # Measured: `/tmp/srui-link -> <dir containing NOTES.md>` was removed by the sweep.
    [ -L "$dir" ] && return 1
    [ -O "$dir" ] || return 1 # not ours to delete
    [ -z "$(find "$dir" -mindepth 1 -maxdepth 1 ! -type s -print -quit 2>/dev/null)" ]
}

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
unverified=0
for dir in "${candidates[@]+"${candidates[@]}"}"; do
    if [ "$socket_evidence" -eq 0 ]; then
        held=$((held + 1))
        continue
    fi
    # Evidence before `rm -rf`, never a name match alone.
    if ! socket_only_directory "$dir"; then
        unverified=$((unverified + 1))
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
printf 'reap-test-servers: %s %d fixture server(s), %s %d socket director(y|ies) (%d.%d MB); left %d fixture process(es) with a live parent or too young, %d outside this repository, %d replaced before signalling, %d referenced director(y|ies), %d not socket director(y|ies), %d recently touched.\n' \
    "$verb_killed" "$killed" "$verb_removed" "$removed" \
    "$((removed_kb / 1024))" "$(((removed_kb % 1024) * 10 / 1024))" \
    "$spared" "$foreign" "$replaced" "$held" "$unverified" "$fresh"

if [ "$failures" -gt 0 ]; then
    echo "reap-test-servers: $failures failure(s)" >&2
    exit 1
fi
exit 0
