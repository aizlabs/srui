#!/usr/bin/env bash
set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
trace_path=${1:-/tmp/srui-allocations.trace}
result_path=${2:-/tmp/srui-render-profile.json}

swift build --package-path "$repo_root/client-macos" -c release --product BenchmarkDriver
binary="$repo_root/client-macos/.build/release/BenchmarkDriver"

uv run --frozen python "$repo_root/benchmarks/parse-render/run_xctrace.py" \
    "$trace_path" \
    "$binary" \
    "$repo_root/benchmarks/fixtures/coding-agent-ui.json" \
    "$result_path"

echo "$trace_path"
echo "${trace_path}.summary.json"
echo "$result_path"
