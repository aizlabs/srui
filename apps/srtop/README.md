# Process Explorer

PX-003 adds one real read-only Linux process snapshot to the shell, alongside PX-002's deterministic fake snapshot: a heading, status, and native PID/Name table. Periodic refresh and process actions are not implemented.

Build and run from the repository root:

```sh
cargo build --locked --manifest-path apps/srtop/Cargo.toml
apps/srtop/target/debug/srtop --socket /tmp/srtop-dev/srtop.sock --fake-source
apps/srtop/target/debug/srtop --socket /tmp/srtop-dev/srtop.sock --live-source   # Linux
```

The fake snapshot contains three rows in order: `4101 / worker`, `4102 / worker`, and `Unavailable / helper`. Duplicate names have distinct opaque item IDs, and missing data is explicit. Omit both flags to retain the empty-shell fixture; the two source flags are mutually exclusive. Source records, source identity and sample time are typed separately from UI projection; fake mode performs no live process enumeration.

`--live-source` reads `/proc` once through `std` alone (no new dependency) and publishes PID and process name. Each process gets a `ProcessKey` of source, host, boot and PID-namespace identity, the PID, and a creation token taken from `/proc/<pid>/stat` field 22 in kernel clock ticks since boot — not a rounded start-time second. The key resolves to an opaque session item ID, so a reused PID with a different creation token is a different row identity. A snapshot is either complete, where an empty list authoritatively means "no processes visible", or incomplete, where individually denied, vanished or unparsable records are skipped with a reason, the status line says how many were unreadable, and an empty list never means "nothing is running". Process names are sanitized once on the server: invalid UTF-8, control, bidi and zero-width characters become U+FFFD and the result is length-bounded, while spaces and parentheses stay literal text that is never interpolated into a command. On a non-Linux host `--live-source` reports an incomplete scan rather than an empty one.

The socket directory must be private to the current non-root user. An existing socket is never replaced. Configure the existing `srui-ssh-bridge` subsystem to point to that socket and connect with the unchanged generic macOS client.

App-specific deterministic checks:

```sh
cargo test --locked --manifest-path apps/srtop/Cargo.toml
bash apps/srtop/test.sh
```

The native entry point builds the app and SSH bridge, then runs `ProcessExplorerShellTests` through the existing Swift client test target over an ephemeral authenticated localhost SSH subsystem, checking both empty and fake-source cases, actual native cell values and retained handles. Missing prerequisites fail; they are never silently skipped.

`--smoke-fixture` enables a local SIGUSR1 trigger for one deterministic title value. It updates the existing Surface label and heading through a normal atomic transaction. This is a test fixture, not a remote process action. The native test verifies retained window and table handles before and after that update.
