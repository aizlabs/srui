#!/usr/bin/env bash
#
# test_task22_persistence.sh
# End-to-end automated verification script for Task 22:
# Persistent sessiond across SSH disconnects and globally unique incarnation tokens (§17, §20.2).
#

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "=== Task 22 Verification: Persistent sessiond & Incarnation Tokens (§17, §20.2) ==="

echo "[1/4] Building Rust workspace and sessiond binaries..."
cargo build --manifest-path "$REPO_ROOT/server-rust/Cargo.toml" --workspace

echo "[2/4] Running Rust sessiond state & persistence test suite..."
cargo test --manifest-path "$REPO_ROOT/server-rust/Cargo.toml" -p srui-sessiond --test session_state_persistence_test

echo "[3/4] Building Swift client package..."
swift build --package-path "$REPO_ROOT/client-macos"

echo "[4/4] Running Swift persistence and SSH integration tests..."
swift test --package-path "$REPO_ROOT/client-macos" --filter SSHTransportPersistenceIntegrationTests

echo "=== Task 22 verification completed successfully! ==="
