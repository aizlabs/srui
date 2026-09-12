#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test --locked --manifest-path apps/srtop/Cargo.toml
cargo build --locked --manifest-path apps/srtop/Cargo.toml
cargo build --locked --manifest-path server-rust/Cargo.toml -p srui-ssh-bridge
swift test --package-path client-macos --filter ProcessExplorerShellTests
