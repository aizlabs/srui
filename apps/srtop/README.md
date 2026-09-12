# Process Explorer

PX-001 provides a read-only empty shell: a heading, status, and a native table with PID and Name columns. Process collection and actions are not implemented.

Build and run from the repository root:

```sh
cargo build --locked --manifest-path apps/srtop/Cargo.toml
apps/srtop/target/debug/srtop --socket /tmp/srtop-dev/srtop.sock
```

The socket directory must be private to the current non-root user. An existing socket is never replaced. Configure the existing `srui-ssh-bridge` subsystem to point to that socket and connect with the unchanged generic macOS client.

App-specific deterministic checks:

```sh
cargo test --locked --manifest-path apps/srtop/Cargo.toml
bash apps/srtop/test.sh
```

The native entry point builds the app and SSH bridge, then runs `ProcessExplorerShellTests` through the existing Swift client test target over an ephemeral authenticated localhost SSH subsystem. Missing prerequisites fail; they are never silently skipped.

`--smoke-fixture` enables a local SIGUSR1 trigger for one deterministic title value. It updates the existing Surface label and heading through a normal atomic transaction. This is a test fixture, not a remote process action. The native test verifies retained window and table handles before and after that update.
