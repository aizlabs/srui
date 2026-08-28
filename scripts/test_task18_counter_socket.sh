#!/usr/bin/env bash
#
# test_task18_counter_socket.sh
# End-to-end automated socket integration test for Task 18 (§20.2, §22, §29).
# Launches the Rust counter server on a Unix socket, connects the Swift client,
# and verifies 3+ consecutive click-and-observe cycles over the socket.
#

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SOCKET_PATH="/tmp/test-srui-counter-$$.sock"
SERVER_PID=""

cleanup() {
    if [ -n "$SERVER_PID" ]; then
        echo "Cleaning up counter server process (PID $SERVER_PID)..."
        kill -TERM "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
    rm -f "$SOCKET_PATH"
}
trap cleanup EXIT INT TERM

echo "=== Task 18 Integration Test: Swift Client + Rust Sessiond Socket Bridge ==="

# 1. Build Rust counter server
echo "Building Rust counter server..."
cargo build --manifest-path "$REPO_ROOT/examples/counter/Cargo.toml"

# 2. Build Swift client
echo "Building Swift client..."
swift build --package-path "$REPO_ROOT/client-macos"

# 3. Launch Rust counter server listening on local Unix domain socket
echo "Starting counter server on socket $SOCKET_PATH..."
cargo run --manifest-path "$REPO_ROOT/examples/counter/Cargo.toml" -- --socket "$SOCKET_PATH" &
SERVER_PID=$!

# Wait for socket to become available
for i in $(seq 1 50); do
    if [ -S "$SOCKET_PATH" ]; then
        echo "Socket $SOCKET_PATH is ready."
        break
    fi
    sleep 0.1
done

if [ ! -S "$SOCKET_PATH" ]; then
    echo "Error: Socket $SOCKET_PATH failed to initialize within timeout."
    exit 1
fi

# 4. Run Swift test suite including live integration tests
echo "Executing Swift integration test suite..."
swift test --package-path "$REPO_ROOT/client-macos"

echo "=== Task 18 verification completed successfully! ==="
