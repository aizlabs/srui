#!/usr/bin/env bash
#
# test_task19_counter_ssh.sh
# End-to-end automated SSH subsystem integration test for Task 19 (§19, §19.1, §20.1, §22, §29).
# Builds server-rust and client-macos, then runs all SSHTransport tests (posture, host-key, live).
#

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "=== Task 19 Verification: Real SSH Subsystem Transport (§19, §19.1, §20.1) ==="

echo "[1/3] Building Rust workspace and counter example..."
cargo build --manifest-path "$REPO_ROOT/server-rust/Cargo.toml" --workspace
cargo build --manifest-path "$REPO_ROOT/examples/counter/Cargo.toml"

echo "[2/3] Building Swift client..."
swift build --package-path "$REPO_ROOT/client-macos"

echo "[3/3] Running SSH transport tests (posture, host-key fail-closed, live integration)..."
swift test --package-path "$REPO_ROOT/client-macos" --filter SSHTransport

echo "=== Task 19 verification completed successfully! ==="
