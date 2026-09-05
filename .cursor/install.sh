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
#    Ubuntu's `libstdc++-dev` is virtual with no install candidate, so resolve a
#    concrete `libstdc++-<n>-dev` from the package index rather than pinning a
#    version that only exists on one release. Best effort: fuzzing is optional
#    and `./scripts/run-fuzz.sh` reports the missing toolchain itself, so a
#    failure here must not block the rest of the bootstrap.
CXX_PROBE_BIN="$(mktemp -t srui-cxxcheck.XXXXXX)"
trap 'rm -f "$CXX_PROBE_BIN"' EXIT

cxx_probe() {
  echo '#include <cassert>
int main() { return 0; }' | "${CXX:-c++}" -std=c++17 -x c++ - -o "$CXX_PROBE_BIN"
}

# Only called from a context where `set -e` is suppressed; every step reports
# failure through an explicit `return 1`.
install_cxx_stdlib() {
  local sudo_cmd=()
  if [ "$(id -u)" -ne 0 ]; then
    if command -v sudo >/dev/null 2>&1 && sudo -n true >/dev/null 2>&1; then
      sudo_cmd=(sudo -n)
    else
      echo "warning: not root and no non-interactive sudo; skipping C++ stdlib install" >&2
      return 1
    fi
  fi
  if ! command -v apt-get >/dev/null 2>&1; then
    echo "warning: apt-get unavailable; skipping C++ stdlib install" >&2
    return 1
  fi

  "${sudo_cmd[@]+"${sudo_cmd[@]}"}" apt-get update -qq || return 1

  local pkgs=(build-essential)
  local stdcxx=""
  stdcxx="$(apt-cache pkgnames 'libstdc++-' 2>/dev/null \
    | grep -E '^libstdc\+\+-[0-9]+-dev$' | sort -t- -k2 -n | tail -1)" || true
  if [ -n "$stdcxx" ]; then
    pkgs+=("$stdcxx")
  fi

  # `sudo env VAR=...` rather than `sudo VAR=... cmd`: the latter is rejected
  # under the default env_reset sudoers policy without `setenv`.
  "${sudo_cmd[@]+"${sudo_cmd[@]}"}" env DEBIAN_FRONTEND=noninteractive \
    apt-get install -y --no-install-recommends "${pkgs[@]}" || return 1
}

if ! cxx_probe >/dev/null 2>&1; then
  install_cxx_stdlib || true
  if ! cxx_probe; then
    echo "warning: C++ stdlib unavailable; cargo-fuzz (./scripts/run-fuzz.sh) will not build" >&2
  fi
fi

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
