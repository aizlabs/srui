#!/usr/bin/env bash
# PX-008, the R0 end-to-end scenario: from an empty client window, through a worker created in
# the Linux dev box, its row appearing and being sampled, to the worker's own exit and its row's
# removal — srtop --live-source behind the real srui-ssh-bridge subsystem, the unchanged generic
# macOS client. Re-runnable; the output of the native test is the evidence.
#
#   apps/srtop/devbox/r0-scenario.sh                  start the box, run, stop the box
#   SRTOP_R0_KEEP_BOX=1 apps/srtop/devbox/r0-scenario.sh   leave the box running afterwards
#   PX008_EVIDENCE_DIR=DIR apps/srtop/devbox/r0-scenario.sh  also write the client window's image
#                                                     and the op/byte trace to DIR
#
# The box is the one devbox.sh manages (same SRTOP_DEVBOX_* variables). Its image must hold the
# srtop under test: the scenario refuses an image without an org.srui.revision label, or one
# whose label differs from the revision under test (SRTOP_DEVBOX_REV, default HEAD) in anything
# that goes into the image, and names the `devbox.sh build` that fixes it. A missing image is
# built from that revision.
set -euo pipefail

DEVBOX_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "$DEVBOX_DIR/../../.." && pwd)
STATE_DIR=${SRTOP_DEVBOX_STATE:-$HOME/.codex/srtop-devbox}
NAME=${SRTOP_DEVBOX_NAME:-srtop-devbox}
IMAGE=${SRTOP_DEVBOX_IMAGE:-srtop-devbox:latest}
PORT=${SRTOP_DEVBOX_PORT:-2222}
REV=${SRTOP_DEVBOX_REV:-HEAD}
# What `devbox.sh build` puts into the image: srtop and the runtime from the exported revision,
# and the image files from this directory.
IMAGE_INPUTS=(
  apps/srtop/Cargo.toml apps/srtop/Cargo.lock apps/srtop/src server-rust protocol
  apps/srtop/devbox/Dockerfile apps/srtop/devbox/entrypoint.sh apps/srtop/devbox/sshd_config
)

die() { echo "r0-scenario: $*" >&2; exit 1; }

started=""
log=""
# Stops the box this run started and removes the run's temporary log, whatever ends the run: a
# failed `up`, a failed scenario or an interrupt. No step may abort the others, and the run
# still exits with the status that ended it.
cleanup() {
  local status=$?
  set +e
  [ -n "$log" ] && rm -f "$log"
  if [ -n "$started" ] && [ -z "${SRTOP_R0_KEEP_BOX:-}" ]; then
    "$DEVBOX_DIR/devbox.sh" down
  fi
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

[ "$(uname -s)" = Darwin ] || die "the client is the macOS AppKit client; run this on a Mac"
docker=$(command -v docker) || die "docker is required (Docker Desktop must be running)"
docker info >/dev/null 2>&1 || die "docker is not running"

revision=$(git -C "$REPO_ROOT" rev-parse --verify --quiet "$REV^{commit}") || die "no such revision: $REV"
rebuild="SRTOP_DEVBOX_REV=$REV apps/srtop/devbox/devbox.sh build"
git -C "$REPO_ROOT" diff --quiet "$revision" -- "${IMAGE_INPUTS[@]}" \
  || die "the working tree changes what goes into the image since $REV; commit it, then: $rebuild"
if docker image inspect "$IMAGE" >/dev/null 2>&1; then
  label=$(docker image inspect -f '{{index .Config.Labels "org.srui.revision"}}' "$IMAGE" 2>/dev/null || true)
  [ -n "$label" ] || die "$IMAGE has no org.srui.revision label; rebuild it: $rebuild"
  git -C "$REPO_ROOT" cat-file -e "$label^{commit}" 2>/dev/null \
    || die "$IMAGE was built from $label, which this clone does not have; rebuild it: $rebuild"
  git -C "$REPO_ROOT" diff --quiet "$label" "$revision" -- "${IMAGE_INPUTS[@]}" \
    || die "$IMAGE was built from $label, which differs from $revision in what goes into the image; rebuild it: $rebuild"
fi

# From here on the box is this run's: a failed `up` must not leave a container behind.
started=1
SRTOP_DEVBOX_REV=$REV "$DEVBOX_DIR/devbox.sh" up
label=$(docker image inspect -f '{{index .Config.Labels "org.srui.revision"}}' "$IMAGE")
echo "r0-scenario: box $NAME from image $IMAGE, srtop revision $label (under test: $revision)"
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
exit "$status"
