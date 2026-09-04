#!/usr/bin/env bash
#
# test_task28_collection_ranges.sh
# Builds the Rust large-collection fixture, then runs the Swift live range test.
# A missing counter binary must fail this script rather than skip the Swift test.
#

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COUNTER_BIN="$REPO_ROOT/examples/counter/target/debug/counter"

echo "=== Task 28 Integration Test: sparse collection ranges ==="

echo "Building Rust counter fixture..."
cargo build --manifest-path "$REPO_ROOT/examples/counter/Cargo.toml"

if [[ ! -x "$COUNTER_BIN" ]]; then
    echo "error: expected $COUNTER_BIN after cargo build" >&2
    exit 1
fi

echo "Building Swift client..."
swift build --package-path "$REPO_ROOT/client-macos"

echo "Executing collection range Unix-socket integration test..."
swift test \
    --package-path "$REPO_ROOT/client-macos" \
    --filter CollectionRangeSocketIntegrationTests

echo "=== Task 28 verification completed successfully! ==="
