# Process Explorer

PX-003 adds one real read-only Linux process snapshot to the shell, alongside PX-002's deterministic fake snapshot: a heading, status, and native PID/Name table. Periodic refresh and process actions are not implemented.

Build and run from the repository root:

```sh
cargo build --locked --manifest-path apps/srtop/Cargo.toml
apps/srtop/target/debug/srtop --socket /tmp/srtop-dev/srtop.sock --fake-source
apps/srtop/target/debug/srtop --socket /tmp/srtop-dev/srtop.sock --live-source   # Linux
```

The fake snapshot contains three rows in order: `4101 / worker`, `4102 / worker`, and `Unavailable / helper`. Duplicate names have distinct opaque item IDs, and missing data is explicit. Omit both flags to retain the empty-shell fixture; the two source flags are mutually exclusive. Source records, source identity and sample time are typed separately from UI projection; fake mode performs no live process enumeration.

`--live-source` reads `/proc` once through `std` alone (no new dependency) and publishes PID and process name. Each process gets a `ProcessKey` of source, host, boot and PID-namespace identity, the PID, and a creation token taken from `/proc/<pid>/stat` field 22 in kernel clock ticks since boot — not a rounded start-time second. The key resolves to an opaque session item ID, so a reused PID with a different creation token is a different row identity. The PID namespace comes from the scanned mount's own namespace init, `<root>/1/ns/pid`, whenever the scan may read it. An unprivileged scan usually may not, and then falls back to `self/ns/pid` — the reader's own namespace — only under both proofs that it describes these records: `<root>/self` names this process's own PID, and `<root>/self` and `/proc/self` are the same device and inode, since one procfs superblock belongs to exactly one PID namespace. A host `/proc` bind-mounted into a container fails the second proof, a nested namespace that inherited an outer `/proc` fails the first, and either way the namespace is reported unavailable rather than stamped with the reader's. A snapshot is either complete, where an empty list authoritatively means "no processes visible", or incomplete, where individually denied or unparsable records are skipped with a reason and an empty list never means "nothing is running". A process that exits between listing `/proc` and reading it was not unreadable — it no longer exists at sample time — so it is counted separately and does not degrade the scan; ordinary churn on a busy host therefore never leaves the shell labelled incomplete. The status line states what actually fell short: how many records were unreadable when some were, that the process list itself was unavailable and why when the root could not be listed at all, and that host identity is incomplete when only the identity files were missing, and how many entries were beyond the collector's own record limit when the scan stopped at that bound rather than at a read failure — it never publishes a count of zero unreadable, and never describes a record it declined to read as unreadable. Process names are sanitized once on the server: invalid UTF-8 plus every control (Cc), format (Cf, including bidi overrides and zero-width joiners), line/paragraph-separator and otherwise invisible character becomes U+FFFD and the result is length-bounded, while spaces and parentheses stay literal text that is never interpolated into a command. A `stat` past the 64 KiB read bound is skipped with a reason rather than parsed from the bytes that fit, since a cut line still parses and would mint a wrong creation token. The status a source publishes states what it read and from where: only `--live-source` describes its data as live, and a scoped root claims neither liveness nor synthesis, because a path cannot tell a fixture tree from a bind mount of the host's real `/proc`. On a non-Linux host `--live-source` reports an incomplete scan rather than an empty one.

The socket directory must be private to the current non-root user. An existing socket is never replaced. Configure the existing `srui-ssh-bridge` subsystem to point to that socket and connect with the unchanged generic macOS client.

App-specific deterministic checks:

```sh
cargo test --locked --manifest-path apps/srtop/Cargo.toml
bash apps/srtop/test.sh
```

The native entry point builds the app and SSH bridge, then runs `ProcessExplorerShellTests` through the existing Swift client test target over an ephemeral authenticated localhost SSH subsystem, checking both empty and fake-source cases, actual native cell values and retained handles. Missing prerequisites fail; they are never silently skipped.

`--smoke-fixture` enables a local SIGUSR1 trigger for one deterministic title value. It updates the existing Surface label and heading through a normal atomic transaction. This is a test fixture, not a remote process action. The native test verifies retained window and table handles before and after that update.
