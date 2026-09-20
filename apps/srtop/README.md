# Process Explorer

PX-002 adds an optional deterministic fake snapshot to the read-only shell: a heading, status, and native PID/Name table. Live process collection and actions are not implemented.

Build and run from the repository root:

```sh
cargo build --locked --manifest-path apps/srtop/Cargo.toml
apps/srtop/target/debug/srtop --socket /tmp/srtop-dev/srtop.sock --fake-source
```

The fake snapshot contains three rows in order: `4101 / worker`, `4102 / worker`, and `Unavailable / helper`. Duplicate names have distinct opaque item IDs, and missing data is explicit. Omit `--fake-source` to retain the empty-shell fixture. Source records, source identity and sample time are typed separately from UI projection; fake mode performs no live process enumeration.

The socket directory must be private to the current non-root user. An existing socket is never replaced. Configure the existing `srui-ssh-bridge` subsystem to point to that socket and connect with the unchanged generic macOS client.

App-specific deterministic checks:

```sh
cargo test --locked --manifest-path apps/srtop/Cargo.toml
bash apps/srtop/test.sh
```

The native entry point builds the app and SSH bridge, then runs `ProcessExplorerShellTests` through the existing Swift client test target over an ephemeral authenticated localhost SSH subsystem, checking both empty and fake-source cases, actual native cell values and retained handles. Missing prerequisites fail; they are never silently skipped.

`--smoke-fixture` enables a local SIGUSR1 trigger for one deterministic title value. It updates the existing Surface label and heading through a normal atomic transaction. This is a test fixture, not a remote process action. The native test verifies retained window and table handles before and after that update.
