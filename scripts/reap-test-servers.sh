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
#   6. the file it is running lives inside a checkout of *this* repository. That is the one thing
#      which makes a process this repository's test debris rather than some other program with a
#      familiar name, so it is the whole of the rule, in three parts:
#
#        * the root set is the physical path (`pwd -P`) of the main checkout and of every linked
#          worktree (`git worktree list`), plus the resolved destination of every `target` symlink
#          inside them;
#        * the candidate is argv[0] *fully resolved* — absolute, with every symlink component
#          followed, the final one included (`resolved_executable`);
#        * it is reapable only if that file lies under a root.
#
#      What that promises, exactly: a process is signalled only when the executable it is running is
#      a file inside a checkout of this repository, or inside a build cache one of those checkouts
#      points its `target` at. How the process was *launched* is not evidence of anything.
#      `/usr/local/bin/srui-sessiond`, a sibling project's `target/debug/srui-sessiond`, a path whose
#      directory no longer exists, and anything whose path cannot be determined at all are never
#      signalled, however leak-shaped they look.
#
#      Each part is there because the cheaper version of it killed something, and all three failures
#      were measured here:
#        * comparing the path as `ps` spells it, even with `.` and `..` folded out, admits any
#          foreign file reachable through a symlink inside a checkout — with `repo/cache ->
#          ../sibling`, `…/repo/cache/target/debug/srui-sessiond` carries the checkout's prefix
#          letter for letter while living in another project, whose daemon the sweep killed;
#        * comparing only physical paths, without the cache roots, makes every fixture in a worktree
#          whose `target` is a symlink into a shared build cache unreapable, since their files are
#          outside the checkout by design;
#        * resolving all of a path but its last component admits a fixture-named symlink inside a
#          checkout that points at an installed daemon or another project's build output — the name
#          is in the right place, the executable is not.
#
#      Costs, accepted deliberately. An orphan left by a run in a worktree that has since been
#      deleted is no longer reapable, because its file is now inside no checkout; those pids survive
#      every sweep and have to be killed by hand, and stale-worktree pruning makes that a one-way
#      ratchet. A file in a shared build cache is reapable however it was launched, including one
#      that got there because a *different* project pointed its own `target` at the same cache: a
#      cache a checkout of this repository points into is in scope, a symlink anywhere else in the
#      tree is not. Both are the right way round — the alternative is a sweep that can reach
#      binaries this repository never built.
#
#      Ownership evidence finer than rules 5 and 6 — a marker each test run writes for its own
#      fixtures — is still missing; a daemon detached by hand from inside a checkout, onto some
#      explicit socket other than the default, is still read as debris once it is old enough.
#   7. a fixture `sshd`, recognized by the configuration file it was started from rather than by its
#      executable. The SSH integration tests spawn the system `sshd` (`SSHTestSupport.launchSSHD`),
#      so its executable is `/usr/sbin/sshd` — outside every checkout, which rule 6 refuses and is
#      right to refuse. It is still this repository's debris, and when a run is killed it survives as
#      an orphaned listener holding the fixture directory its keys live in.
#
#      The evidence is `-f <path>`: a path inside one of the fixture directory families this sweep
#      already collects (`SRUI_REAP_TMP_GLOBS`). Nothing but a test writes an `sshd_config` there, so
#      a daemon configured from one is a fixture by construction. The system's own `sshd` reads
#      `/etc/ssh/sshd_config` and is never selected; neither is one whose `-f` path is relative
#      (nothing can say what it resolves to from a `ps` snapshot alone), one with no `-f` at all, or
#      one belonging to another user. Rules 1, 2 and 3 apply unchanged, so a listener a running suite
#      owns, or one younger than the age floor, is never touched.
#
#      This is deliberately not rule 4 with a wider pattern: matching `sshd` by name and then applying
#      rule 6 selects nothing (the binary is never in a checkout), and matching it by name *without*
#      rule 6 would put every `sshd` on this machine — including a host's real one, if a sweep ever
#      ran as root — one age check away from being killed. The configuration path is the narrowest
#      evidence that separates a fixture from a service.
#
#   Deliberately out of scope: `swift-test`. It is a toolchain binary
#   (`…/Xcode.app/…/usr/bin/swift-test`, `/usr/bin/swift-test`), so it lies outside every checkout and
#   no rule here can admit it without giving up what rule 6 promises. It does not need one: it waits
#   on the `swiftpm-testing-helper` that rules 4 and 6 *do* admit, so reaping the helper ends the
#   driver too. A `swift-test` that somehow outlives its helper has to be killed by hand.
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

# The cargo fixture servers, plus the SwiftPM processes a killed `swift test` leaves behind: the
# testing helper and the test bundle's own executable, both built inside a checkout's `.build` and so
# admitted by rule 6 exactly as a `target/debug` fixture is. `swift-test` itself is not here; see the
# out-of-scope note above.
pattern=${SRUI_REAP_PATTERN:-'/target/debug/(counter|srui-sessiond|coding-agent-demo|srtop)$|/\.build/[^ ]*/(swiftpm-testing-helper|[^/ ]+\.xctest/Contents/MacOS/[^/ ]+)$'}
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
# Rule 7 matches `sshd` by name only to find candidates; what admits one is the configuration path
# (`inside_fixture_directory`), never this pattern on its own.
sshd_pattern='(^|/)sshd$'
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

# Why any of the identity machinery below exists: selection and signalling are not simultaneous. Each
# candidate can occupy the kill loop for up to 2.2s (SIGTERM, twenty 0.1s liveness polls, SIGKILL,
# 0.2s), so with several candidates a *later* one has seconds in which to exit on its own and have its
# number handed to an unrelated process of this same user - which an unconditional kill would signal,
# and the escalation would SIGKILL. Every signal is therefore gated on what selected the candidate.
#
# `lstart` is the only start time `ps` will say on both platforms, and it is printed in whole seconds
# (procps-ng included). A single coarse value is not an identity: a pid reused by another process of
# this user *within the same second* presents the recorded one. So the comparison is not one value,
# see `still_the_selected_process`.

# A pid's start time in clock ticks since boot, where the platform will say it: field 22 of
# `/proc/<pid>/stat`, which is immutable and has 100 ticks to `lstart`'s one second on a default
# Linux. Empty where there is no /proc (macOS), where the pid is gone, or where the line is not the
# shape this expects - and no tick evidence is one guard fewer, never licence to signal. The callers
# spell "none" as `-`, because these values travel through tab-separated records that `read` would
# collapse around an empty field.
#
# Parsed from the last `)` and never by counting whitespace from the left: field 2 is `comm`, in
# parentheses, and a process is free to put spaces and parentheses in its own name (`(sd-pam)`,
# `Google Chrome`), which shifts every field after it. Nothing from `state` onwards contains a paren,
# so the last `) ` in the line always ends `comm`.
start_ticks_from_stat_line() {
    local line=$1 rest
    case $line in *') '*) ;; *) return 0 ;; esac
    rest=${line##*') '}
    # After `comm` the fields are state(3), ppid(4), ...; starttime(22) is the 20th of them.
    printf '%s' "$rest" | awk '{ if (NF >= 20 && $20 ~ /^[0-9]+$/) printf "%s", $20 }'
}

process_start_ticks() {
    local line
    [ -r "/proc/$1/stat" ] || return 0
    IFS= read -r line <"/proc/$1/stat" 2>/dev/null || return 0
    start_ticks_from_stat_line "$line"
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

# Where a checkout's build directories actually point: the shared build cache layout, in which the
# physical path of a perfectly ordinary fixture binary lies outside the checkout. Naming the cache as
# a root is what keeps those fixtures reapable now that only resolved paths are compared.
#
# Both build directories count, because both hold processes this sweep reaps: cargo's `target` and
# SwiftPM's `.build` (whose `swiftpm-testing-helper` and test bundle rule 4 now selects). A checkout
# that symlinks either one into a shared cache would otherwise leave those orphans unreapable, which
# is the same defect the `target` case was fixed for.
#
# A real build directory is pruned rather than descended into - it holds the whole build output, and
# walking it on every sweep (this script runs before each test run) would cost more than the sweep
# does. `-maxdepth 3` covers the layouts this repository has: `target`, `<crate>/target`,
# `<group>/<crate>/target` and `client-macos/.build`.
target_cache_roots() {
    local root=$1 link dir
    find "$root" -maxdepth 3 \( -name .git -o -name target -o -name .build \) \
        \( -type l -print -o -prune \) 2>/dev/null | while IFS= read -r link; do
        [ -L "$link" ] || continue
        dir=$(cd "$link" 2>/dev/null && pwd -P) || continue
        printf '%s\n' "${dir%/}"
    done
}

# The file argv[0] actually names: every symlink component followed, the final one included, so that
# what rule 6 compares against the roots is the executable itself and not a name that happens to sit
# in the right place. Empty when the path is relative, when a directory along it is gone, or when the
# links loop.
#
# Resolved by hand rather than with `readlink -f`/`realpath`, neither of which is portable to both
# platforms this script runs on. The directory is re-resolved at each step because a link target may
# itself be relative and lead through further symlinks; sixteen steps is ELOOP's usual ceiling.
resolved_executable() {
    local path=$1 dir base target depth=0
    case $path in
        /*) ;;
        *) return 1 ;;
    esac
    while [ "$depth" -lt 16 ]; do
        dir=$(cd "$(dirname "$path")" 2>/dev/null && pwd -P) || return 1
        base=$(basename "$path")
        path="${dir%/}/$base"
        [ -L "$path" ] || break
        target=$(readlink "$path" 2>/dev/null) || return 1
        [ -n "$target" ] || return 1
        case $target in
            /*) path=$target ;;
            *) path="${dir%/}/$target" ;;
        esac
        depth=$((depth + 1))
    done
    [ "$depth" -lt 16 ] || return 1
    printf '%s' "$path"
}

# Rule 6: is the file this process is running inside a checkout of this repository?
inside_repo_checkout() {
    local exe=$1 resolved root
    resolved=$(resolved_executable "$exe") || return 1
    [ -n "$resolved" ] || return 1
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
# The `lstart` is read out of the very snapshot that selects the pid, so no window exists in which the
# number could be reissued before that value is recorded. The start tick `rule4_candidates` adds
# beside it *is* a second read, and is safe for a different reason: it is one more condition a signal
# has to satisfy, so a tick belonging to a replacement can only ever hold a signal back.
# The pattern is an argument rather than the global, so rule 7 can reuse this selection with its own
# (`sshd_pattern`) and both families are parsed by the same code.
select_pattern_candidates() {
    awk -v pattern="${1:-$pattern}" -v my_uid="$my_uid" -v self="$$" "$AWK_COMMAND_LINE"'
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

# pid<TAB>ppid<TAB>age in seconds<TAB>lstart<TAB>start ticks<TAB>argv[0], for every process in a
# snapshot that is ours and whose argv[0] matches the pattern: rules 3 and 4, and the whole identity
# of each candidate. argv[0] stays last because it can contain spaces; nothing here can contain a tab.
#
# Called twice with different snapshots - the process table, and a single `ps` row read immediately
# before a signal - so the pre-signal check cannot drift from the rules that selected the candidate:
# it *is* those rules, run again on a fresh observation.
rule4_candidates() {
    local snap=$1 resolved= pid ppid age start ticks cmd
    while IFS=$'\t' read -r pid ppid age start cmd; do
        [ -n "${pid:-}" ] || continue
        ticks=$(process_start_ticks "$pid")
        resolved="${resolved}${pid}"$'\t'"${ppid}"$'\t'"${age}"$'\t'"${start}"$'\t'"${ticks:--}"$'\t'"$(command_argv0 "$cmd")"$'\n'
    done <<<"$(printf '%s\n' "$snap" | select_pattern_candidates)"
    # Rule 4 proper, back in awk: the pattern is an environment variable, and awk is the engine that
    # pre-filtered with it, so letting a second tool decide would mean two regex dialects (awk
    # processes escape sequences in a `-v` assignment; grep does not) and a pattern that selects
    # under one and not the other.
    printf '%s' "$resolved" | awk -F'\t' -v pattern="$pattern" 'NF >= 6 && $6 ~ pattern'
}

# The `-f <path>` an `sshd` was started from, or empty when it names none. `-f path` and `-f/path`
# are both accepted, because both are how it is written.
#
# A relative path yields nothing: what it resolves to depends on a working directory no `ps` snapshot
# records, so there is no evidence here to act on. A path containing a space is out of scope for the
# same reason argv[0] is (see rule 4) - `ps` joins argv with spaces and nothing can split it back.
sshd_config_path() {
    local cmd=$1 token next=0
    local -a tokens=()
    read -r -a tokens <<<"$cmd" # `read -a`, never `for token in $cmd`: no glob expansion
    for token in "${tokens[@]+"${tokens[@]}"}"; do
        if [ "$next" -eq 1 ]; then
            case $token in /*) printf '%s' "$token" ;; esac
            return 0
        fi
        case $token in
            -f) next=1 ;;
            -f/*)
                printf '%s' "${token#-f}"
                return 0
                ;;
        esac
    done
}

# Rule 7's evidence: is `path` inside one of the fixture directory families this sweep collects?
#
# The glob list is iterated with pathname expansion disabled. Unquoted, each pattern would be
# expanded against the real filesystem first and the loop would compare `path` against whatever
# happens to exist in /tmp today - so the test would pass or fail depending on the machine's litter
# rather than on the pattern. (The self-test hit exactly that: see its case 21.)
inside_fixture_directory() {
    local path=$1 glob
    [ -n "$path" ] || return 1
    case $path in /*) ;; *) return 1 ;; esac
    set -f
    # shellcheck disable=SC2086 # deliberate word splitting, with globbing off: a list of patterns
    set -- $tmp_globs
    set +f
    for glob in "$@"; do
        # shellcheck disable=SC2254 # $glob is a pattern here, by design
        case $path in $glob/*) return 0 ;; esac
    done
    return 1
}

# pid<TAB>ppid<TAB>age<TAB>lstart<TAB>start ticks<TAB>config path<TAB>argv[0] for every `sshd` of ours
# started from a configuration file inside a fixture directory: rule 7, and the whole identity of each
# candidate, in the same shape rule 4's candidates carry.
#
# Selected through the same snapshot and the same column parsing as every other candidate, so the two
# families cannot disagree about who a pid is. Only the admission evidence differs: rule 6 asks where
# the executable lives, rule 7 asks which configuration file it was handed.
fixture_sshd_candidates() {
    local snap=$1 pid ppid age start ticks cmd exe config
    while IFS=$'\t' read -r pid ppid age start cmd; do
        [ -n "${pid:-}" ] || continue
        exe=$(command_argv0 "$cmd")
        case $(basename "$exe") in sshd) ;; *) continue ;; esac
        config=$(sshd_config_path "$cmd")
        inside_fixture_directory "$config" || continue
        ticks=$(process_start_ticks "$pid")
        printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
            "$pid" "$ppid" "$age" "$start" "${ticks:--}" "$config" "$exe"
    done <<<"$(printf '%s\n' "$snap" | select_pattern_candidates "$sshd_pattern")"
}

# Re-derives, for the family that admitted this pid, every rule that selected it. A candidate is only
# ever re-checked against its own evidence: rule 6 would refuse every fixture `sshd` (its executable is
# outside the checkout, which is why rule 7 exists), and rule 7 would admit no cargo fixture (it is
# configured from no file at all).
still_selected_by_family() {
    local family=$1 pid=$2 start=$3 ticks=$4 evidence=$5 exe=$6
    if [ "$family" = sshd ]; then
        still_the_selected_sshd "$pid" "$start" "$ticks" "$evidence" "$exe"
    else
        still_the_selected_process "$pid" "$start" "$ticks" "$exe"
    fi
}

# True when `pid` still satisfies everything that selected it under rule 7, re-derived from a fresh
# observation exactly as `still_the_selected_process` does for rules 4 and 6: ours, orphaned, still
# past the age floor, the same `lstart` and start tick, the same argv[0], and still configured from the
# same fixture path. The evidence is re-read rather than remembered, so an `sshd` that was restarted
# on the same pid with a different configuration is not signalled.
still_the_selected_sshd() {
    local pid=$1 lstart=$2 ticks=$3 config=$4 exe=$5 row fresh
    local f_pid f_ppid f_age f_start f_ticks f_config f_exe
    row=$(ps -o pid=,ppid=,uid=,etime=,lstart=,command= -p "$pid" 2>/dev/null)
    [ -n "$row" ] || return 1
    fresh=$(fixture_sshd_candidates "$row")
    [ -n "$fresh" ] || return 1
    IFS=$'\t' read -r f_pid f_ppid f_age f_start f_ticks f_config f_exe <<<"$fresh"
    [ "$f_pid" = "$pid" ] || return 1
    [ "${f_ppid:-0}" -eq 1 ] || return 1
    [ "${f_age:-0}" -ge "$age_seconds" ] || return 1
    [ "$f_start" = "$lstart" ] || return 1
    [ "$f_config" = "$config" ] || return 1
    [ "$f_exe" = "$exe" ] || return 1
    [ "$ticks" = "-" ] || [ "$f_ticks" = "$ticks" ] || return 1
    return 0
}

# True when `pid` still satisfies everything that selected it, re-derived from a fresh observation
# rather than remembered: it is ours and not this shell (rule 3), orphaned (rule 1), still past the
# age floor (rule 2), its argv[0] still matches the pattern and is still the same path (rule 4), that
# path still resolves inside a checkout of this repository (rule 6), its `lstart` is still the one the
# selecting snapshot recorded, and - where /proc could say so - its start tick is unchanged.
#
# Independent evidence, because `lstart` alone cannot separate a pid reused inside one second from the
# process selected. A reused pid is a brand-new process, so the age floor excludes it outright
# whenever `SRUI_REAP_AGE_MINUTES` is non-zero, which is what every real sweep runs with.
#
# What remains at `SRUI_REAP_AGE_MINUTES=0`, stated rather than implied: the window narrows to "a
# process of this user, started in the same second as the one selected (the same 10ms tick where /proc
# is readable), whose argv[0] matches the fixture pattern, is the identical path, and lies inside a
# checkout of this repository". It does not close. Rule 5 is not re-derived either - that would mean a
# fresh `lsof` per signal - so a replacement that bound a default runtime socket within the window is
# judged by the other rules alone.
still_the_selected_process() {
    local pid=$1 lstart=$2 ticks=$3 exe=$4 row fresh f_pid f_ppid f_age f_start f_ticks f_exe
    # The same columns, in the same order, as `process_snapshot`: one observation, parsed by the same
    # code.
    row=$(ps -o pid=,ppid=,uid=,etime=,lstart=,command= -p "$pid" 2>/dev/null)
    [ -n "$row" ] || return 1
    fresh=$(rule4_candidates "$row")
    [ -n "$fresh" ] || return 1
    IFS=$'\t' read -r f_pid f_ppid f_age f_start f_ticks f_exe <<<"$fresh"
    [ "$f_pid" = "$pid" ] || return 1
    [ "${f_ppid:-0}" -eq 1 ] || return 1
    [ "${f_age:-0}" -ge "$age_seconds" ] || return 1
    [ "$f_start" = "$lstart" ] || return 1
    [ "$f_exe" = "$exe" ] || return 1
    [ "$ticks" = "-" ] || [ "$f_ticks" = "$ticks" ] || return 1
    inside_repo_checkout "$f_exe" || return 1
    return 0
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
# An array, never a space-separated string, and every expansion quoted. `TMPDIR` and
# `XDG_RUNTIME_DIR` are paths a user controls, and a space in either one split the list into
# nonexistent fragments: `default_socket_present` then found no socket in a directory it had just
# mis-spelled, `kill_allowed` stayed 1 on a host with no `lsof`, and a detached `srui-sessiond`
# serving its default socket was killed as debris. The array is never empty - the last two elements
# are unconditional - which the awk matcher below relies on.
default_tmp=${TMPDIR:-/tmp}
default_runtime_dirs=()
[ -n "${XDG_RUNTIME_DIR:-}" ] && default_runtime_dirs+=("${XDG_RUNTIME_DIR%/}")
default_runtime_dirs+=("${default_tmp%/}/srui-$my_uid" "/tmp/srui-$my_uid")

# Is this exactly one of those directories? Compared in the shell rather than handed to awk, which
# processes escape sequences in a `-v` assignment and would read a directory named with a backslash
# as something else.
is_default_runtime_dir() {
    local candidate=${1%/} dir
    for dir in "${default_runtime_dirs[@]}"; do
        [ "${dir%/}" = "$candidate" ] && return 0
    done
    return 1
}

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
    for dir in "${default_runtime_dirs[@]}"; do
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
# The directory list arrives as the first input file, one path per line, rather than through `-v`:
# `split(dirs, list, " ")` could not carry a path containing a space, and an assignment would have
# its escape sequences processed on the way in. `NR == FNR` is safe because that file always has at
# least the two unconditional entries of `default_runtime_dirs`.
default_socket_pids=$(awk -F'\t' -v leaf="$default_runtime_leaf" \
    -v sock_name="$default_socket_name" '
        NR == FNR { if ($0 != "") list[++n] = $0; next }
        {
            # A directory this sweep can name: protects every default socket under it, sessiond or
            # not (srtop and the demos have their own default names).
            for (i = 1; i <= n; i++) {
                if (index($2, list[i] "/") == 1) { print $1; next }
            }
            # Shape, for a daemon whose runtime directory this sweep cannot name: the default
            # runtime directory under a `TMPDIR` other than ours...
            if (index($2, "/" leaf "/") > 0) { print $1; next }
            # ...and the default socket file name under an `XDG_RUNTIME_DIR` other than ours.
            name = $2
            sub(/^.*\//, "", name)
            if (name == sock_name) { print $1; next }
        }
    ' <(printf '%s\n' "${default_runtime_dirs[@]}") <(printf '%s\n' "$socket_snapshot") |
    sort -u | tr '\n' ' ')

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

# Rules 3 and 4 over the snapshot, with each candidate's identity (see `rule4_candidates`).
matched=$(rule4_candidates "$snapshot")
matched_total=$(printf '%s\n' "$matched" | grep -c . || true)

# Rules 1, 2, 5 and 6, in that order. Rule 6 is applied here rather than in any awk because
# resolving a path through symlinks needs a filesystem.
foreign=0
reapable=
while IFS=$'\t' read -r pid ppid age start ticks exe; do
    [ -n "${pid:-}" ] || continue
    [ "$ppid" -eq 1 ] || continue             # live parent: a running test owns this process
    [ "$age" -ge "$age_seconds" ] || continue # too young to be debris
    # Serving a default runtime socket: a human started this one (rule 5).
    case " $default_socket_pids " in *" $pid "*) continue ;; esac
    if inside_repo_checkout "$exe"; then
        # pid, the identity the selecting snapshot recorded for it, which family admitted it and on
        # what evidence, then argv[0]: argv[0] can contain spaces, so it has to stay the last field.
        #
        # The evidence field is `-` rather than empty for this family, which carries none. A tab is
        # an IFS *whitespace* character, so `read` collapses a run of them: an empty field here
        # silently shifted argv[0] into `evidence` and left `exe` empty, after which every
        # pre-signal re-check failed and the sweep killed nothing while reporting each candidate as
        # "no longer the process selected". Placeholders are how the start tick already handles this.
        reapable="${reapable}${pid}"$'\t'"${start}"$'\t'"${ticks}"$'\t'checkout$'\t'-$'\t'"${exe}"$'\n'
    else
        foreign=$((foreign + 1))
    fi
done <<<"$matched"

# Rule 7, over the same snapshot: a fixture `sshd`, admitted by the configuration file it was started
# from rather than by where its executable lives. Rules 1, 2 and 3 are applied here too, and rule 5
# needs no mention - an `sshd` binds no unix socket at all, let alone a default runtime one.
sshd_matched=0
while IFS=$'\t' read -r pid ppid age start ticks config exe; do
    [ -n "${pid:-}" ] || continue
    sshd_matched=$((sshd_matched + 1))
    [ "$ppid" -eq 1 ] || continue
    [ "$age" -ge "$age_seconds" ] || continue
    reapable="${reapable}${pid}"$'\t'"${start}"$'\t'"${ticks}"$'\t'sshd$'\t'"${config}"$'\t'"${exe}"$'\n'
done <<<"$(fixture_sshd_candidates "$snapshot")"
matched_total=$((matched_total + sshd_matched))

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
while IFS=$'\t' read -r pid start ticks family evidence exe; do
    [ -n "${pid:-}" ] || continue
    if [ "$dry_run" -eq 1 ]; then
        echo "would kill pid $pid $exe"
        killed=$((killed + 1))
        killed_pids="$killed_pids $pid"
        continue
    fi
    # A pid that simply exited since the snapshot is not a replacement, so it is skipped silently
    # rather than counted as one.
    process_terminated "$pid" && continue
    # Every rule, re-derived immediately before the signal, by the family that admitted this pid.
    if ! still_selected_by_family "$family" "$pid" "$start" "$ticks" "$evidence" "$exe"; then
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
        # the number still names the process that ignored SIGTERM - by the same family, for the same
        # reason the selection used it.
        if ! still_selected_by_family "$family" "$pid" "$start" "$ticks" "$evidence" "$exe"; then
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

# `set --` splits the list on whitespace *and* expands each glob, so `"$@"` is already the matched
# paths. Expanding them a second time (`for path in $glob`, unquoted) split every match that contains
# a space back into fragments: a directory named `/tmp/srui-my notes` was then never collected at all,
# and - worse - a fragment that happened to name some *other* directory of ours put it on the
# candidate list without any glob matching it. A glob that matches nothing stays here as the pattern
# itself, which `-d` discards. A glob cannot contain a space, by the documented design of
# `SRUI_REAP_TMP_GLOBS`; the paths it matches can.
# shellcheck disable=SC2086 # deliberate word splitting: tmp_globs is a space-separated list of globs
set -- $tmp_globs
candidates=()
for path in "$@"; do
    [ -d "$path" ] || continue
    candidates+=("$path")
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
    if is_default_runtime_dir "$dir"; then
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
