#!/usr/bin/env python3
"""Select local checks from pushed revisions; full CI remains the merge gate.

The checks are single-flighted: cargo and SwiftPM serialize on one lock inside the
shared build directory, so two concurrent pushes used to wedge indefinitely instead
of queueing. One advisory `flock` in the repository's *common* git directory lets the
second push wait with a bounded, reported wait instead.
"""
from __future__ import annotations

import argparse
from collections.abc import Mapping
import contextlib
from dataclasses import asdict, dataclass
import errno
import fcntl
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import shlex
import signal
import subprocess
import sys
import time

SCRIPT = "scripts/pre_push_checks.py"
PLAN = "apps/srtop/srui-process-explorer-plan"
HOOK_FILES = {
    ".githooks/pre-push", ".githooks/pre-push-full", SCRIPT,
    "protocol/tests/test_pre_push_checks.py",
}
NATIVE_TEST = "client-macos/Tests/SRUITests/ProcessExplorerShellTests.swift"

LOCK_FILE = "srui-pre-push.lock"
# One full-profile run (uv sync + cargo + swift) takes several minutes, so the wait has
# to outlast a complete run in another worktree; 15 minutes covers that with slack and
# still fails loudly instead of hanging. SRUI_PRE_PUSH_LOCK_TIMEOUT overrides it (tests,
# and anyone who knows their own build is slower).
LOCK_TIMEOUT_SECONDS = 900.0
LOCK_POLL_SECONDS = 0.5
# Distinct from 1 (a failed check): the pushed revision was never examined.
LOCK_BUSY_STATUS = 75


class CheckError(RuntimeError):
    pass


class LockBusy(RuntimeError):
    """Another push held the single-flight lock past the bounded wait."""


# How long a signalled check is given to stop before, and then after, SIGKILL.
CHECK_STOP_GRACE_SECONDS = 5.0
# Records a stop that could not prove the build directory was quiet again.
UNVERIFIED_FILE = "srui-pre-push-unverified"
# Exit status for a push refused because of such a record: not a failed check (1),
# and not a lock held by a live run (75).
UNVERIFIED_STATUS = 76


@dataclass
class Check:
    name: str
    argv: list[str]
    # Whether this check writes the shared cargo/SwiftPM/uv state that two
    # concurrent runs deadlock on. Only a plan containing one takes the build lock:
    # a documentation-only push plans `git diff --check` alone, and queueing that
    # behind another worktree's full build - then refusing it with status 75 - stalls
    # a push that could not have collided with anything.
    builds: bool = False


@dataclass
class Plan:
    targets: list[str]
    ranges: list[tuple[str, str]]
    paths: list[str]
    profiles: dict[str, list[str]]
    checks: list[Check]
    notes: list[str]


# Git exports these repository-local variables to hooks. They must not leak
# into checks that create their own repositories (or into an explicit cwd).
LOCAL_GIT_ENV = {
    "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_CONFIG", "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT", "GIT_OBJECT_DIRECTORY", "GIT_DIR", "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE", "GIT_GRAFT_FILE", "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS", "GIT_REPLACE_REF_BASE", "GIT_PREFIX",
    "GIT_SHALLOW_FILE", "GIT_COMMON_DIR",
}


def check_environment() -> dict[str, str]:
    return {key: value for key, value in os.environ.items()
            if key not in LOCAL_GIT_ENV
            and not key.startswith(("GIT_CONFIG_KEY_", "GIT_CONFIG_VALUE_"))}


def git(repo: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", *args], cwd=repo, stdin=subprocess.DEVNULL, env=check_environment(),
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    if result.returncode:
        raise CheckError(os.fsdecode(result.stderr).strip())
    return os.fsdecode(result.stdout).removesuffix("\n")


def commit(repo: Path, ref: str) -> str:
    return git(repo, "rev-parse", "--verify", "--end-of-options", ref + "^{commit}")


def pushed_updates(text: str) -> list[tuple[str, str, str, str]]:
    updates = []
    for line in text.splitlines():
        fields = line.split()
        if len(fields) != 4 or any(
            re.fullmatch(r"[0-9a-fA-F]{40}|[0-9a-fA-F]{64}", fields[i]) is None
            for i in (1, 3)
        ):
            raise CheckError("Malformed pre-push ref update; cannot identify the pushed revision.")
        updates.append(tuple(fields))
    return updates


def new_branch_base(repo: Path, remote: str, head: str) -> str:
    # Use cached remote refs, never a potentially unrelated local main branch.
    for ref in (f"refs/remotes/{remote}/HEAD", f"refs/remotes/{remote}/main",
                f"refs/remotes/{remote}/master"):
        try:
            return git(repo, "merge-base", commit(repo, ref), head)
        except CheckError:
            continue
    raise CheckError("No cached remote default-branch merge base.")


def classify(paths: list[str]) -> dict[str, list[str]]:
    profiles: dict[str, list[str]] = {}
    for path in paths:
        suffix = Path(path).suffix.lower()
        if path in HOOK_FILES:
            profile = "hook"
        elif path.startswith(PLAN + "/") and suffix in {".md", ".json", ".png", ".py"}:
            profile = "plan"
        elif suffix in {".md", ".rst"} or (suffix == ".txt" and path.startswith("docs/")):
            profile = "docs"
        elif path.startswith("apps/srtop/") or path == NATIVE_TEST:
            profile = "srtop"
        else:
            # Shared code/configuration, dependencies, other apps and unknown
            # executable paths retain the original full checks.
            profile = "full"
        profiles.setdefault(profile, []).append(path)
    return profiles


def commands(profiles: dict[str, list[str]], paths: list[str],
             ranges: list[tuple[str, str]], targets: list[str],
             system: str) -> list[Check]:
    checks = [
        Check("whitespace", ["git", "diff", "--check", "--no-ext-diff", base, head])
        for base, head in ranges
    ]
    if not ranges:
        checks.extend(Check("whitespace", ["git", "show", "--format=", "--check", head])
                      for head in targets)
    skills = [path for path in paths if Path(path).name == "SKILL.md"]
    if skills:
        checks.append(Check("skill frontmatter", [
            "uv", "run", "--frozen", "python", SCRIPT, "--check-skills", "--", *skills,
        ], builds=True))
    if "plan" in profiles:
        checks.extend([
            Check("Process Explorer plan", ["python3", f"{PLAN}/validate_plan.py"]),
            Check("Process Explorer ledgers", ["python3", SCRIPT, "--check-ledgers"]),
        ])
        if any(Path(path).suffix.lower() == ".py" for path in profiles["plan"]):
            checks.append(Check("Process Explorer plan tooling regression tests", [
                "python3", "-m", "unittest", "discover", "-s", PLAN, "-p", "test_*.py",
            ]))
    if "hook" in profiles:
        checks.extend([
            Check("hook syntax", ["sh", "-n", ".githooks/pre-push"]),
            Check("full-hook syntax", ["sh", "-n", ".githooks/pre-push-full"]),
            Check("selector regression tests", [
                "python3", "-m", "unittest", "discover", "-s", "protocol/tests",
                "-p", "test_pre_push_checks.py",
            ]),
        ])
    if "full" in profiles:
        checks.extend([
            Check("locked Python development environment",
                  ["uv", "sync", "--frozen", "--extra", "dev"], builds=True),
            Check("full repository checks", ["sh", ".githooks/pre-push-full"], builds=True),
        ])
    elif "srtop" in profiles:
        manifest = ["--locked", "--manifest-path", "apps/srtop/Cargo.toml"]
        checks.extend([
            Check("Process Explorer formatting", [
                "cargo", "fmt", "--manifest-path", "apps/srtop/Cargo.toml",
                "-p", "srui-process-explorer", "--check",
            ], builds=True),
            Check("Process Explorer Clippy",
                  ["cargo", "clippy", *manifest, "--all-targets", "--", "-D", "warnings"],
                  builds=True),
            Check("test process stdio", ["bash", "scripts/check-test-process-stdio.sh"]),
        ])
        if system == "Darwin":
            # This entrypoint already tests Rust and builds the app and bridge.
            checks.append(Check("Process Explorer Rust and native SSH tests",
                                ["bash", "apps/srtop/test.sh"], builds=True))
        else:
            if NATIVE_TEST in paths:
                # Parse exactly the selected test. The shared range-based script
                # could select unrelated Swift files; deleted tests need no parse.
                checks.append(Check("Process Explorer Swift syntax (no type checking)", [
                    "bash", "-c", 'if [ -f "$1" ]; then swiftc -frontend -parse "$1"; fi',
                    "parse-native-test", NATIVE_TEST,
                ]))
            checks.append(Check("Process Explorer Rust tests",
                                ["cargo", "test", *manifest], builds=True))
    return checks


def make_plan(repo: Path, updates: list[tuple[str, str, str, str]],
              remote: str, system: str) -> Plan:
    targets, ranges, paths, notes = set(), set(), set(), []
    fallback = []
    for _local_ref, local_oid, remote_ref, remote_oid in updates:
        if set(local_oid) == {"0"}:  # A ref deletion sends no code.
            continue
        head = commit(repo, local_oid)
        targets.add(head)
        try:
            base = (new_branch_base(repo, remote, head)
                    if set(remote_oid) == {"0"} else commit(repo, remote_oid))
            # Endpoint diff covers force-push deletions too. Disabling rename
            # detection retains both paths, so a code -> docs rename cannot hide code.
            changed = git(repo, "diff", "--name-only", "--no-renames", "-z", base, head)
            paths.update(path for path in changed.split("\0") if path)
            ranges.add((base, head))
        except CheckError:
            fallback.append(f"Unresolved comparison base for {remote_ref}")
    profiles = classify(sorted(paths))
    if fallback:
        profiles.setdefault("full", []).extend(fallback)
    if "srtop" in profiles and system != "Darwin":
        notes.append("Native SSH/AppKit checks require macOS; this push is not native acceptance evidence. The macOS CI gate still applies.")
    if "full" in profiles:
        notes.append("Conservative full profile: shared/configuration/unknown changes or an unresolved base.")
    return Plan(
        sorted(targets), sorted(ranges), sorted(paths), profiles,
        commands(profiles, sorted(paths), sorted(ranges), sorted(targets), system), notes,
    )


def require_candidate(repo: Path, targets: list[str]) -> None:
    if not targets:
        return
    if set(targets) != {commit(repo, "HEAD")}:
        raise CheckError(
            "The pushed revision differs from this checkout (or several different commits are being pushed). "
            "Push each revision from its own clean worktree; refusing to test the wrong code."
        )
    if git(repo, "status", "--porcelain", "-z", "--untracked-files=all"):
        raise CheckError(
            "The checkout has tracked or untracked changes. Commit/stash them or push from a clean "
            "worktree so checks validate the exact pushed revision."
        )


def run_plan(repo: Path, plan: Plan) -> int:
    require_candidate(repo, plan.targets)
    for check in plan.checks:
        print(f"\n== {check.name}: {shlex.join(check.argv)}", flush=True)
        status = run_check(check.argv, repo)
        if status:
            print(f"FAILED: {check.name} (exit {status}); push aborted.", file=sys.stderr)
            return 1
    # Detect source changes caused by generators/tests rather than certify a
    # different tree. Ignored build artifacts do not make the checkout dirty.
    require_candidate(repo, plan.targets)
    print("Selected pre-push checks passed." if plan.targets else "No code-bearing ref updates; no checks needed.")
    return 0


def lock_path(repo: Path) -> Path:
    # Inside a linked worktree `.git` is a *file*, so only --git-common-dir names a
    # directory, and it names the same one for every worktree of this repository.
    # They all share the cargo/SwiftPM build directories, so they must all contend
    # for one lock; a per-worktree lock would not prevent the deadlock at all.
    common = Path(git(repo, "rev-parse", "--git-common-dir"))
    if not common.is_absolute():
        common = repo / common
    return common.resolve() / LOCK_FILE


def lock_timeout() -> float:
    raw = os.environ.get("SRUI_PRE_PUSH_LOCK_TIMEOUT")
    if not raw:
        return LOCK_TIMEOUT_SECONDS
    try:
        return max(0.0, float(raw))
    except ValueError:
        return LOCK_TIMEOUT_SECONDS


def lock_holder(handle: int) -> str:
    # Advisory only: the pid is for the waiting message, never for a decision.
    try:
        text = os.fsdecode(os.pread(handle, 64, 0)).strip()
    except OSError:
        return "unknown"
    return text.splitlines()[0] if text else "unknown"


@contextlib.contextmanager
def single_flight(repo: Path, timeout: float | None = None,
                  poll: float = LOCK_POLL_SECONDS, stream=None):
    """Hold an exclusive advisory lock for the duration of the checks.

    flock is owned by the open file description, so the kernel releases it on every
    exit path including a crash or SIGKILL. A lock built from file existence would
    strand every future push instead.
    """
    stream = sys.stderr if stream is None else stream
    timeout = lock_timeout() if timeout is None else timeout
    path = lock_path(repo)
    handle = os.open(path, os.O_RDWR | os.O_CREAT | os.O_CLOEXEC, 0o644)
    try:
        deadline = time.monotonic() + timeout
        announced = False
        while True:
            try:
                fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except OSError as error:
                if error.errno not in (errno.EACCES, errno.EAGAIN, errno.EWOULDBLOCK):
                    raise
            if not announced:
                announced = True
                print(f"pre-push: waiting up to {timeout:.0f}s for the pre-push checks lock "
                      f"{path}, held by pid {lock_holder(handle)}. Concurrent cargo/SwiftPM "
                      "builds deadlock on the shared build directory, so this push queues "
                      "behind that one.", file=stream, flush=True)
            if time.monotonic() >= deadline:
                raise LockBusy(
                    f"pid {lock_holder(handle)} still holds {path} after {timeout:.0f}s; "
                    f"this revision was not checked (exit {LOCK_BUSY_STATUS}). Let that run "
                    "finish and push again, or push with --no-verify to skip the local gate "
                    "entirely -- that checks nothing at all before the push, leaving CI as "
                    "the only gate."
                )
            time.sleep(poll)
        restore = install_release_on_signal(repo)
        try:
            os.ftruncate(handle, 0)
            os.pwrite(handle, f"{os.getpid()}\n".encode(), 0)
            yield path
        finally:
            restore()
            with contextlib.suppress(OSError):
                os.ftruncate(handle, 0)
            fcntl.flock(handle, fcntl.LOCK_UN)
    finally:
        os.close(handle)


# The check this process is currently waiting on, as its own process group.
_active_check: subprocess.Popen | None = None


# Signals whose delivery must never land between spawning a check and recording it.
STOP_SIGNALS = tuple(
    number for number in (getattr(signal, name, None)
                          for name in ("SIGTERM", "SIGHUP", "SIGINT"))
    if number is not None
)


def unblock_stop_signals() -> None:
    """Clear the inherited signal mask in a freshly forked check.

    A signal *mask* survives fork and exec, unlike a handler. Blocking in the
    parent to close the registration window below therefore handed every check a
    mask in which SIGTERM, SIGHUP and SIGINT were blocked -- so the check ignored
    the SIGTERM sent to stop it, the drain ran to its deadline, and the lock was
    released only after SIGKILL. Measured: `sleep` survived SIGTERM for the full
    five seconds. The child clears the mask for itself, between fork and exec.
    """
    signal.pthread_sigmask(signal.SIG_UNBLOCK, STOP_SIGNALS)


def run_check(argv: list[str], cwd: Path) -> int:
    """Run one check in its own process group and wait for it.

    Its own group, so a signal delivered to this hook alone can still stop the
    build: `git` sends the hook a signal, not the group, and a cargo or SwiftPM
    child that outlives the hook keeps the shared build directory busy while the
    lock is already gone -- after which the next push acquires the lock and starts
    a second build into it, which is the overlap the lock exists to prevent.

    The handled signals are blocked across the spawn and the registration. A
    signal that arrived in between would find no active check, stop nothing, and
    release the lock over a build that had just started -- the same overlap,
    through a window a few instructions wide. Blocking does not lose it: it is
    delivered as soon as the mask is restored, by which time the check is
    recorded.
    """
    global _active_check
    previous = signal.pthread_sigmask(signal.SIG_BLOCK, STOP_SIGNALS)
    try:
        process = subprocess.Popen(argv, cwd=cwd, stdin=subprocess.DEVNULL,
                                   env=check_environment(), start_new_session=True,
                                   preexec_fn=unblock_stop_signals)
        _active_check = process
    finally:
        signal.pthread_sigmask(signal.SIG_SETMASK, previous)
    try:
        return process.wait()
    finally:
        _active_check = None


def process_listing(columns: str) -> str | None:
    """One `ps` snapshot, or `None` when the process table could not be read.

    `None` and "nothing is running" must never be the same answer. An empty stdout
    from a failed `ps` read as an empty process table would let the drain below
    conclude that an interrupted build had finished, and release the lock over a
    cargo or SwiftPM process still writing to the shared build directory. Every
    failure mode lands here -- nonzero exit, a timeout, `ps` missing entirely -- so
    none of them can propagate out of a signal handler either.
    """
    try:
        snapshot = subprocess.run(["ps", "-eo", columns], stdin=subprocess.DEVNULL,
                                  stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                  text=True, check=False, timeout=10)
    except (OSError, subprocess.SubprocessError):
        return None
    if snapshot.returncode != 0:
        return None
    return snapshot.stdout


def check_process_tree(root: int) -> tuple[set[int], set[int]] | None:
    """Every pid and process group the check `root` is responsible for.

    `None` when the process table could not be read; see [`process_listing`].

    One `ps` snapshot, walked twice. First by parent, to find the descendants;
    then by group, because a wrapper may put its own children in a *new* process
    group -- `scripts/run-swift-tests.sh` does exactly that (`set -m`), precisely
    so it can reap them by group -- and because a descendant reparented to init
    keeps its group when it loses its parent. Signalling `root`'s group alone
    therefore misses the `swift test` that the group boundary hides, which is how
    a SwiftPM build outlived the lock that was protecting it.

    This hook's own pid, group and session are never included: the point is to
    stop what the check started, not to signal the push.
    """
    listing = process_listing("pid=,ppid=,pgid=")
    if listing is None:
        return None
    children: dict[int, list[int]] = {}
    group_of: dict[int, int] = {}
    for line in listing.splitlines():
        fields = line.split()
        if len(fields) != 3 or not all(field.isdigit() for field in fields):
            continue
        pid, ppid, pgid = (int(field) for field in fields)
        children.setdefault(ppid, []).append(pid)
        group_of[pid] = pgid

    # The root is a seed for the walk, not a result: once `Popen.wait` has reaped
    # it the number is free for the kernel to reissue, and treating it as part of
    # the check would make the drain condition never succeed and could aim the
    # SIGKILL pass at whatever reused the pid.
    tree: set[int] = set()
    queue = [root]
    while queue:
        for child in children.get(queue.pop(), ()):
            if child not in tree:
                tree.add(child)
                queue.append(child)
    if root in group_of:
        tree.add(root)

    mine = {os.getpid(), os.getppid()}
    forbidden = {0, 1, os.getpgrp()} | mine
    groups = {group_of[pid] for pid in tree if pid in group_of} - forbidden
    members = {pid for pid, pgid in group_of.items() if pgid in groups}
    return (tree | members) - mine, groups


def live_groups(groups: set[int], session: int | Mapping[int, int] | None = None
                ) -> dict[int, set[int]] | None:
    """The members of each of `groups` that are still live, keyed by group.

    `session` is the session each group was *observed in when it was discovered* -
    one sid for every group, or a pgid -> sid mapping - and a member outside it does
    not count. A pgid in `groups` was true when it was discovered; by the time the
    escalation pass runs, that group can have drained and the kernel can have
    reissued the number while another of this check's groups was still consuming the
    grace window. The pgid then has live members again - someone else's - and one
    snapshot cannot tell the difference, because pgid and state are exactly what
    matched before. The session can, as long as it is the one *that group* had:
    a descendant that calls `setsid()` (`start_new_session=True` in
    `benchmarks/process_control.py`, every portable-pty child) leads a session of its
    own and is still this check's, so comparing every group against the check's own
    session would drop exactly those groups - never signalled, never awaited, and the
    stop reported verified over them. With a mapping, a group it does not name has no
    observed session to compare against and is not reported.

    Per group, not in aggregate: when a check owns several groups and one drains
    while another is still running, signalling *every* accumulated group again
    would aim the escalation at a group whose leader pid has since been reissued -
    the pid-reuse hazard, one level up. Only groups present here may be signalled.

    One snapshot, not two joined by pid. Sampling membership and then grouping it
    separately let a pid exit and be reissued between the halves: the reused
    process's unrelated pgid came back as a live group of this check and took the
    SIGTERM, and a member forked after the first sample was missing from the second,
    which reads as drained. Both are the same mistake as everywhere else here -
    treating two observations of a changing table as one fact.
    """
    if not groups:
        return {}
    listing = process_listing("pid=,pgid=,state=")
    if listing is None:
        return None
    mine = {os.getpid(), os.getppid()}
    by_group: dict[int, set[int]] = {}
    for line in listing.splitlines():
        fields = line.split()
        if len(fields) != 3 or not fields[0].isdigit() or not fields[1].isdigit():
            continue
        pid, pgid, state = int(fields[0]), int(fields[1]), fields[2]
        # A zombie holds no build directory; see [`live_group_members`].
        if state.startswith("Z") or pid in mine or pgid not in groups:
            continue
        if session is not None:
            expected = session.get(pgid) if isinstance(session, Mapping) else session
            if expected is None:
                continue  # Never observed as this check's; nothing to compare against.
            try:
                if os.getsid(pid) != expected:
                    continue  # This number is someone else's group now.
            except ProcessLookupError:
                continue  # Exited between the snapshot and the question.
            except OSError:
                return None  # "Cannot tell" is never "nothing there".
        by_group.setdefault(pgid, set()).add(pid)
    return by_group


def live_group_members(groups: set[int], session: int | Mapping[int, int] | None = None
                       ) -> set[int] | None:
    """Which processes still belong to `groups`, excluding this push's own.

    Membership, not parentage: a process keeps its group when its parent dies, so
    this still sees a build whose shell has already exited. `None` means the table
    could not be read, which is not the same as nothing being there.

    Derived from [`live_groups`] rather than sampling the table a second time, so
    these two questions can never be answered from two different process tables.

    A zombie is an exit status nobody has collected yet, not a process still writing
    to the build directory: it holds no resources and cannot be signalled. Counting
    one kept the drain from ever succeeding, so every interrupted push burned its
    full grace window and then escalated to SIGKILL against processes already gone.
    """
    by_group = live_groups(groups, session)
    if by_group is None:
        return None
    return {pid for members in by_group.values() for pid in members}


def live_session_members(session: int) -> set[int] | None:
    """Which live processes still belong to `session`, excluding this push's own.

    The session, not the group, is what covers a process group this hook never
    discovered. `run_check` spawns with `start_new_session`, so the check leads its
    own session and its sid *is* its pid; every descendant inherits that session,
    and a wrapper's `set -m` creates new process *groups* inside it rather than a
    new session. So the `swift test` that `scripts/run-swift-tests.sh` hides behind
    a group boundary is named here even when `ps` could not be read at the time of
    the stop and its group was never learned.

    Membership is asked of the kernel per pid because `ps` has no portable session
    column: Linux spells it `sid`, and on macOS `sess` prints a kernel pointer that
    is useless once the session leader has been reaped. `None` means the question
    could not be answered, which is never the same as nothing being there.
    """
    if not session:
        return set()
    listing = process_listing("pid=,state=")
    if listing is None:
        return None
    mine = {os.getpid(), os.getppid()}
    live = set()
    for line in listing.splitlines():
        fields = line.split()
        if len(fields) != 2 or not fields[0].isdigit():
            continue
        pid, state = int(fields[0]), fields[1]
        # A zombie holds no build directory; see [`live_group_members`].
        if state.startswith("Z") or pid in mine:
            continue
        try:
            member = os.getsid(pid)
        except ProcessLookupError:
            continue  # Exited between the snapshot and the question.
        except OSError:
            # Some systems refuse getsid across sessions. "Cannot tell" must not
            # read as "not a member", which would clear the record over a live build.
            return None
        if member == session:
            live.add(pid)
    return live

def observe_group_sessions(tree: tuple[set[int], set[int]], sessions: dict[int, int]) -> None:
    """Record, for each newly discovered group, the session one of its members is in now.

    Asked of a pid the discovery snapshot placed in that group, and only kept when the
    kernel still agrees on the group - otherwise the pid has been reissued since the
    snapshot and its session says nothing about the check. A group already observed
    keeps its first answer: that is the one taken closest to discovery.
    """
    pids, groups = tree
    for pid in sorted(pids):
        try:
            pgid = os.getpgid(pid)
            if pgid not in groups or pgid in sessions:
                continue
            sid = os.getsid(pid)
            if os.getpgid(pid) == pgid:
                sessions[pgid] = sid
        except OSError:
            continue  # Gone, or not ours to ask: no evidence either way.


def stop_active_check(grace: float = CHECK_STOP_GRACE_SECONDS) -> dict[int, int]:
    """Stop everything the running check started, and wait for all of it to go.

    Returns the process groups it could **not** prove are gone, each with the session
    it was observed in - empty when the drain was verified. The caller records a non-empty answer, because mutual
    exclusion has to survive this process's death: see [`record_unverified_stop`].

    Waiting is the point: the lock must outlive the build it was taken for, so
    this returns only once nothing is left writing to the build directory.

    Signalling is by *process group* only, and only groups discovered from the
    check's own tree. Never by bare pid: the direct child is reaped by `Popen` the
    moment it exits, after which its number is free for the kernel to reissue, and
    a later pass that signalled it could hit an unrelated process of this user. A
    group is only signalled while it still has live members, and the groups are
    re-derived while the root lives, so a wrapper's `set -m` group created after
    the first pass is still caught.
    """
    process = _active_check
    if process is None or process.poll() is not None:
        return {}
    # The group the check leads is known without any snapshot: `start_new_session`
    # made it the leader of its own group and session, so its pid *is* both. Every
    # other group is discovered, and so is the session it was in at the time: a
    # descendant may have left the check's session with `setsid()` and is still the
    # check's to stop. Whether a group is *still this check's* is answered against the
    # session it was observed in, never against one assumed for it.
    groups = {process.pid}
    sessions = {process.pid: process.pid}
    tree = check_process_tree(process.pid)
    if tree is not None:
        groups |= tree[1]
        observe_group_sessions(tree, sessions)
    for sig in (signal.SIGTERM, signal.SIGKILL):
        if process.poll() is None:
            tree = check_process_tree(process.pid)
            if tree is not None:
                groups |= tree[1]
                observe_group_sessions(tree, sessions)
        alive = live_groups(groups, sessions)
        if alive == {}:
            break
        # Only groups with a live member right now.
        if alive is not None:
            targets = sorted(alive)
        elif process.poll() is None:
            # No per-group answer, so the group the check leads is signalled on its
            # own. That is sound only while this holds: a child this process has not
            # reaped is still running, so the kernel cannot have reissued its pid.
            targets = [process.pid]
        else:
            # Reaped *and* unenumerable. The number is free for the kernel to hand
            # out, and the group wearing it now would be someone else's, so nothing is
            # signalled: the stop reports itself unverified and the next push refuses,
            # rather than this pass sending SIGKILL to an unrelated group of this user.
            targets = []
        for pgid in targets:
            with contextlib.suppress(OSError, ProcessLookupError):
                os.killpg(pgid, sig)
        deadline = time.monotonic() + grace
        drained = False
        while time.monotonic() < deadline:
            with contextlib.suppress(subprocess.TimeoutExpired):
                process.wait(timeout=0.05)
            # Only an *answered* enumeration can end the wait. `None` is "cannot
            # tell", and treating it as drained is precisely how the lock would be
            # released over a build still running.
            if live_groups(groups, sessions) == {}:
                drained = True
                break
            time.sleep(0.05)
        if drained:
            break
    with contextlib.suppress(subprocess.TimeoutExpired):
        process.wait(timeout=grace)
    remaining = live_groups(groups, sessions)
    if remaining == {}:
        return {}
    # Either a group is still alive, or the table could not be read at all. Both mean
    # the same thing to the caller: this push cannot promise the build directory is
    # quiet, and the next one must not start a build into it on trust. A group whose
    # session was never observed is recorded under the check's own, the only session
    # anything it started is known to have been in.
    unaccounted = groups if remaining is None else set(remaining)
    return {pgid: sessions.get(pgid, process.pid) for pgid in unaccounted}


def unverified_path(repo: Path) -> Path:
    """Where an unverifiable stop is recorded, beside the lock it could not vouch for."""
    return lock_path(repo).with_name(UNVERIFIED_FILE)


def write_unverified(repo: Path, groups: set[int] | Mapping[int, int],
                     session: int | None) -> bool:
    """Persist the record, or say loudly that it could not be persisted.

    Returns whether it is on disk. The failure must not be swallowed: a full or
    read-only git directory used to leave the hook printing that it had recorded a
    marker which does not exist, after which the next push found nothing, took the
    lock, and built into a directory the interrupted build was still writing to.
    """
    path = unverified_path(repo)
    payload = {"pid": os.getpid(), "groups": sorted(groups), "session": session}
    if isinstance(groups, Mapping):
        # The session each group was observed in, so the next push asks the same
        # question of it that this one did; see [`live_groups`].
        payload["group_sessions"] = {str(pgid): sid for pgid, sid in sorted(groups.items())}
    try:
        path.write_text(json.dumps(payload) + "\n")
        return True
    except OSError as error:
        print(f"pre-push: FAILED to record the interrupted run at {path}: {error}. The "
              f"next push will NOT be held back, so confirm nothing this one started is "
              f"still running (process group(s) {sorted(groups)}, session {session}) "
              "before pushing again.", file=sys.stderr)
        return False


def arm_unverified_stop(repo: Path) -> tuple[int | None, bool]:
    """Record, *before* anything is signalled, that this run may leave a build behind.

    Written first and removed only once the stop has proved otherwise, because the
    record has to outlive this process and this process can die in the middle of the
    stop: the SIGTERM being handled is routinely followed by a SIGKILL that no
    handler sees.

    Returns the session recorded - `None` when no check was running - and whether the
    record reached the disk, which the caller must not ignore: a cancellation nobody
    can be told about is one this hook declines to perform.

    The session is what makes the record complete rather than a record of only what
    enumeration happened to discover; see [`live_session_members`].
    """
    process = _active_check
    if process is None or process.poll() is not None:
        return None, True  # Nothing is running, so there is nothing to lose.
    session = process.pid  # `start_new_session` made the check its own session leader.
    return session, write_unverified(repo, {session}, session)


def record_unverified_stop(repo: Path, groups: set[int] | Mapping[int, int],
                           session: int | None = None) -> None:
    """Record that a stopped run could not prove its build had finished.

    A lock cannot outlive the process holding it - flock is released when the fd
    closes, which is exactly what makes it safe against a crash. So when a stop
    cannot prove the build directory is quiet, the *fact* has to outlive the
    process instead: the next push reads this and refuses to build into the same
    directory on trust. Concretely, this is the case where `ps` is unavailable and
    `scripts/run-swift-tests.sh` has put `swift test` in a group of its own
    (`set -m`, and it installs no signal trap), so the nested build cannot be
    discovered, let alone awaited.
    """
    if not write_unverified(repo, groups, session):
        return
    named = "" if session is None else f" and session {session}"
    print(f"pre-push: could not confirm the interrupted checks had stopped; recorded "
          f"{unverified_path(repo)}. The next push will refuse until process group(s) "
          f"{sorted(groups)}{named} are gone.", file=sys.stderr)


def clear_unverified_stop(repo: Path) -> None:
    """Drop the armed record: the stop accounted for everything the check started."""
    with contextlib.suppress(OSError):
        unverified_path(repo).unlink()


def stop_and_record(repo: Path, grace: float = CHECK_STOP_GRACE_SECONDS) -> bool:
    """Stop the running check and leave the next push a truthful record of the result.

    Arm, stop, then clear only on a proved stop. The order is the point: a record
    written after the drain is missing in precisely the case it exists for, because
    the drain is what the follow-up SIGKILL interrupts.

    Returns whether the caller may exit. `False` means the record could not be
    written, so the checks were **not** stopped: cancelling a build while being
    unable to warn the next push about it is the one outcome worth refusing, and
    continuing to hold the lock until the checks finish is what the lock is for.
    """
    session, armed = arm_unverified_stop(repo)
    if not armed:
        print("pre-push: the interrupted run could not be recorded, so the checks were "
              "NOT stopped - this hook keeps the lock until they finish rather than "
              "cancelling a build the next push cannot be warned about. Free space in "
              f"{unverified_path(repo).parent} and signal again, or kill this hook and "
              "check by hand that no cargo or swift build survived it.", file=sys.stderr)
        return False
    unaccounted = stop_active_check(grace)
    if unaccounted:
        record_unverified_stop(repo, unaccounted, session)
        return True
    if session is None:
        return True
    # Accounted-for groups are not an empty session: a group that was never discovered
    # cannot appear in `unaccounted`, which is the whole case the record exists for. So
    # only an *answered* and empty session may clear what the arm wrote.
    if live_session_members(session) == set():
        clear_unverified_stop(repo)
    else:
        record_unverified_stop(repo, {session}, session)
    return True


def unverified_reason(repo: Path) -> str | None:
    """Why this push must not run checks yet, or `None` when it may.

    Self-clearing in the ordinary case: the record names what a previous run left
    unaccounted for, so once none of it has a live member the leftover build really
    is gone and the record is removed. It is kept - and the push refused - while
    anything still runs, or while the process table cannot be read at all, because
    neither of those is proof of anything.

    Both the groups *and* the session are checked. The groups alone would clear the
    record in exactly the case it is written for: when enumeration failed, the only
    group known is the one the check led, so killing the wrapper empties it while the
    separately grouped `swift test` it started keeps building. That group was never
    discovered - its session was.
    """
    path = unverified_path(repo)
    try:
        recorded = json.loads(path.read_text())
    except FileNotFoundError:
        return None
    except (OSError, ValueError):
        return (f"{path} exists but could not be read. A previous push could not confirm "
                "its checks had stopped; make sure no cargo or swift build is running, "
                "then delete that file.")
    groups = {int(group) for group in recorded.get("groups", [])}
    session = int(recorded.get("session") or 0)
    observed = recorded.get("group_sessions")
    # Session-scoped here too, against the session each group was observed in: a
    # recorded pgid the kernel has since handed to a process in another session would
    # otherwise keep having live members, and the record would never clear. This does
    # not cover a reissued *session* id - the check's sid is its reaped root's pid, and
    # a new session leader given that number matches both - which is why the refusal
    # names the pids, so a stranger can be told apart by hand.
    scope: int | Mapping[int, int] | None = session or None
    if isinstance(observed, dict):
        scope = {int(pgid): int(sid) for pgid, sid in observed.items()}
    members = live_group_members(groups, scope)
    in_session = live_session_members(session)
    if members is None or in_session is None:
        return (f"a previous push could not confirm its checks had stopped ({path}), and "
                "the process table cannot be read to check now. Make sure no cargo or "
                "swift build is running, then delete that file.")
    leftover = members | in_session
    if leftover:
        return (f"a previous push left process(es) {sorted(leftover)} running from its "
                f"interrupted checks (group(s) {sorted(groups)}, session {session}). They "
                "are still writing to the build directory this push would build into. "
                f"Stop them, or wait, then push again ({path}).")
    with contextlib.suppress(OSError):
        path.unlink()
    return None


def install_release_on_signal(repo: Path):
    """Turn termination signals into SystemExit so the lock's cleanup still runs.

    The running check is stopped *before* the lock unwinds, so the lock is never
    released while a build it was protecting is still going.
    """
    def terminate(signum, _frame):
        # Nothing here may propagate: an exception raised inside the handler would
        # unwind `single_flight` and release the lock over a check still running,
        # which is the one outcome the handler exists to prevent.
        try:
            if not stop_and_record(repo):
                return  # Deliberately not SystemExit: see `stop_and_record`.
        except BaseException as error:  # noqa: BLE001 - deliberately total
            print(f"pre-push: stopping the checks failed: {error!r}", file=sys.stderr)
        raise SystemExit(128 + signum)

    previous = {}
    # SIGINT too: a check in its own process group no longer receives the ^C that
    # the terminal sends to the foreground group, so this handler has to pass it on.
    for name in ("SIGTERM", "SIGHUP", "SIGINT"):
        number = getattr(signal, name, None)
        if number is None:
            continue
        try:  # Only the main thread may install handlers.
            previous[number] = signal.signal(number, terminate)
        except ValueError:
            return lambda: None

    def restore() -> None:
        for number, handler in previous.items():
            with contextlib.suppress(ValueError):
                signal.signal(number, handler)

    return restore


def guarded_run(repo: Path, plan: Plan, runner=run_plan, **lock) -> int:
    """Run the selected checks, one hook invocation at a time per repository.

    Only a plan that *builds* takes the lock. What the lock exists for is two runs
    driving cargo, SwiftPM or uv at the same time, which deadlock on the shared
    build directory; a plan that will not build cannot deadlock with anything.
    Queueing one would only make a push that touches no build directory wait out
    another one's full build and then fail with status 75 having checked nothing:
    `git push --delete`, whose plan is empty by construction (`make_plan` skips ref
    deletions), and a documentation-only push, whose whole plan is
    `git diff --check`.
    """
    if not any(check.builds for check in plan.checks):
        return runner(repo, plan)
    try:
        with single_flight(repo, **lock):
            # Inside the lock, so two pushes cannot race on clearing the record.
            refusal = unverified_reason(repo)
            if refusal is not None:
                print(f"pre-push: {refusal}", file=sys.stderr)
                return UNVERIFIED_STATUS
            return runner(repo, plan)
    except LockBusy as error:
        print(f"pre-push: {error}", file=sys.stderr)
        return LOCK_BUSY_STATUS


def print_plan(plan: Plan) -> None:
    for base, head in plan.ranges:
        print(f"Range: {base[:12]}..{head[:12]}")
    for profile, reasons in sorted(plan.profiles.items()):
        shown = ", ".join(repr(reason) for reason in reasons[:5])
        extra = f" (+{len(reasons) - 5} more)" if len(reasons) > 5 else ""
        print(f"Profile {profile}: {shown}{extra}")
    for note in plan.notes:
        print(f"NOTE: {note}")
    for check in plan.checks:
        print(f"  {check.name}: {shlex.join(check.argv)}")
    sys.stdout.flush()


def check_skills(repo: Path, paths: list[str]) -> int:
    import yaml  # Declared, locked project dependency.
    errors = []
    for name in paths:
        path = repo / name
        if not path.exists():  # Deleted files remain selection inputs.
            continue
        text = path.read_text(encoding="utf-8")
        match = re.match(r"^---\n(.*?)\n---(?:\n|$)", text, re.S)
        try:
            metadata = yaml.safe_load(match[1]) if match else None
            if not isinstance(metadata, dict) or any(
                not isinstance(metadata.get(key), str) or not metadata[key].strip()
                for key in ("name", "description")
            ):
                errors.append(f"{name}: skill needs name and description frontmatter")
        except yaml.YAMLError as error:
            errors.append(f"{name}: invalid skill frontmatter: {error}")
    for error in errors:
        print(f"FAIL: {error}", file=sys.stderr)
    if not errors:
        print(f"Skill frontmatter checks passed for {len(paths)} selected paths.")
    return int(bool(errors))


def check_ledgers(repo: Path) -> int:
    root = repo / PLAN
    spec = importlib.util.spec_from_file_location("feature_ledger", root / "validate_feature_ledger.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    errors = []
    for path in sorted(root.glob("feature-ledger.px*.json")):
        document = json.loads(path.read_text(encoding="utf-8"))
        errors.extend(f"{path.name}: {error}" for error in module.validate_ledger(document))
    for error in errors:
        print(f"FAIL: {error}", file=sys.stderr)
    if not errors:
        print("Process Explorer individual ledgers passed.")
    return int(bool(errors))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--hook", nargs=2, metavar=("REMOTE", "URL"))
    parser.add_argument("--base", default="origin/main")
    parser.add_argument("--head", default="HEAD")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--json", action="store_true", help="Print a plan without executing checks")
    parser.add_argument("--check-skills", action="store_true")
    parser.add_argument("--check-ledgers", action="store_true")
    parser.add_argument("paths", nargs="*")
    args = parser.parse_args()
    try:
        repo = Path(git(Path.cwd(), "rev-parse", "--show-toplevel"))
        if args.check_skills or args.check_ledgers:
            if args.hook:
                raise CheckError("Validation modes cannot override hook selection.")
            return check_skills(repo, args.paths) if args.check_skills else check_ledgers(repo)
        if args.paths:
            raise CheckError("Paths are selected from Git revisions, not command-line overrides.")
        if args.hook:
            updates = pushed_updates(sys.stdin.read())
            plan = make_plan(repo, updates, args.hook[0], platform.system())
        else:
            head = commit(repo, args.head)
            try:
                base = git(repo, "merge-base", commit(repo, args.base), head)
            except CheckError:
                base = "f" * len(head)  # Deliberately unresolved -> full fallback.
            plan = make_plan(repo, [("preview", head, "preview", base)], "origin", platform.system())
        if args.json:
            print(json.dumps(asdict(plan), indent=2))
            return 0
        print_plan(plan)
        # Non-hook invocation is always a read-only preview for agents/humans.
        return guarded_run(repo, plan) if args.hook and not args.dry_run else 0
    except (CheckError, OSError, ValueError) as error:
        print(f"pre-push: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
