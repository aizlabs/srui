#!/usr/bin/env bash
# PX-008, the R0 end-to-end scenario: from an empty client window, through a worker created in
# the Linux dev box, its row appearing and being sampled, to the worker's own exit and its row's
# removal — srtop --live-source behind the real srui-ssh-bridge subsystem, the unchanged generic
# macOS client. Re-runnable; the output of the native test is the evidence.
#
#   apps/srtop/devbox/r0-scenario.sh                  start the box if needed, run, stop the box
#   SRTOP_R0_KEEP_BOX=1 apps/srtop/devbox/r0-scenario.sh   leave the box running afterwards
#   PX008_EVIDENCE_DIR=DIR apps/srtop/devbox/r0-scenario.sh  also write the client window's image
#                                                     and the op/byte trace to DIR
#
# The box is the one devbox.sh manages (same SRTOP_DEVBOX_* variables); its image is built on
# first use and reused after that, so rebuild it with `devbox.sh build` after changing srtop.
set -euo pipefail

DEVBOX_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "$DEVBOX_DIR/../../.." && pwd)
STATE_DIR=${SRTOP_DEVBOX_STATE:-$HOME/.codex/srtop-devbox}
NAME=${SRTOP_DEVBOX_NAME:-srtop-devbox}
IMAGE=${SRTOP_DEVBOX_IMAGE:-srtop-devbox:latest}
PORT=${SRTOP_DEVBOX_PORT:-2222}

die() { echo "r0-scenario: $*" >&2; exit 1; }

[ "$(uname -s)" = Darwin ] || die "the client is the macOS AppKit client; run this on a Mac"
docker=$(command -v docker) || die "docker is required (Docker Desktop must be running)"
docker info >/dev/null 2>&1 || die "docker is not running"

"$DEVBOX_DIR/devbox.sh" up
if [ -z "${SRTOP_R0_KEEP_BOX:-}" ]; then
  trap '"$DEVBOX_DIR/devbox.sh" down' EXIT
fi
revision=$(docker image inspect -f '{{index .Config.Labels "org.srui.revision"}}' "$IMAGE" 2>/dev/null || true)
echo "r0-scenario: box $NAME from image $IMAGE, srtop revision ${revision:-unrecorded}"
echo "r0-scenario: $(docker exec "$NAME" uname -srm)"

# The bridge, the box's sshd and srtop are already running; the test needs only the client.
export SRTOP_R0_DEVBOX_PORT=$PORT
export SRTOP_R0_DEVBOX_KEY=$STATE_DIR/id_ed25519
export SRTOP_R0_DEVBOX_KNOWN_HOSTS=$STATE_DIR/known_hosts
export SRTOP_R0_DOCKER=$docker
export SRTOP_R0_DEVBOX_CONTAINER=$NAME

log=$(mktemp "${TMPDIR:-/tmp}/srtop-r0-scenario.XXXXXX")
set +e
swift test --package-path "$REPO_ROOT/client-macos" --filter ProcessExplorerDevboxScenarioTests 2>&1 | tee "$log"
status=${PIPESTATUS[0]}
set -e
# A skipped or vacuous run must not read as a pass.
if ! grep -q 'PX-008 devbox evidence' "$log" || ! grep -Eq 'Test run with 1 test .*passed' "$log"; then
  echo "r0-scenario: the scenario did not run to its evidence line" >&2
  [ "$status" -eq 0 ] && status=1
fi
rm -f "$log"
exit "$status"
