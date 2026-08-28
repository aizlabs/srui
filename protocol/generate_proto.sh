#!/usr/bin/env bash
# ==============================================================================
# SRUI Protocol Buffers Code Generation Script
# Compiles protocol/srui.proto for Swift (client-macos/Protocol/srui.pb.swift)
# and validates Rust prost compilation.
# ==============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROTO_FILE="${REPO_ROOT}/protocol/srui.proto"
SWIFT_OUT="${REPO_ROOT}/client-macos/Protocol"

echo "=== Generating SRUI Protocol Buffers ==="

# 1. Locate protoc
if [[ -n "${PROTOC:-}" && -x "${PROTOC}" ]]; then
    PROTOC_BIN="${PROTOC}"
elif command -v protoc >/dev/null 2>&1; then
    PROTOC_BIN="$(command -v protoc)"
else
    # Find cargo-vendored protoc
    CARGO_PROTOC=$(find "${HOME}/.cargo" -name "protoc" -perm +111 2>/dev/null | grep -E "macos.*protoc$" | head -n 1 || true)
    if [[ -n "${CARGO_PROTOC}" && -x "${CARGO_PROTOC}" ]]; then
        PROTOC_BIN="${CARGO_PROTOC}"
    else
        echo "Error: protoc binary not found. Please install protobuf or build server-rust." >&2
        exit 1
    fi
fi
echo "Using protoc: ${PROTOC_BIN} ($("${PROTOC_BIN}" --version))"

# 2. Locate protoc-gen-swift
if [[ -n "${PROTOC_GEN_SWIFT:-}" && -x "${PROTOC_GEN_SWIFT}" ]]; then
    PLUGIN_BIN="${PROTOC_GEN_SWIFT}"
elif command -v protoc-gen-swift >/dev/null 2>&1; then
    PLUGIN_BIN="$(command -v protoc-gen-swift)"
else
    # Look in SwiftPM build checkouts
    SWIFT_PLUGIN=$(find "${REPO_ROOT}/client-macos/.build" -name "protoc-gen-swift" -perm +111 -type f 2>/dev/null | grep -v "\.dSYM" | head -n 1 || true)
    if [[ -n "${SWIFT_PLUGIN}" && -x "${SWIFT_PLUGIN}" ]]; then
        PLUGIN_BIN="${SWIFT_PLUGIN}"
    else
        echo "Building protoc-gen-swift from client-macos dependencies..."
        swift build --package-path "${REPO_ROOT}/client-macos/.build/checkouts/swift-protobuf" --product protoc-gen-swift -c release
        ARCH="$(uname -m)"
        PLUGIN_BIN="${REPO_ROOT}/client-macos/.build/checkouts/swift-protobuf/.build/${ARCH}-apple-macosx/release/protoc-gen-swift"
    fi
fi
echo "Using protoc-gen-swift: ${PLUGIN_BIN}"

# 3. Generate Swift Code
mkdir -p "${SWIFT_OUT}"
"${PROTOC_BIN}" \
    --plugin="protoc-gen-swift=${PLUGIN_BIN}" \
    --swift_out="${SWIFT_OUT}" \
    --swift_opt=Visibility=Public \
    -I "${REPO_ROOT}/protocol" \
    "${PROTO_FILE}"

echo "Generated Swift protobuf code -> ${SWIFT_OUT}/srui.pb.swift"
echo "Rust code generation is handled automatically at build time via server-rust/protocol/build.rs (prost-build)."
echo "=== SRUI Codegen Complete ==="
