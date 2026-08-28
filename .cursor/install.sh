#!/usr/bin/env bash
# Idempotent Cloud Agent bootstrap for the SRUI monorepo.
# Prepares the Linux-buildable components: the Python protocol tooling and the
# Rust server workspace + examples. The Swift client (client-macos) targets
# macOS/AppKit and is intentionally not built here.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# 1. Python tooling: install uv (idempotent) and expose it on PATH.
export PATH="$HOME/.local/bin:$PATH"
if ! command -v uv >/dev/null 2>&1; then
  curl -LsSf https://astral.sh/uv/install.sh | sh
  export PATH="$HOME/.local/bin:$PATH"
fi

# 2. Rust toolchain: the server workspace pulls in dependencies that require the
#    2024 edition (stabilized in Rust 1.85), newer than some base images ship.
rustup update stable
rustup default stable

# 3. Python protocol validator + test dependencies (creates .venv).
uv sync --extra dev

# 4. Warm the build caches for the Rust server workspace and the counter example
#    so the first agent command is fast.
cargo build --manifest-path server-rust/Cargo.toml --workspace --all-targets
cargo build --manifest-path examples/counter/Cargo.toml

echo "SRUI environment bootstrap complete."
