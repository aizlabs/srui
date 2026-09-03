# AGENTS.md

Notes for AI coding agents working in this repo. Build/test/architecture guidance lives in
`CLAUDE.md`; this file records tooling hazards that have cost real debugging time.

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
