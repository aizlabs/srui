#!/usr/bin/env bash
#
# test_task18_counter_socket.sh
# End-to-end automated socket integration test for Task 18 (§20.2, §22, §29).
# Builds the Rust counter server, builds the Swift client, and runs the real
# Unix-socket integration test against a live counter process.
#

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "=== Task 18 Integration Test: Swift Client + Rust Sessiond Socket Bridge ==="

# 1. Build Rust counter server
echo "Building Rust counter server..."
cargo build --manifest-path "$REPO_ROOT/examples/counter/Cargo.toml"

# 2. Build Swift client
echo "Building Swift client..."
swift build --package-path "$REPO_ROOT/client-macos"

# 3. Run the real Unix-socket integration test (spawns counter server internally)
echo "Executing Unix socket integration test..."
swift test \
    --package-path "$REPO_ROOT/client-macos" \
    --filter CounterSocketIntegrationTests

echo "=== Task 18 verification completed successfully! ==="
