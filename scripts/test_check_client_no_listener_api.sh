#!/usr/bin/env bash
# Self-test for scripts/check-client-no-listener-api.sh: a clean tree (with near-miss names and a
# test-only listener) passes, and each planted listener or IPC API is refused and named.
set -uo pipefail

repo_root=$(git rev-parse --show-toplevel)
check="$repo_root/scripts/check-client-no-listener-api.sh"
scratch=$(mktemp -d "${TMPDIR:-/tmp}/srui-no-listener-test.XXXXXX")
trap 'rm -rf "$scratch"' EXIT
failures=0

fail() {
    echo "FAIL: $*" >&2
    failures=$((failures + 1))
}

make_clean_tree() {
    rm -rf "$scratch/client"
    mkdir -p "$scratch/client/Session" "$scratch/client/Tests/SRUITests" "$scratch/client/.build/x"
    cat > "$scratch/client/Session/Clean.swift" <<'EOF'
import Foundation
import NetworkExtensionShim
struct Clean {
    var binding = 0
    var listenerCount = 0
    func rebind(_ value: Int) {}
    func unbind() { rebind(binding) }
    func listenerDescription() -> String { "no listener" }
    let socketPath = "/tmp/x.sock" // connect(2) to a server is outbound
}
EOF
    # Tests and build output are out of scope: the PX-026 lsof positive control lives in a test.
    printf 'let s = socket(AF_INET, SOCK_STREAM, 0)\n_ = listen(s, 1)\n' > "$scratch/client/Tests/SRUITests/Control.swift"
    printf 'import Network\nlet l = try NWListener(using: .tcp)\n' > "$scratch/client/.build/x/Generated.swift"
}

make_clean_tree
if ! out=$(bash "$check" "$scratch/client" 2>&1); then
    fail "a clean tree was refused: $out"
fi

plants=(
    'let listener = NSXPCListener.anonymous()'
    'let c = xpc_connection_create_mach_service("x", nil, 0)'
    'let l = try NWListener(using: .tcp)'
    'let port = CFMessagePortCreateLocal(nil, "x" as CFString, nil, nil, nil)'
    'let c = NSConnection.service(withName: "x", rootObject: self)'
    '_ = Darwin.bind(fd, address, length)'
    '_ = bind(fd, address, length)'
    'guard listen(fd, 16) == 0 else { return }'
    'DistributedNotificationCenter.default().addObserver(self, selector: s, name: n, object: nil)'
    'import Network'
    '@preconcurrency import Network'
    'import class Network.NWListener'
)
for plant in "${plants[@]}"; do
    make_clean_tree
    printf 'import Foundation\n%s\n' "$plant" > "$scratch/client/Session/Planted.swift"
    out=$(bash "$check" "$scratch/client" 2>&1)
    status=$?
    if [ "$status" -ne 1 ]; then
        fail "planted '$plant' exited $status, expected 1: $out"
    elif ! printf '%s\n' "$out" | grep -q 'Planted.swift:2:'; then
        fail "planted '$plant' was refused without naming its line: $out"
    fi
done

# A missing root is an error, never a pass.
if bash "$check" "$scratch/absent" >/dev/null 2>&1; then
    fail "a missing client root passed"
fi

if [ "$failures" -ne 0 ]; then
    echo "$failures failure(s)" >&2
    exit 1
fi
echo "check-client-no-listener-api.sh: clean tree passes; ${#plants[@]} planted listener/IPC APIs refused."
