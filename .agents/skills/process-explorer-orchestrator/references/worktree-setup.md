# Worktree setup and recovery

Use this before implementation, independent verification, and delivery in a new Process Explorer worktree. Run commands through the repository's required tools, from the confirmed non-main worktree. Keep logs and run state in the absolute external run directory. Follow tool-enforced permissions; this reference does not authorize changing unrelated configuration.

## Python: install the declared development extra

Read the candidate's `pyproject.toml` and lockfile guidance. With the current SRUI layout:

```sh
uv sync --frozen --extra dev
uv run --frozen python -c 'import sys, pytest, yaml, jsonschema; print(sys.executable); print(pytest.__file__)'
uv run --frozen python -m pytest -q protocol/tests benchmarks/tests
```

`pytest` is an optional development dependency. A new `.venv` can contain `yaml` and `jsonschema` but no `pytest`. In that case, `uv run pytest` may find a global pipx executable whose interpreter cannot import the project's dependencies. Compare the interpreter/module paths rather than installing packages globally. The shared checkout's working environment is not proof that a new worktree is initialized. Use only the checked-in dependency versions; do not upgrade lockfiles as a setup repair.

## Swift: diagnose the specific package checkout

`client-macos` and `client-macos/Benchmarks` have separate SwiftPM resolution/build directories. A passing client build does not establish the nested benchmark checkout. Before delivery on macOS, when the configured hook requires it:

```sh
swift test --disable-automatic-resolution --package-path client-macos/Benchmarks -c release
```

For `unable to read tree` or a pinned-revision checkout failure, preserve the log and read that package's `Package.resolved`. Check the pinned commit and tree objects in the failing checkout, not just another package's successful checkout. Retry the exact command once if the objects are now readable. If it repeats, diagnose the affected local cache/checkout and use normal SwiftPM recovery within the authorized scope; do not repeatedly retry unchanged state, change the pin, or delete shared caches to conceal the error. Record an unresolved environment blocker precisely. The PX-001 retry passed without a pin change; this does not establish the cause of every future checkout failure.

## LemonCrow: index and bind the same worktree

Only apply this section when LemonCrow is active. A graph check is mandatory only if the assigned role or task requires it. Discover supported controls through tool metadata and `lc code index --help`; a missing MCP indexing tool is not proof that the CLI lacks indexing.

Set `verifier_root` to the absolute candidate worktree recorded in run state, then index it:

```sh
lc code index --repo-root "$verifier_root" --json
```

Index creation alone is insufficient. A host MCP session may remain bound to the launcher checkout. Before relying on a search result, query a known changed symbol and confirm that returned paths and contents belong to the candidate. Results from the implementer or shared main checkout are not independent candidate graph evidence.

For a fresh child MCP session, use the installed `lemoncrow.core.foundation.paths.pin_workspace_env(verifier_root, child_env)` helper where available. Its equivalent configuration in the observed runtime is:

```sh
env -u WORKSPACE_FOLDER_PATHS \
  CURSOR_WORKSPACE_ROOT="$verifier_root" \
  LEMONCROW_WORKSPACE_ROOT="$verifier_root" \
  CLAUDE_WORKSPACE_ROOT="$verifier_root" \
  VSCODE_CWD="$verifier_root" \
  lc mcp --host process-explorer-verifier
```

Launch the child with its working directory set to `verifier_root`, connect with the supported MCP client, and repeat the candidate-local query. A single `LEMONCROW_WORKSPACE_ROOT` override is insufficient when higher-priority host variables survive. Apply these pins only to the child session; do not rewrite global settings or disrupt other active sessions. If the installed version differs, inspect its workspace-selection help/API before guessing variable names. Keep query logs proving the candidate binding.

## Recover before declaring a terminal blocker

Distinguish expected mutation-test failures, ordinary implementation defects, local setup errors, and failed acceptance criteria. A setup error warrants a bounded, evidence-based repair within existing authorization, followed by the affected check. Do not ask for permission again merely to install declared dependencies, correct an implementation fixture, or repair child-session binding. The verifier never edits product/tests to make a failure pass.

Stop when the user expressly requires stopping at that point, a repair needs unavailable permission/environment, the same failure survives the scoped recovery, or the task exceeds its ticket. Do not override a returned terminal verdict or silently weaken a mandatory reviewer check. After an authorized resume, retain the previous report and record the new attempt. Report delivery failures separately from ticket acceptance, and never bypass configured push hooks.
