#!/usr/bin/env bash
# ==============================================================================
# SRUI Protocol Buffers & Registry Code Generation Script
# Compiles protocol/srui.proto for Swift (client-macos/Protocol/srui.pb.swift)
# and generates standard registry tables (client-macos/SemanticModel/RegistryTables.swift).
# ==============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROTO_FILE="${REPO_ROOT}/protocol/srui.proto"
SWIFT_OUT="${REPO_ROOT}/client-macos/Protocol"

echo "=== Generating SRUI Protocol Buffers ==="

# 1. Locate protoc
PROTOC_BIN=""
if [[ -n "${PROTOC:-}" && -x "${PROTOC}" ]]; then
    PROTOC_BIN="${PROTOC}"
elif command -v protoc >/dev/null 2>&1 && protoc --version >/dev/null 2>&1; then
    PROTOC_BIN="$(command -v protoc)"
else
    # Search cargo cache for a working protoc binary compatible with this host
    CARGO_DIR="${CARGO_HOME:-${HOME}/.cargo}"
    while IFS= read -r candidate; do
        if [[ -x "${candidate}" ]] && "${candidate}" --version >/dev/null 2>&1; then
            PROTOC_BIN="${candidate}"
            break
        fi
    done < <(find "${CARGO_DIR}" "${REPO_ROOT}/server-rust/target" -name "protoc" -type f -perm +111 2>/dev/null || true)

    if [[ -z "${PROTOC_BIN}" ]]; then
        echo "Error: protoc binary not found. Please install protobuf or build server-rust." >&2
        exit 1
    fi
fi
echo "Using protoc: ${PROTOC_BIN} ($("${PROTOC_BIN}" --version))"

# 2. Locate protoc-gen-swift
PLUGIN_BIN=""
if [[ -n "${PROTOC_GEN_SWIFT:-}" && -x "${PROTOC_GEN_SWIFT}" ]]; then
    PLUGIN_BIN="${PROTOC_GEN_SWIFT}"
elif command -v protoc-gen-swift >/dev/null 2>&1; then
    PLUGIN_BIN="$(command -v protoc-gen-swift)"
else
    # Look for existing build of protoc-gen-swift in client-macos/.build
    while IFS= read -r candidate; do
        if [[ -x "${candidate}" && "${candidate}" != *".dSYM"* ]]; then
            PLUGIN_BIN="${candidate}"
            break
        fi
    done < <(find "${REPO_ROOT}/client-macos/.build" -name "protoc-gen-swift" -type f -perm +111 2>/dev/null || true)

    if [[ -z "${PLUGIN_BIN}" ]]; then
        CHECKOUT_DIR="${REPO_ROOT}/client-macos/.build/checkouts/swift-protobuf"
        if [[ -d "${CHECKOUT_DIR}" ]]; then
            echo "Building protoc-gen-swift from client-macos dependencies..."
            swift build --package-path "${CHECKOUT_DIR}" --product protoc-gen-swift -c release
            SWIFT_BIN_DIR="$(swift build --package-path "${CHECKOUT_DIR}" --product protoc-gen-swift -c release --show-bin-path 2>/dev/null || true)"
            if [[ -n "${SWIFT_BIN_DIR}" && -x "${SWIFT_BIN_DIR}/protoc-gen-swift" ]]; then
                PLUGIN_BIN="${SWIFT_BIN_DIR}/protoc-gen-swift"
            fi
        fi
    fi

    if [[ -z "${PLUGIN_BIN}" ]]; then
        echo "Error: protoc-gen-swift plugin not found. Please run 'swift build' in client-macos first." >&2
        exit 1
    fi
fi
echo "Using protoc-gen-swift: ${PLUGIN_BIN}"

# 3. Generate Swift Protobuf Code
mkdir -p "${SWIFT_OUT}"
"${PROTOC_BIN}" \
    --plugin="protoc-gen-swift=${PLUGIN_BIN}" \
    --swift_out="${SWIFT_OUT}" \
    --swift_opt=Visibility=Public \
    -I "${REPO_ROOT}/protocol" \
    "${PROTO_FILE}"

echo "Generated Swift protobuf code -> ${SWIFT_OUT}/srui.pb.swift"
echo "Rust code generation is handled automatically at build time via server-rust/protocol/build.rs (prost-build)."

# 4. Generate Swift Registry Tables
echo "=== Generating SRUI Registry Tables ==="
if command -v uv >/dev/null 2>&1; then
    uv run python "${REPO_ROOT}/protocol/generate_swift_registry.py"
elif command -v python3 >/dev/null 2>&1; then
    python3 "${REPO_ROOT}/protocol/generate_swift_registry.py"
else
    echo "Warning: Python 3 not found to regenerate Swift registry tables."
fi

echo "=== SRUI Codegen Complete ==="
