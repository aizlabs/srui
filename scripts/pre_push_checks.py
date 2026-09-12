#!/usr/bin/env python3
"""Select local checks from pushed revisions; full CI remains the merge gate."""
from __future__ import annotations

import argparse
from dataclasses import asdict, dataclass
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import shlex
import subprocess
import sys
from urllib.parse import unquote, urlsplit

SCRIPT = "scripts/pre_push_checks.py"
PLAN = "apps/srtop/srui-process-explorer-plan"
HOOK_FILES = {
    ".githooks/pre-push", ".githooks/pre-push-full", SCRIPT,
    "protocol/tests/test_pre_push_checks.py",
}
NATIVE_TEST = "client-macos/Tests/SRUITests/ProcessExplorerShellTests.swift"


class CheckError(RuntimeError):
    pass


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
    markdown = [path for path in paths if path.lower().endswith(".md")]
    if markdown:
        checks.append(Check("documentation", [
            "uv", "run", "--frozen", "python", SCRIPT, "--check-docs", "--", *markdown,
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
        result = subprocess.run(check.argv, cwd=repo, stdin=subprocess.DEVNULL, env=check_environment())
        if result.returncode:
            print(f"FAILED: {check.name} (exit {result.returncode}); push aborted.", file=sys.stderr)
            return 1
    # Detect source changes caused by generators/tests rather than certify a
    # different tree. Ignored build artifacts do not make the checkout dirty.
    require_candidate(repo, plan.targets)
    print("Selected pre-push checks passed." if plan.targets else "No code-bearing ref updates; no checks needed.")
    return 0


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


def check_docs(repo: Path, paths: list[str]) -> int:
    errors = []
    for name in paths:
        path = repo / name
        if not path.exists():  # Deleted files remain selection inputs.
            continue
        text = path.read_text(encoding="utf-8")
        if path.name == "SKILL.md":
            import yaml  # Declared, locked project dependency; only docs need it.
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
        visible, fence = [], None
        for line in text.splitlines():
            marker = re.match(r"^\s*(" + chr(96) + r"{3,}|~{3,})(.*)$", line)
            if marker:
                token, rest = marker.groups()
                if fence is None:
                    fence = token
                elif token[0] == fence[0] and len(token) >= len(fence) and not rest.strip():
                    fence = None
            elif fence is None:
                visible.append(line)
        if fence is not None:
            errors.append(f"{name}: unclosed Markdown fence")
        for target in re.findall(r"\[[^\]]*\]\(([^)]+)\)", "\n".join(visible)):
            target = target.strip("<>")
            url = urlsplit(target)
            if url.scheme or url.netloc or not url.path:
                continue
            linked = (repo / unquote(url.path.lstrip("/")) if url.path.startswith("/")
                      else path.parent / unquote(url.path))
            if not linked.exists():
                errors.append(f"{name}: missing local link {target}")
    for error in errors:
        print(f"FAIL: {error}", file=sys.stderr)
    if not errors:
        print(f"Documentation checks passed for {len(paths)} selected paths.")
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
    parser.add_argument("--check-docs", action="store_true")
    parser.add_argument("--check-ledgers", action="store_true")
    parser.add_argument("paths", nargs="*")
    args = parser.parse_args()
    try:
        repo = Path(git(Path.cwd(), "rev-parse", "--show-toplevel"))
        if args.check_docs or args.check_ledgers:
            if args.hook:
                raise CheckError("Validation modes cannot override hook selection.")
            return check_docs(repo, args.paths) if args.check_docs else check_ledgers(repo)
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
        return run_plan(repo, plan) if args.hook and not args.dry_run else 0
    except (CheckError, OSError, ValueError) as error:
        print(f"pre-push: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
