#!/usr/bin/env bash
# The client exposes no external automation or IPC endpoint (D1 §22.9; PX-026).
#
# §22.9 keeps semantic inspection and automation in-process: a future XPC surface "SHOULD be
# disabled or permission-gated by default until its authorization model is specified". This gate
# fails if any client product source (every Swift file under client-macos except tests and build
# output) uses an API that would let another process reach it: an XPC or Mach listener, a
# Network.framework listener, a CFMessagePort, distributed objects or distributed notifications,
# or a BSD socket bind/listen. Outbound connections (the SSH transport's pipes, connect(2) to a
# unix socket) are not endpoints and are not matched.
#
# Usage: scripts/check-client-no-listener-api.sh [client-root]   (default: client-macos)
# Pure grep, so it runs on every platform. `scripts/test_check_client_no_listener_api.sh` plants
# each pattern and checks that this script refuses it.
set -euo pipefail

if [ "$#" -gt 0 ]; then
    root=$1
else
    root="$(git rev-parse --show-toplevel)/client-macos"
fi
[ -d "$root" ] || { echo "error: no client root at $root" >&2; exit 2; }

# One extended regex per API family. Identifier boundaries use portable classes (BSD and GNU grep).
patterns=(
    'NSXPCListener'
    'xpc_connection_create'
    'NWListener'
    'CFMessagePort'
    '(^|[^[:alnum:]_])NSConnection([^[:alnum:]_]|$)'
    '(^|[^[:alnum:]_])bind[[:space:]]*\('
    '(^|[^[:alnum:]_])listen[[:space:]]*\('
    'DistributedNotificationCenter'
    '^[[:space:]]*(@[[:alnum:]_]+[[:space:]]+)*import[[:space:]]+([a-z]+[[:space:]]+)?Network([.[:space:]]|$)'
)

sources=()
while IFS= read -r -d '' file; do
    sources+=("$file")
done < <(find "$root" -name '*.swift' -not -path '*/.build/*' -not -path '*/Tests/*' -print0 | sort -z)

if [ "${#sources[@]}" -eq 0 ]; then
    echo "error: no client product Swift sources under $root" >&2
    exit 2
fi

found=0
for pattern in "${patterns[@]}"; do
    if grep -nHE -- "$pattern" "${sources[@]}"; then
        found=1
    fi
done

if [ "$found" -ne 0 ]; then
    echo "error: the client must not open a listener or IPC endpoint (D1 §22.9); see the lines above" >&2
    exit 1
fi
echo "No listener or IPC endpoint API in ${#sources[@]} client product Swift source(s) under $root."
