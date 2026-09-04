#!/usr/bin/env bash
# Idempotent Cloud Agent bootstrap for the SRUI monorepo.
# Prepares the Linux-buildable components: the Python protocol tooling and the
# Rust server workspace + examples. The Swift client (client-macos) targets
# macOS/AppKit and is intentionally not built here.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# 1. C++ compiler + libstdc++ headers for cargo-fuzz / libFuzzer.
#    SRUI has no C++ sources. `libfuzzer-sys` compiles LLVM's C++ fuzzer runtime,
#    and `./scripts/run-fuzz.sh` fails at `#include <cassert>` without these.
#    Ubuntu 24.04's `libstdc++-dev` is virtual with no install candidate. The
#    image's clang (which cargo-fuzz invokes as `c++`) searches gcc 14 include
#    paths, so install `build-essential` plus `libstdc++-14-dev`.
cxx_probe() {
  echo '#include <cassert>
int main() { return 0; }' | "${CXX:-c++}" -std=c++17 -x c++ - -o /tmp/srui-cxxcheck
}

if ! cxx_probe >/dev/null 2>&1; then
  sudo apt-get update -qq
  sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    build-essential libstdc++-14-dev
fi
if ! cxx_probe; then
  echo "error: C++ stdlib is still missing after apt-get (needed by cargo-fuzz)" >&2
  exit 1
fi
rm -f /tmp/srui-cxxcheck

# 2. Python tooling: install uv (idempotent) and expose it on PATH.
export PATH="$HOME/.local/bin:$PATH"
if ! command -v uv >/dev/null 2>&1; then
  curl -LsSf https://astral.sh/uv/install.sh | sh
  export PATH="$HOME/.local/bin:$PATH"
fi

# 3. Rust toolchain: the server workspace pulls in dependencies that require the
#    2024 edition (stabilized in Rust 1.85), newer than some base images ship.
rustup update stable
rustup default stable

# 4. Python protocol validator + test dependencies (creates .venv).
uv sync --extra dev

# 5. Warm the build caches for the Rust server workspace and the counter example
#    so the first agent command is fast.
cargo build --manifest-path server-rust/Cargo.toml --workspace --all-targets
cargo build --manifest-path examples/counter/Cargo.toml

echo "SRUI environment bootstrap complete."
