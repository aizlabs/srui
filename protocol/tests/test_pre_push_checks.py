"""Regression tests for actual pushed ranges and focused pre-push execution.

Uses temporary Git repositories and stub toolchains; no product builds or network.
The existing protocol pytest CI job also discovers this unittest suite.
"""
from contextlib import redirect_stderr, redirect_stdout
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
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


if __name__ == "__main__":
    unittest.main()
