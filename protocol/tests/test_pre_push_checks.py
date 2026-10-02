"""Regression tests for actual pushed ranges and focused pre-push execution.

Uses temporary Git repositories and stub toolchains; no product builds or network.
The existing protocol pytest CI job also discovers this unittest suite.
"""
from contextlib import redirect_stderr, redirect_stdout
import importlib.util
import io
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("pre_push_checks", ROOT / "scripts/pre_push_checks.py")
checks = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = checks
SPEC.loader.exec_module(checks)
ZERO = "0" * 40


class SelectionTests(unittest.TestCase):
    def profiles(self, paths):
        return set(checks.classify(paths))

    def test_app_and_native_changes_do_not_select_core_or_benchmarks(self):
        paths = [
            "apps/srtop/src/source.rs", "apps/srtop/Cargo.toml", "apps/srtop/Cargo.lock",
            checks.NATIVE_TEST, "apps/srtop/README.md",
            checks.PLAN + "/feature-ledger.px002.json",
        ]
        profiles = checks.classify(paths)
        self.assertEqual(set(profiles), {"srtop", "docs", "plan"})
        commands = checks.commands(profiles, paths, [], [], "Darwin")
        argv = [item.argv for item in commands]
        self.assertIn(["bash", "apps/srtop/test.sh"], argv)
        self.assertFalse(any("pre-push-full" in " ".join(command) for command in argv))
        self.assertFalse(any("Benchmarks" in " ".join(command) for command in argv))
        self.assertFalse(any(command[:2] == ["cargo", "test"] for command in argv),
                         "the native entrypoint already runs the app's Rust tests")

    def test_documentation_and_skills_do_not_build_packages(self):
        paths = ["CLAUDE.md", ".agents/skills/process-explorer-orchestrator/SKILL.md"]
        self.assertEqual(self.profiles(paths), {"docs"})
        commands = checks.commands(checks.classify(paths), paths, [], [], "Darwin")
        self.assertEqual([item.name for item in commands], ["skill frontmatter"])

    def test_plan_python_changes_run_validators_and_regressions_without_app_builds(self):
        for name in ("validate_plan.py", "validate_feature_ledger.py",
                     "test_validate_feature_ledger.py"):
            for system in ("Darwin", "Linux"):
                with self.subTest(name=name, system=system):
                    paths = [checks.PLAN + "/" + name]
                    commands = checks.commands(checks.classify(paths), paths, [], [], system)
                    self.assertEqual([item.argv for item in commands], [
                        ["python3", checks.PLAN + "/validate_plan.py"],
                        ["python3", checks.SCRIPT, "--check-ledgers"],
                        ["python3", "-m", "unittest", "discover", "-s", checks.PLAN,
                         "-p", "test_*.py"],
                    ])

    def test_plan_data_changes_only_run_validators(self):
        paths = [checks.PLAN + "/feature-ledger.px002.json"]
        commands = checks.commands(checks.classify(paths), paths, [], [], "Darwin")
        self.assertEqual([item.name for item in commands], [
            "Process Explorer plan", "Process Explorer ledgers",
        ])

    def test_shared_dependencies_and_unknown_code_fall_back_to_full(self):
        for path in ["server-rust/sdk/src/lib.rs", "protocol/srui.proto",
                     "client-macos/RendererAppKit/AppKitRenderer.swift",
                     "client-macos/Package.resolved", "uv.lock",
                     ".github/workflows/ci.yml", "CMakeLists.txt", "new-app/main.py"]:
            with self.subTest(path=path):
                self.assertEqual(self.profiles([path]), {"full"})

    def test_hook_changes_run_the_selector_suite(self):
        commands = checks.commands(checks.classify(sorted(checks.HOOK_FILES)), [], [], [], "Darwin")
        self.assertIn(["python3", "-m", "unittest", "discover", "-s", "protocol/tests",
                       "-p", "test_pre_push_checks.py"], [item.argv for item in commands])
        self.assertFalse(any(item.name == "full repository checks" for item in commands))

    def test_combined_full_profile_does_not_repeat_app_checks(self):
        paths = ["server-rust/sdk/src/lib.rs", "apps/srtop/src/main.rs"]
        commands = checks.commands(checks.classify(paths), paths, [], [], "Darwin")
        self.assertIn(["sh", ".githooks/pre-push-full"], [item.argv for item in commands])
        self.assertNotIn(["bash", "apps/srtop/test.sh"], [item.argv for item in commands])

    def test_linux_uses_rust_and_does_not_claim_appkit(self):
        paths = ["apps/srtop/src/main.rs"]
        commands = checks.commands(checks.classify(paths), paths, [], [], "Linux")
        self.assertIn(["cargo", "test", "--locked", "--manifest-path", "apps/srtop/Cargo.toml"],
                      [item.argv for item in commands])
        self.assertNotIn(["bash", "apps/srtop/test.sh"], [item.argv for item in commands])

    def test_linux_native_changes_select_syntax_check_only_when_needed(self):
        paths = [checks.NATIVE_TEST]
        commands = checks.commands(checks.classify(paths), paths, [], [], "Linux")
        parsers = [item for item in commands if item.argv[-1] == checks.NATIVE_TEST]
        self.assertEqual(len(parsers), 1)
        self.assertIn("swiftc -frontend -parse", parsers[0].argv[2])
        for system, paths in (("Darwin", [checks.NATIVE_TEST]),
                              ("Linux", ["apps/srtop/src/main.rs"])):
            commands = checks.commands(checks.classify(paths), paths, [], [], system)
            self.assertFalse(any("swiftc" in " ".join(item.argv) for item in commands))

    def test_malformed_hook_input_is_rejected(self):
        for text in ["\n", "one two", f"ref $(touch) ref {ZERO}", f"ref {'a'*39} ref {ZERO}"]:
            with self.subTest(text=text):
                self.assertRaises(checks.CheckError, checks.pushed_updates, text)
        self.assertEqual(checks.pushed_updates(""), [])


class GitFixture(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="srui-pre-push-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self.git("init", "-q", "--initial-branch=main")
        self.git("config", "user.name", "Pre-push test")
        self.git("config", "user.email", "pre-push@example.invalid")
        self.git("config", "commit.gpgsign", "false")
        self.git("config", "core.hooksPath", ".githooks")
        self.write("README.md", "# Fixture\n")
        self.base = self.save()
        self.git("update-ref", "refs/remotes/origin/main", self.base)

    def git(self, *args):
        return checks.git(self.repo, *args)

    def write(self, name, text):
        path = self.repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        return path

    def save(self):
        self.git("add", "--all")
        self.git("commit", "-qm", "fixture")
        return self.git("rev-parse", "HEAD")

    def plan(self, head=None, old=ZERO, system="Darwin"):
        return checks.make_plan(self.repo, [("refs/heads/work", head or self.git("rev-parse", "HEAD"),
                                            "refs/heads/work", old)], "origin", system)

    def test_new_branch_uses_committed_diff_even_when_worktree_is_clean(self):
        self.write("apps/srtop/src/main.rs", "// app\n")
        head = self.save()
        self.assertEqual(self.git("status", "--porcelain"), "")
        plan = self.plan(head)
        self.assertEqual(plan.ranges, [(self.base, head)])
        self.assertEqual(plan.paths, ["apps/srtop/src/main.rs"])
        self.assertEqual(set(plan.profiles), {"srtop"})

    def test_existing_branch_uses_advertised_remote_tip(self):
        self.write("server-rust/sdk/src/lib.rs", "// already pushed\n")
        old = self.save()
        self.write("apps/srtop/src/main.rs", "// newly pushed\n")
        head = self.save()
        plan = self.plan(head, old)
        self.assertEqual(plan.ranges, [(old, head)])
        self.assertEqual(set(plan.profiles), {"srtop"})

    def test_code_renamed_to_markdown_still_requires_full(self):
        self.write("server-rust/sdk/src/lib.rs", "// code\n")
        old = self.save()
        self.git("mv", "server-rust/sdk/src/lib.rs", "README-renamed.md")
        plan = self.plan(self.save(), old)
        self.assertIn("server-rust/sdk/src/lib.rs", plan.paths)
        self.assertEqual(set(plan.profiles), {"full", "docs"})

    def test_force_push_includes_removed_shared_code(self):
        self.write("server-rust/sdk/src/lib.rs", "// remote-only code\n")
        old = self.save()
        plan = self.plan(self.base, old)
        self.assertIn("server-rust/sdk/src/lib.rs", plan.paths)
        self.assertIn("full", plan.profiles)

    def test_unknown_remote_base_falls_back_to_full(self):
        self.git("update-ref", "-d", "refs/remotes/origin/main")
        plan = self.plan()
        self.assertIn("full", plan.profiles)
        self.assertIn("Unresolved comparison base", plan.profiles["full"][0])
        self.assertIn(["sh", ".githooks/pre-push-full"], [item.argv for item in plan.checks])

    def test_deleted_refs_need_no_tests(self):
        plan = checks.make_plan(self.repo, [("(delete)", ZERO, "refs/heads/old", self.base)], "origin", "Darwin")
        self.assertEqual(plan.targets, [])
        self.assertEqual(plan.checks, [])

    def test_multiple_refs_union_their_changes(self):
        self.write("apps/srtop/src/main.rs", "// app\n")
        middle = self.save()
        self.write("README.md", "# Updated fixture\n")
        head = self.save()
        plan = checks.make_plan(self.repo, [
            ("refs/heads/a", head, "refs/heads/a", self.base),
            ("refs/heads/b", head, "refs/heads/b", middle),
        ], "origin", "Darwin")
        self.assertEqual(set(plan.profiles), {"docs", "srtop"})
        self.assertEqual(sum(item.argv == ["bash", "apps/srtop/test.sh"] for item in plan.checks), 1)

    def test_wrong_or_dirty_checkout_is_rejected(self):
        self.write("README.md", "# Updated\n")
        head = self.save()
        self.assertRaises(checks.CheckError, checks.require_candidate, self.repo, [self.base])
        self.assertRaises(checks.CheckError, checks.require_candidate, self.repo, [self.base, head])
        self.write("README.md", "# Uncommitted\n")
        self.assertRaises(checks.CheckError, checks.require_candidate, self.repo, [head])

    def test_untracked_source_is_rejected(self):
        self.write("apps/srtop/tests/untracked.rs", "// could affect tests\n")
        self.assertRaises(checks.CheckError, checks.require_candidate, self.repo, [self.base])

    def test_nul_separated_paths_preserve_spaces_and_newlines(self):
        self.write("docs/space name\nsecond.md", "# Example\n")
        plan = self.plan(self.save())
        self.assertEqual(plan.paths, ["docs/space name\nsecond.md"])
        self.assertEqual(set(plan.profiles), {"docs"})

    def test_runner_stops_after_failure(self):
        marker = self.root / "must-not-run"
        plan = checks.Plan([self.base], [], [], {}, [
            checks.Check("failure", [sys.executable, "-c", "raise SystemExit(7)"]),
            checks.Check("later", [sys.executable, "-c",
                                  f"from pathlib import Path; Path({str(marker)!r}).touch()"]),
        ], [])
        with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
            self.assertEqual(checks.run_plan(self.repo, plan), 1)
        self.assertFalse(marker.exists())

    def test_runner_rejects_source_mutated_by_a_check(self):
        plan = checks.Plan([self.base], [], [], {}, [
            checks.Check("mutation", [sys.executable, "-c",
                "from pathlib import Path; Path('README.md').write_text('changed')"]),
        ], [])
        with redirect_stdout(io.StringIO()):
            self.assertRaises(checks.CheckError, checks.run_plan, self.repo, plan)

    def test_preview_reads_the_named_revision_without_requiring_checkout(self):
        self.write("apps/srtop/src/main.rs", "// app\n")
        self.save()
        result = subprocess.run(
            [sys.executable, str(ROOT / checks.SCRIPT), "--base", self.base, "--json"],
            cwd=self.repo, check=True, capture_output=True, text=True,
        )
        self.assertEqual(set(json.loads(result.stdout)["profiles"]), {"srtop"})

    def test_plain_document_push_only_checks_whitespace(self):
        self.write("README.md", 'Use ' + chr(96) + '[x](missing.md)' + chr(96)
                   + ' as syntax.\n\n[guide](absent.md "Guide title")\n')
        plan = self.plan(self.save())
        self.assertEqual([item.name for item in plan.checks], ["whitespace"])
        with redirect_stdout(io.StringIO()):
            self.assertEqual(checks.run_plan(self.repo, plan), 0)

    def test_linux_native_syntax_failure_blocks_push_and_deleted_file_is_skipped(self):
        self.write(checks.NATIVE_TEST, "func broken( {\n")
        head = self.save()
        plan = self.plan(head, system="Linux")
        parser = next(item for item in plan.checks if item.argv[-1] == checks.NATIVE_TEST)
        bin_dir = self.root / "bin"
        bin_dir.mkdir()
        swiftc = bin_dir / "swiftc"
        swiftc.write_text('#!/bin/sh\n[ "$1" = "-frontend" ] && [ "$2" = "-parse" ] || exit 99\nexit 7\n')
        swiftc.chmod(0o755)
        parser_plan = checks.Plan([head], [], [], {}, [parser], [])
        with patch.dict(os.environ, PATH=str(bin_dir) + os.pathsep + os.environ["PATH"]):
            with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                self.assertEqual(checks.run_plan(self.repo, parser_plan), 1)
                self.git("rm", checks.NATIVE_TEST)
                deleted = self.save()
                parser_plan.targets = [deleted]
                self.assertEqual(checks.run_plan(self.repo, parser_plan), 0)

    def test_inherited_git_context_cannot_redirect_fixture_operations(self):
        foreign = self.root / "foreign"
        foreign.mkdir()
        checks.git(foreign, "init", "-q")
        before = (foreign / ".git/config").read_bytes()
        inherited = {
            "GIT_DIR": str(foreign / ".git"),
            "GIT_COMMON_DIR": str(foreign / ".git"),
            "GIT_WORK_TREE": str(foreign),
            "GIT_INDEX_FILE": str(foreign / ".git/index"),
            "GIT_IMPLICIT_WORK_TREE": "0",
        }
        with patch.dict(os.environ, inherited):
            self.assertEqual(self.git("rev-parse", "HEAD"), self.base)
            self.git("config", "test.isolation", "yes")
            plan = checks.Plan([self.base], [], [], {}, [
                checks.Check("environment", [sys.executable, "-c",
                    "import os; assert not any(k in os.environ for k in "
                    "('GIT_DIR', 'GIT_COMMON_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE', 'GIT_IMPLICIT_WORK_TREE'))"]),
            ], [])
            with redirect_stdout(io.StringIO()):
                self.assertEqual(checks.run_plan(self.repo, plan), 0)
        self.assertEqual((foreign / ".git/config").read_bytes(), before)
        self.assertFalse((foreign / ".git/index").exists())

    def test_real_git_push_invokes_focused_hook(self):
        # Install the production wrapper/selector and exact app test entrypoint
        # into a disposable repo, with fake toolchains that only record argv.
        for name in (".githooks/pre-push", checks.SCRIPT, "apps/srtop/test.sh"):
            destination = self.repo / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / name, destination)
        (self.repo / ".githooks/pre-push").chmod(0o755)
        self.write("scripts/check-test-process-stdio.sh", "#!/bin/sh\nexit 0\n")
        self.write(".githooks/pre-push-full", "#!/bin/sh\nexit 99\n")
        self.write("apps/srtop/src/main.rs", "// base\n")
        self.save()
        bare = self.root / "remote.git"
        subprocess.run(["git", "clone", "--bare", "-q", str(self.repo), str(bare)], check=True)
        self.git("remote", "add", "origin", str(bare))
        self.git("fetch", "-q", "origin")
        self.git("switch", "-qc", "codex/app-change")
        self.write("apps/srtop/src/main.rs", "// changed app\n")
        self.save()
        bin_dir = self.root / "bin"
        bin_dir.mkdir()
        log = self.root / "toolchain.log"
        stub = "#!/bin/sh\nprintf '%s %s\\n' \"$0\" \"$*\" >> \"$SRUI_HOOK_TEST_LOG\"\n"
        for name in ("cargo", "swift"):
            path = bin_dir / name
            path.write_text(stub)
            path.chmod(0o755)
        env = dict(os.environ, PATH=str(bin_dir) + os.pathsep + os.environ["PATH"],
                   SRUI_HOOK_TEST_LOG=str(log))
        result = subprocess.run(["git", "push", "-u", "origin", "codex/app-change"],
                                cwd=self.repo, env=env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("Profile srtop:", result.stdout)
        self.assertNotIn("Profile full:", result.stdout)
        executed = log.read_text()
        self.assertIn("fmt --manifest-path apps/srtop/Cargo.toml", executed)
        self.assertIn("clippy --locked --manifest-path apps/srtop/Cargo.toml", executed)
        self.assertIn("test --locked --manifest-path apps/srtop/Cargo.toml", executed)
        self.assertNotIn("Benchmarks", executed)
        self.assertEqual(subprocess.check_output(
            ["git", "--git-dir", str(bare), "rev-parse", "refs/heads/codex/app-change"],
            text=True).strip(), self.git("rev-parse", "HEAD"))


# Runs checks.guarded_run with a stub runner, so the single-flight behaviour is proved
# without the real multi-minute cargo/SwiftPM checks. Every lock transition is appended
# to one shared log; time.monotonic is system-wide, so the stamps order across processes.
# A hook run whose single check is a long-running script, driven through the real
# `run_plan` so the child is spawned exactly as a cargo or SwiftPM check would be.
SIGNAL_HELPER = '''
import importlib.util, sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("pre_push_checks", sys.argv[1])
checks = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = checks
spec.loader.exec_module(checks)

repo, script = Path(sys.argv[2]), sys.argv[3]
plan = checks.Plan([], [], [], {}, [checks.Check("slow", [script])], [])
raise SystemExit(checks.guarded_run(repo, plan, timeout=30, poll=0.02))
'''

LOCK_HELPER = '''
import importlib.util, os, sys, time
from pathlib import Path

spec = importlib.util.spec_from_file_location("pre_push_checks", sys.argv[1])
checks = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = checks
spec.loader.exec_module(checks)

repo, log = Path(sys.argv[2]), Path(sys.argv[3])
hold, timeout = float(sys.argv[4]), float(sys.argv[5])

def record(event):
    with log.open("a") as handle:
        handle.write("%s %d %d\\n" % (event, os.getpid(), time.monotonic_ns()))

def runner(_repo, _plan):
    record("enter")
    time.sleep(hold)
    record("exit")
    return 0

# A plan that *has* a check, because only such a plan takes the lock: one with none
# bypasses it deliberately (see `guarded_run`), and staging an empty one here would
# make every serialization assertion below pass without a lock existing at all.
# `argv` is never executed, since `runner` is stubbed.
empty = len(sys.argv) > 6 and sys.argv[6] == '--no-checks'
stub = [] if empty else [checks.Check('stub', ['true'])]
plan = checks.Plan([], [], [], {}, stub, [])
raise SystemExit(checks.guarded_run(repo, plan, runner=runner, timeout=timeout, poll=0.02))
'''


class SingleFlightTests(unittest.TestCase):
    """One hook run at a time: concurrent pushes used to deadlock in the build directory."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="srui-pre-push-lock-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        for args in (("init", "-q", "--initial-branch=main"),
                     ("config", "user.name", "Pre-push test"),
                     ("config", "user.email", "pre-push@example.invalid"),
                     ("config", "commit.gpgsign", "false")):
            checks.git(self.repo, *args)
        (self.repo / "README.md").write_text("# Fixture\n")
        checks.git(self.repo, "add", "--all")
        checks.git(self.repo, "commit", "-qm", "fixture")
        self.helper = self.root / "lock_helper.py"
        self.helper.write_text(LOCK_HELPER)
        self.log = self.root / "lock.log"
        self.log.touch()

    def spawn(self, hold, timeout, repo=None, checks_planned=True):
        repo = self.repo if repo is None else repo
        process = subprocess.Popen(
            [sys.executable, str(self.helper), str(ROOT / checks.SCRIPT), str(repo),
             str(self.log), str(hold), str(timeout)]
            + ([] if checks_planned else ["--no-checks"]),
            cwd=str(repo), stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        )
        self.addCleanup(self.reap, process)
        return process

    def reap(self, process):
        if process.poll() is None:
            process.kill()
        try:
            process.communicate(timeout=30)
        except (subprocess.TimeoutExpired, ValueError):
            pass

    def events(self):
        entries = []
        for line in self.log.read_text().splitlines():
            event, pid, stamp = line.split()
            entries.append((event, int(pid), int(stamp)))
        return entries

    def await_holder(self, process):
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if any(event == "enter" for event, _pid, _stamp in self.events()):
                return
            self.assertIsNone(process.poll(), "the holder exited before taking the lock")
            time.sleep(0.02)
        self.fail("the holder never reported entering the checks")

    def test_concurrent_invocations_serialize_instead_of_overlapping(self):
        holder = self.spawn(hold=1.0, timeout=30)
        self.await_holder(holder)
        waiter = self.spawn(hold=0.0, timeout=60)
        for process in (holder, waiter):
            stdout, stderr = process.communicate(timeout=120)
            self.assertEqual(process.returncode, 0, stdout + stderr)
        events = self.events()
        self.assertEqual([event for event, _pid, _stamp in events],
                         ["enter", "exit", "enter", "exit"], events)
        self.assertEqual({pid for _event, pid, _stamp in events}, {holder.pid, waiter.pid})
        first_exit = next(stamp for event, pid, stamp in events
                          if event == "exit" and pid == holder.pid)
        second_enter = next(stamp for event, pid, stamp in events
                            if event == "enter" and pid == waiter.pid)
        self.assertGreaterEqual(
            second_enter, first_exit,
            "the second push ran its checks while the first still held the lock")

    def test_a_signalled_hook_stops_its_check_before_releasing_the_lock(self):
        """The lock must outlive the build it was taken for.

        `git` signals the hook, not its process group. A check left running after the
        hook exits keeps writing to the shared build directory while the lock is
        already gone, and the next push then acquires the lock and starts a second
        build into it - the overlap this mechanism exists to prevent.
        """
        marker = self.root / "check.pid"
        script = self.root / "slow_check.sh"
        script.write_text(
            "#!/bin/sh\n"
            f"printf '%s' \"$$\" > {marker}\n"
            "sleep 60\n"
        )
        script.chmod(0o755)
        helper = self.root / "signal_helper.py"
        helper.write_text(SIGNAL_HELPER)
        hook = subprocess.Popen(
            [sys.executable, str(helper), str(ROOT / checks.SCRIPT), str(self.repo),
             str(script)],
            cwd=str(self.repo), stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        )
        self.addCleanup(self.reap, hook)

        deadline = time.monotonic() + 30
        while not marker.exists():
            self.assertIsNone(hook.poll(), "the hook exited before running its check")
            self.assertLess(time.monotonic(), deadline, "the check never started")
            time.sleep(0.02)
        child = int(marker.read_text())
        os.kill(child, 0)  # Running, and ours to observe.

        os.kill(hook.pid, signal.SIGTERM)  # The hook alone, exactly as git does.
        stdout, stderr = hook.communicate(timeout=60)
        self.assertNotEqual(hook.returncode, 0, stdout + stderr)

        # By the time the hook is gone - and so by the time the lock is free - the
        # check must be gone too. No sleep here on purpose: a grace period would let
        # the assertion pass for a build that is merely slow to notice.
        with self.assertRaises(OSError, msg="the check outlived the hook that held the lock"):
            os.kill(child, 0)

    def test_a_nested_process_group_is_stopped_with_the_check(self):
        """A wrapper's own process group must not outlive the lock either.

        `scripts/run-swift-tests.sh` puts `swift test` in a *new* process group with
        `set -m`, so signalling the check's group does not reach it. That is the real
        shape of the macOS full hook, and the shape this reproduces: killing the
        directly spawned shell is not enough, because the build is one group boundary
        further in.
        """
        marker = self.root / "nested.pid"
        script = self.root / "wrapper.sh"
        script.write_text(
            "#!/bin/sh\n"
            "set -m\n"          # exactly what run-swift-tests.sh does
            "sleep 60 &\n"
            f"printf '%s' \"$!\" > {marker}\n"
            "wait\n"
        )
        script.chmod(0o755)
        helper = self.root / "signal_helper.py"
        helper.write_text(SIGNAL_HELPER)
        hook = subprocess.Popen(
            [sys.executable, str(helper), str(ROOT / checks.SCRIPT), str(self.repo),
             str(script)],
            cwd=str(self.repo), stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        )
        self.addCleanup(self.reap, hook)

        deadline = time.monotonic() + 30
        while not marker.exists() or not marker.read_text().strip():
            self.assertIsNone(hook.poll(), "the hook exited before the wrapper started")
            self.assertLess(time.monotonic(), deadline, "the nested job never started")
            time.sleep(0.02)
        nested = int(marker.read_text())
        os.kill(nested, 0)
        self.assertNotEqual(os.getpgid(nested), os.getpgid(hook.pid),
                            "the fixture must put the nested job in its own group")

        os.kill(hook.pid, signal.SIGTERM)
        stdout, stderr = hook.communicate(timeout=60)
        self.assertNotEqual(hook.returncode, 0, stdout + stderr)
        with self.assertRaises(OSError, msg="a nested process group outlived the lock"):
            os.kill(nested, 0)

    def test_a_drained_check_costs_no_second_grace_window_and_no_stale_signal(self):
        """A check that dies on SIGTERM must end the stop, not start a SIGKILL pass.

        Two consequences of getting this wrong, both asserted here: every interrupted
        push burns both grace windows, and the later pass aims SIGKILL at a pid that
        `Popen` has already reaped -- a number the kernel is free to have reissued to
        an unrelated process of this user.
        """
        script = self.root / "quick_check.sh"
        script.write_text("#!/bin/sh\nsleep 60\n")
        script.chmod(0o755)
        signals = []
        real_kill, real_killpg = os.kill, os.killpg

        def record_kill(pid, sig):
            signals.append(("kill", pid, sig))
            return real_kill(pid, sig)

        def record_killpg(pgid, sig):
            signals.append(("killpg", pgid, sig))
            return real_killpg(pgid, sig)

        started = time.monotonic()
        with patch.object(checks.os, "kill", record_kill), \
             patch.object(checks.os, "killpg", record_killpg):
            thread = threading.Thread(target=checks.run_check, args=([str(script)], self.repo))
            thread.start()
            deadline = time.monotonic() + 30
            while checks._active_check is None:
                self.assertLess(time.monotonic(), deadline, "the check never registered")
                time.sleep(0.01)
            root = checks._active_check.pid
            checks.stop_active_check(grace=5.0)
            elapsed = time.monotonic() - started
            thread.join(timeout=30)

        self.assertLess(elapsed, 5.0,
                        f"the stop burned a grace window for a check that died: {elapsed:.2f}s")
        self.assertNotIn(signal.SIGKILL, [sig for _kind, _target, sig in signals],
                         f"SIGKILL was sent to a check that had already gone: {signals}")
        self.assertNotIn(("kill", root, signal.SIGTERM), signals,
                         "the root pid was signalled by number; after reaping it may be reused")
        self.assertNotIn(("kill", root, signal.SIGKILL), signals,
                         "the root pid was signalled by number; after reaping it may be reused")
        with self.assertRaises(OSError):
            os.kill(root, 0)

    def test_a_zombie_in_the_group_does_not_count_as_a_live_member(self):
        """An uncollected exit status is not a process still using the build directory.

        A zombie holds no resources and cannot be signalled, so counting one keeps the
        drain from ever succeeding: the stop burns its grace window and escalates to
        SIGKILL against something that has already gone.
        """
        # A Python parent, not a shell: `sh` reaps its background jobs on SIGCHLD, so
        # it leaves no zombie to observe. This one forks and deliberately never waits.
        marker = self.root / "zombie.pid"
        script = self.root / "leaves_a_zombie.py"
        script.write_text(
            "import os, sys, time\n"
            "pid = os.fork()\n"
            "if pid == 0:\n"
            "    os._exit(0)\n"
            f"open({str(marker)!r}, 'w').write(str(pid))\n"
            "time.sleep(60)\n"
        )
        child = subprocess.Popen([sys.executable, str(script)], stdin=subprocess.DEVNULL,
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                 start_new_session=True)
        self.addCleanup(self.reap, child)

        deadline = time.monotonic() + 30
        while not marker.exists() or not marker.read_text().strip():
            self.assertLess(time.monotonic(), deadline, "the fixture never reported its child")
            time.sleep(0.02)
        zombie = int(marker.read_text())

        # Wait for it to actually be a zombie: still in the table, state Z.
        def state_of(pid):
            row = subprocess.run(["ps", "-o", "state=", "-p", str(pid)],
                                 stdout=subprocess.PIPE, text=True, check=False)
            return row.stdout.strip()

        while not state_of(zombie).startswith("Z"):
            self.assertLess(time.monotonic(), deadline, f"never became a zombie: {state_of(zombie)!r}")
            time.sleep(0.02)

        members = checks.live_group_members({child.pid})
        self.assertIn(child.pid, members, "the live shell must count as a member")
        self.assertNotIn(zombie, members, "a zombie was counted as a live group member")

    def test_the_handled_signals_are_blocked_while_a_check_is_registered(self):
        """No window between spawning a check and recording it.

        A signal delivered in that window would find no active check, stop nothing,
        and release the lock over a build that had just started.
        """
        observed = {}

        class FakePopen:
            def __init__(self, *_args, **_kwargs):
                observed["mask"] = signal.pthread_sigmask(signal.SIG_BLOCK, [])
                observed["active_at_spawn"] = checks._active_check
                self.pid = os.getpid()

            def wait(self):
                observed["active_while_waiting"] = checks._active_check is self
                return 0

            def poll(self):
                return 0

        before = signal.pthread_sigmask(signal.SIG_BLOCK, [])
        with patch.object(checks.subprocess, "Popen", FakePopen):
            self.assertEqual(checks.run_check(["true"], self.repo), 0)
        self.assertTrue(set(checks.STOP_SIGNALS).issubset(observed["mask"]),
                        f"the handled signals were deliverable during the spawn: {observed['mask']}")
        self.assertTrue(observed["active_while_waiting"], "the check was never registered")
        self.assertEqual(signal.pthread_sigmask(signal.SIG_BLOCK, []), before,
                         "the mask was not restored after registration")
        self.assertIsNone(checks._active_check, "the check outlived its own run")

    def test_a_plan_with_no_checks_never_waits_for_the_lock(self):
        """A deletion-only push plans no checks, so it must not queue behind a build.

        `make_plan` skips ref deletions, so `git push --delete` has an empty plan. It
        cannot deadlock in the build directory, because it builds nothing - and before
        this, it waited out the holder's full timeout in order to run nothing at all.
        """
        holder = self.spawn(hold=30.0, timeout=30)
        self.await_holder(holder)
        # A zero timeout: were this run to take the lock at all, it would fail at once
        # with LOCK_BUSY_STATUS rather than run.
        deletion = self.spawn(hold=0.0, timeout=0, checks_planned=False)
        stdout, stderr = deletion.communicate(timeout=60)
        self.assertEqual(deletion.returncode, 0, stdout + stderr)
        self.assertNotIn("waiting up to", stderr)
        self.assertIn(deletion.pid, [pid for _event, pid, _stamp in self.events()],
                      "the deletion-only push must still run its (empty) plan")

    def test_bounded_wait_reports_the_holder_then_exits_with_the_documented_status(self):
        holder = self.spawn(hold=30.0, timeout=30)
        self.await_holder(holder)
        waiter = self.spawn(hold=0.0, timeout=0)
        stdout, stderr = waiter.communicate(timeout=60)
        self.assertEqual(waiter.returncode, checks.LOCK_BUSY_STATUS, stdout + stderr)
        self.assertIn("waiting up to", stderr)
        self.assertIn(str(holder.pid), stderr)
        self.assertIn(checks.LOCK_FILE, stderr)
        self.assertIn("--no-verify", stderr)
        self.assertNotIn(waiter.pid, [pid for _event, pid, _stamp in self.events()],
                         "the blocked push must not run any check")
        self.reap(holder)

    def test_the_lock_is_shared_across_worktrees_of_one_repository(self):
        worktree = self.root / "linked"
        checks.git(self.repo, "worktree", "add", "-q", "-b", "codex/linked", str(worktree))
        self.assertFalse((worktree / ".git").is_dir(), "a linked worktree's .git is a file")
        self.assertEqual(checks.lock_path(worktree), checks.lock_path(self.repo))
        holder = self.spawn(hold=30.0, timeout=30)
        self.await_holder(holder)
        waiter = self.spawn(hold=0.0, timeout=0, repo=worktree)
        stdout, stderr = waiter.communicate(timeout=60)
        self.assertEqual(waiter.returncode, checks.LOCK_BUSY_STATUS, stdout + stderr)
        self.assertIn(str(holder.pid), stderr)
        self.reap(holder)

    def test_a_dead_holder_does_not_block_the_next_push(self):
        holder = self.spawn(hold=600.0, timeout=30)
        self.await_holder(holder)
        self.reap(holder)
        waiter = self.spawn(hold=0.0, timeout=5)
        stdout, stderr = waiter.communicate(timeout=60)
        self.assertEqual(waiter.returncode, 0, stdout + stderr)
        self.assertIn(("enter", waiter.pid),
                      [(event, pid) for event, pid, _stamp in self.events()])


class TargetIgnoreTests(unittest.TestCase):
    """`target/` matched directories only, so a symlinked build cache was untracked noise."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="srui-pre-push-ignore-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        checks.git(self.repo, "init", "-q", "--initial-branch=main")
        shutil.copyfile(ROOT / ".gitignore", self.repo / ".gitignore")

    def check_ignore(self, *paths):
        return subprocess.run(
            ["git", "check-ignore", "-v", "--", *paths], cwd=self.repo,
            env=checks.check_environment(), capture_output=True, text=True,
        )

    def test_a_target_symlink_is_ignored_like_a_target_directory(self):
        cache = self.root / "buildcache"
        cache.mkdir()
        (self.repo / "server-rust").mkdir()
        (self.repo / "target").mkdir()
        os.symlink(cache, self.repo / "server-rust/target")
        self.assertTrue((self.repo / "server-rust/target").is_symlink())
        result = self.check_ignore("target", "server-rust/target")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(len(result.stdout.splitlines()), 2, result.stdout)
        self.assertEqual(checks.git(self.repo, "status", "--porcelain", "--untracked-files=all"),
                         "?? .gitignore")

    def test_neighbouring_paths_are_still_visible(self):
        for name in ("server-rust/targets/keep.rs", "docs/target.md", "src/target.rs"):
            path = self.repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("keep\n")
            with self.subTest(name=name):
                self.assertEqual(self.check_ignore(name).returncode, 1)

    def test_no_tracked_path_is_named_target(self):
        # The unanchored pattern also matches a *file* named `target`. Nothing tracked is,
        # and this fails the moment something becomes so, before the file silently vanishes.
        tracked = subprocess.run(["git", "ls-files", "-z"], cwd=ROOT, check=True,
                                 env=checks.check_environment(), capture_output=True, text=True)
        named = [path for path in tracked.stdout.split("\0")
                 if PurePosixPath(path).name == "target"]
        self.assertEqual(named, [])


if __name__ == "__main__":
    unittest.main()
