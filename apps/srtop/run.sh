#!/usr/bin/env bash
# Launch the read-only Process Explorer (R0) in the unchanged generic macOS client.
#
#   apps/srtop/run.sh fake               the fixed three-row fake snapshot, on this Mac
#   apps/srtop/run.sh sequence [MS]      the scripted refresh sequence, every MS ms (default 1000)
#   apps/srtop/run.sh devbox             live Linux /proc in the local Docker dev box
#   apps/srtop/run.sh ssh HOST [PORT] [IDENTITY] [KNOWN_HOSTS]
#                                        a Linux host that already serves srtop as the srui subsystem
#
# Stop: quit the client (Cmd-Q, or close its window). `fake` and `sequence` then stop the srtop
# they started (SIGTERM, which removes its socket) and remove its private directory; Ctrl-C in
# this terminal does both. `devbox` leaves the box running: `apps/srtop/devbox/devbox.sh down`.
# On a real host, stop srtop there with Ctrl-C or SIGTERM.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
CLIENT="$ROOT/client-macos/.build/debug/RendererDemoApp"
SRTOP="$ROOT/apps/srtop/target/debug/srtop"

die() { echo "run.sh: $*" >&2; exit 1; }
usage() { sed -n '2,13p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "$1 is required ($2)"; }

[ "$(uname -s)" = Darwin ] || die "the client is the macOS AppKit client; run this on a Mac"
need swift "Xcode or the Swift toolchain, for the client"

build_client() {
  swift build --package-path "$ROOT/client-macos" --product RendererDemoApp
}

# What `fake` and `sequence` started, for `cleanup`.
server=""
dir=""

# Stops the srtop this script started and removes its private directory, whatever ended the
# script: the client quitting, Ctrl-C, srtop refusing its arguments or dying first. No step may
# abort the others, and the script still exits with the status that ended it.
cleanup() {
  local status=$?
  set +e
  if [ -n "$server" ]; then
    kill -TERM "$server" 2>/dev/null
    wait "$server" 2>/dev/null
  fi
  if [ -n "$dir" ]; then
    # A clean stop removes the socket itself; one that died abnormally leaves it behind.
    [ -S "$dir/srtop.sock" ] && rm -f "$dir/srtop.sock"
    rmdir "$dir" 2>/dev/null
  fi
  exit "$status"
}

# srtop and the client on this Mac, joined by a private Unix socket: no SSH is involved.
local_source() {
  need cargo "the Rust toolchain, for srtop"
  cargo build --locked --manifest-path "$ROOT/apps/srtop/Cargo.toml"
  build_client
  trap cleanup EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  # srtop refuses a socket directory other users can enter; mktemp -d makes it 0700. /tmp keeps
  # the socket path short of the 104-byte AF_UNIX limit wherever TMPDIR points.
  dir=$(mktemp -d /tmp/srtop-run.XXXXXX)
  local socket="$dir/srtop.sock"
  "$SRTOP" --socket "$socket" "$@" &
  server=$!
  for _ in $(seq 1 100); do
    [ -S "$socket" ] && break
    kill -0 "$server" 2>/dev/null || die "srtop exited before publishing its socket"
    sleep 0.05
  done
  [ -S "$socket" ] || die "srtop did not publish $socket"
  echo "run.sh: srtop $* (pid $server) on $socket; quit the client window to stop both"
  "$CLIENT" --socket "$socket"
}

case "${1:-}" in
  fake)
    local_source --fake-source
    ;;
  sequence)
    local_source --fake-sequence --refresh-interval-ms "${2:-1000}"
    ;;
  devbox)
    need docker "Docker Desktop, for the Linux dev box"
    "$ROOT/apps/srtop/devbox/devbox.sh" up
    exec "$ROOT/apps/srtop/devbox/devbox.sh" client
    ;;
  ssh)
    [ -n "${2:-}" ] || usage
    build_client
    args=(--ssh "$2" --subsystem srui)
    [ -n "${3:-}" ] && args+=(--port "$3")
    [ -n "${4:-}" ] && args+=(--identity "$4")
    [ -n "${5:-}" ] && args+=(--known-hosts "$5")
    exec "$CLIENT" "${args[@]}"
    ;;
  *)
    usage
    ;;
esac
