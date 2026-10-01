#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
# This runs `swift test` directly, so nothing reaps the srtop/sessiond fixtures a wedged or
# interrupted run leaves behind. Sweep the previous run's orphans first; never fail the suite for it.
scripts/reap-test-servers.sh || echo "note: pre-test sweep failed; continuing" >&2
cargo test --locked --manifest-path apps/srtop/Cargo.toml
cargo build --locked --manifest-path apps/srtop/Cargo.toml
cargo build --locked --manifest-path server-rust/Cargo.toml -p srui-ssh-bridge
swift test --package-path client-macos --filter ProcessExplorerShellTests
