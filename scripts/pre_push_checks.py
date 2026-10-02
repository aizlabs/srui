#!/usr/bin/env python3
"""Select local checks from pushed revisions; full CI remains the merge gate.

The checks are single-flighted: cargo and SwiftPM serialize on one lock inside the
shared build directory, so two concurrent pushes used to wedge indefinitely instead
of queueing. One advisory `flock` in the repository's *common* git directory lets the
second push wait with a bounded, reported wait instead.
"""
from __future__ import annotations

import argparse
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


@dataclass
class Check:
    name: str
    argv: list[str]


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
        ]))
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
            Check("locked Python development environment", ["uv", "sync", "--frozen", "--extra", "dev"]),
            Check("full repository checks", ["sh", ".githooks/pre-push-full"]),
        ])
    elif "srtop" in profiles:
        manifest = ["--locked", "--manifest-path", "apps/srtop/Cargo.toml"]
        checks.extend([
            Check("Process Explorer formatting", [
                "cargo", "fmt", "--manifest-path", "apps/srtop/Cargo.toml",
                "-p", "srui-process-explorer", "--check",
            ]),
            Check("Process Explorer Clippy", ["cargo", "clippy", *manifest, "--all-targets", "--", "-D", "warnings"]),
            Check("test process stdio", ["bash", "scripts/check-test-process-stdio.sh"]),
        ])
        if system == "Darwin":
            # This entrypoint already tests Rust and builds the app and bridge.
            checks.append(Check("Process Explorer Rust and native SSH tests", ["bash", "apps/srtop/test.sh"]))
        else:
            if NATIVE_TEST in paths:
                # Parse exactly the selected test. The shared range-based script
                # could select unrelated Swift files; deleted tests need no parse.
                checks.append(Check("Process Explorer Swift syntax (no type checking)", [
                    "bash", "-c", 'if [ -f "$1" ]; then swiftc -frontend -parse "$1"; fi',
                    "parse-native-test", NATIVE_TEST,
                ]))
            checks.append(Check("Process Explorer Rust tests", ["cargo", "test", *manifest]))
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
        restore = install_release_on_signal()
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


def run_check(argv: list[str], cwd: Path) -> int:
    """Run one check in its own process group and wait for it.

    Its own group, so a signal delivered to this hook alone can still stop the
    build: `git` sends the hook a signal, not the group, and a cargo or SwiftPM
    child that outlives the hook keeps the shared build directory busy while the
    lock is already gone -- after which the next push acquires the lock and starts
    a second build into it, which is the overlap the lock exists to prevent.
    """
    global _active_check
    process = subprocess.Popen(argv, cwd=cwd, stdin=subprocess.DEVNULL,
                               env=check_environment(), start_new_session=True)
    _active_check = process
    try:
        return process.wait()
    finally:
        _active_check = None


def stop_active_check(grace: float = CHECK_STOP_GRACE_SECONDS) -> None:
    """Stop the running check's whole process group, and wait for it to go.

    Waiting is the point: the lock must outlive the build it was taken for, so
    this returns only once nothing is left writing to the build directory. The
    group, not the pid, because cargo and swift spawn children of their own.
    """
    process = _active_check
    if process is None or process.poll() is not None:
        return
    for sig, wait in ((signal.SIGTERM, grace), (signal.SIGKILL, grace)):
        with contextlib.suppress(OSError, ProcessLookupError):
            os.killpg(process.pid, sig)
        try:
            process.wait(timeout=wait)
            return
        except subprocess.TimeoutExpired:
            continue


def install_release_on_signal():
    """Turn termination signals into SystemExit so the lock's cleanup still runs.

    The running check is stopped *before* the lock unwinds, so the lock is never
    released while a build it was protecting is still going.
    """
    def terminate(signum, _frame):
        stop_active_check()
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

    A plan with no checks takes no lock. What the lock exists for is two runs
    driving cargo and SwiftPM at the same time, which deadlock on the shared
    build directory; a plan that will not build anything cannot deadlock with
    anything. Queueing it would only make a push that runs nothing wait out
    another one's full build -- `git push --delete`, whose plan is empty by
    construction (`make_plan` skips ref deletions), would block for up to the
    lock timeout to run no check at all.
    """
    if not plan.checks:
        return runner(repo, plan)
    try:
        with single_flight(repo, **lock):
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
