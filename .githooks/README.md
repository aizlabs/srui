# Local pre-push checks

The hook selects checks from the revisions Git is about to push. Full CI remains
the merge gate; its jobs and coverage are unchanged.

| Changed paths | Local profile |
| --- | --- |
| Process Explorer source, tests, app dependencies, launcher, or its native Swift test | App formatting, Clippy, Rust tests, and native SSH integration on macOS |
| Markdown/reStructuredText, text files under docs/, and skills | Whitespace plus changed Markdown links/fences and skill frontmatter |
| Process Explorer plan Markdown/JSON/evidence | Plan and individual-ledger validators, plus applicable documentation checks |
| Hook, selector, or selector tests | Shell syntax and selector regression tests |
| Shared runtime/SDK/protocol/client, shared dependencies/configuration, other or unknown executable paths | Existing full checks |

Profiles combine. A full profile includes the app checks, so the selector does not
run a second app suite. The app's native entrypoint already runs its Rust tests
and builds the app/bridge; those steps are not duplicated either. Unchanged
infrastructure is still compiled when the app needs it, without running all its
dedicated suites.

On Linux the app profile runs Rust checks and explicitly reports native
SSH/AppKit as unavailable locally. This is not native acceptance evidence; the
ticket's required macOS verification and CI gate still apply.

Preview the current branch without executing checks:

~~~sh
python3 scripts/pre_push_checks.py --base origin/main --head HEAD
python3 scripts/pre_push_checks.py --base origin/main --head HEAD --json
~~~

Agents can use the JSON commands and reasons when recording ticket verification
scope. Required ticket-specific live, security, conformance and release evidence
may add checks beyond this path-based delivery profile.

For an existing remote branch, the hook compares the advertised remote tip with
the pushed commit. For a new branch, it uses the merge base with the cached remote
default branch (remote HEAD, main, or master). Refresh remote refs when necessary;
the hook itself does not fetch or access the network. An unresolved base selects
the full profile. Deleted refs need no code checks. Rename sources and destinations
are both classified, including a code file renamed into documentation.

The hook reads all Git pre-push ref updates, unions their profiles and prints
the ranges, selection reasons and commands. It does not inspect only uncommitted
changes. To avoid certifying the wrong code, execution requires a clean checkout
at the pushed commit; pushes of different commits must use their respective
worktrees. A generator/test that changes tracked source also blocks the push.
Ignored build artifacts are allowed. Selected checks stop at their first failure.

The previous full hook is retained for conservative fallback and explicit use:

~~~sh
sh .githooks/pre-push-full
~~~

Selector tests use disposable Git repositories and stub toolchains, including a
real local Git push. They run without product builds:

~~~sh
python3 -m unittest discover -s protocol/tests -p test_pre_push_checks.py
~~~

The existing protocol pytest CI job discovers these tests. No CI configuration
or merge protection is changed by local test selection.
