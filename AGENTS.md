# AGENTS.md

Notes for AI coding agents working in this repo. Build/test/architecture guidance lives in
`CLAUDE.md`; this file records tooling hazards that have cost real debugging time.

## Branch and worktree discipline

Before any repository file write, check `git status --short --branch`, `git branch --show-current`, and `git worktree list`. This applies to code, documentation, instructions, generated files, formatting, and test artifacts.

- Never change any file in a checkout on `main`; never stage or commit directly on `main`.
- Create a new task branch, normally `codex/<task>`, in a dedicated worktree based on the intended `origin/main` revision before making changes. Continue there for edits, generation, builds, and tests.
- Do not switch branches in the shared main checkout or reuse another task's worktree without explicit authorization. Preserve unrelated changes and other worktrees.
- Commit and push the task branch, then open a pull request. Main changes only through merged pull requests; do not merge locally into main or push directly to main.
- If this task already has edits in the main checkout, first preserve them in the task worktree and verify the transfer. Restore only this task's known edits when authorized; never discard someone else's work.

## Benchmark documentation map

Before changing or interpreting the §31 benchmark suite, read the documents at the appropriate
level:

- `benchmarks/README.md` defines the suite-wide execution, validation, and reporting contract.
- `benchmarks/parse-render/README.md` is the operational guide for the live §31.1 macOS renderer
  benchmark.
- `benchmarks/parse-render/INSTRUMENTATION_FINDINGS.md` records the detailed evidence, rejected
  approaches (including the removed xctrace prototype), permission behavior, measurement
  semantics, reproduction commands, and Linux/Windows portability notes. Read it before changing
  paint, allocation, footprint, WindowServer, ScreenCaptureKit, or profiler instrumentation.

The findings document is explanatory, not authoritative: the design document remains the source
of product requirements, while executable benchmark schemas and assertions define the checked
measurement contract.

## Editing tools may preserve mtime — touch before trusting a red test

**Symptom.** You edit a file, re-run the test, and the result is unchanged: a fix stays red, or a
test that should now fail stays green. Reverting and re-running reproduces the *same* stale result.

**Cause.** Some agent editing tools (observed with the LemonCrow `edit` MCP tool, 2026-09) restore
the file's original `mtime` after writing. The kernel bumps `mtime` on `write(2)`, but the tool then
calls `utimensat()` to set it back. Only `ctime` — which userspace cannot set — reflects the real
edit.

Probe that demonstrates it:

```
before:  size 9   mtime 2020-01-01 00:00:00
after:   size 13  mtime 2020-01-01 00:00:00   ctime 2026-09-03 02:41:28
```

**Why it matters here.** Both of this repo's compiled toolchains decide freshness by comparing
`mtime` against build artifacts:

- `cargo` / `rustc` fingerprints under `server-rust/target/`
- SwiftPM / llbuild under `client-macos/.build/`

A frozen `mtime` means the compiler concludes the source is older than the artifact and skips the
rebuild. The test binary that runs is the *previous* one. This has produced at least two phantom
failures: a Rust coalescing test and a Swift state-machine conformance fixture, both green
immediately after touching.

**Mitigation.** After an edit batch, before believing any test outcome:

```bash
# from repo root
find . -name '*.rs'    -not -path './server-rust/target/*'  -exec touch {} +
find . -name '*.swift' -not -path './client-macos/.build/*' -exec touch {} +
```

To detect whether a tree is affected, compare the two timestamps — `ctime` newer than `mtime` on a
file you just edited is the tell:

```bash
stat -f 'mtime=%Sm ctime=%Sc %N' -t '%F %T' <file>   # macOS
stat -c 'mtime=%y ctime=%z %n' <file>                # Linux
```

**Status.** Believed to be unintended tool behavior; not yet confirmed with the LemonCrow
maintainers. Remove this note if the tool starts leaving `mtime` to the kernel.

## Cursor Cloud specific instructions

Cloud Agents run on Linux. Do not run `swift test --package-path client-macos`: that package
pulls AppKit via `RendererAppKit` and will not build here.

Linux-valid Swift checks (same as `.github/workflows/swift-linux.yml`):

```bash
bash scripts/parse-changed-swift.sh
bash scripts/check-logical-channel-scheduling-imports.sh
swift test --package-path client-macos/LogicalChannelScheduling
```

`parse-changed-swift.sh` is syntax-only (`swiftc -frontend -parse`). It does not type-check,
resolve modules, link, or validate Apple-framework APIs.

Socket writers, SSH/TCP/Unix transports, backpressure, and the rest of the client test graph
stay on the macOS `swift` job in `.github/workflows/ci.yml`.
