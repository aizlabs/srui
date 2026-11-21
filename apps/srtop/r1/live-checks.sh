#!/usr/bin/env bash
# PX-026-G01: the installable-R1 live checks against the Linux devbox (D1 §§15, 18, 19.1, 25-27).
# Starts the box that apps/srtop/devbox/devbox.sh manages, records the exact environment it ran
# against, runs the native ProcessExplorerR1DevboxTests suite (compatible and incompatible
# profiles and core versions, unknown and changed host keys, an unauthorized key, reconnect,
# server replacement, no process-changing action, labels and keyboard, a command-line secret that
# reaches no log), audits every log it collected with privacy-audit.sh, and stops the box.
#
#   apps/srtop/r1/live-checks.sh                       start the box, run, stop the box
#   SRTOP_R1_KEEP_BOX=1 apps/srtop/r1/live-checks.sh   leave the box running afterwards
#   PX026G01_EVIDENCE_DIR=DIR apps/srtop/r1/live-checks.sh   keep the evidence in DIR
#   SRUI_R1_TEST_TIMEOUT=SECONDS                       the test run's own deadline (default 1200)
#
# The box's image must hold the srtop under test, exactly as r0-scenario.sh requires: an image
# without an org.srui.revision label, or built from a revision that differs from the one under
# test (SRTOP_DEVBOX_REV, default HEAD) in anything the image contains, is refused. The state
# directory is the box's own ($SRTOP_DEVBOX_STATE); ~/.ssh and the user's known_hosts are never
# read for trust or written.
set -euo pipefail

R1_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "$R1_DIR/../../.." && pwd)
DEVBOX_DIR=$REPO_ROOT/apps/srtop/devbox
STATE_DIR=${SRTOP_DEVBOX_STATE:-$HOME/.codex/srtop-devbox}
NAME=${SRTOP_DEVBOX_NAME:-srtop-devbox}
IMAGE=${SRTOP_DEVBOX_IMAGE:-srtop-devbox:latest}
PORT=${SRTOP_DEVBOX_PORT:-2222}
REV=${SRTOP_DEVBOX_REV:-HEAD}
TIMEOUT=${SRUI_R1_TEST_TIMEOUT:-1200}
SUITE=ProcessExplorerR1DevboxTests
# One evidence line per test in the suite; a run with fewer did not run every check. One of them,
# theServerRefusesAnIncompatibleClientWithItsReason, needs a server that sends SERVER
# HANDSHAKE_REFUSED with its reason (PX-026-G02) and fails against one that does not.
EXPECTED_TESTS=7
IMAGE_INPUTS=(
  apps/srtop/Cargo.toml apps/srtop/Cargo.lock apps/srtop/src server-rust protocol
  apps/srtop/devbox/Dockerfile apps/srtop/devbox/entrypoint.sh apps/srtop/devbox/sshd_config
)

die() { echo "live-checks: $*" >&2; exit 1; }

started=""
test_pid=""
cleanup() {
  local status=$?
  set +e
  if [ -n "$test_pid" ] && kill -0 "$test_pid" 2>/dev/null; then
    kill -TERM -- "-$test_pid" 2>/dev/null
  fi
  if [ -n "$started" ] && [ -z "${SRTOP_R1_KEEP_BOX:-}" ]; then
    "$DEVBOX_DIR/devbox.sh" down
  fi
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

[ "$(uname -s)" = Darwin ] || die "the client is the macOS AppKit client; run this on a Mac"
[ "$(id -u)" -ne 0 ] || die "run as your own user, never as root"
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

evidence=${PX026G01_EVIDENCE_DIR:-}
if [ -z "$evidence" ]; then
  evidence=$(mktemp -d "${TMPDIR:-/tmp}/srui-r1-live.XXXXXX")
fi
mkdir -p "$evidence"
export PX026G01_EVIDENCE_DIR=$evidence

started=1
SRTOP_DEVBOX_REV=$REV "$DEVBOX_DIR/devbox.sh" up
label=$(docker image inspect -f '{{index .Config.Labels "org.srui.revision"}}' "$IMAGE")

# The exact environment these checks ran against. Versions only: no key, no known_hosts entry.
{
  echo "date_utc: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "revision_under_test: $revision"
  echo "image: $IMAGE id $(docker image inspect -f '{{.Id}}' "$IMAGE") label org.srui.revision=$label"
  echo "image_bases: $(grep -E '^FROM ' "$DEVBOX_DIR/Dockerfile" | tr '\n' ' ')"
  echo "box_kernel: $(docker exec "$NAME" uname -srm)"
  echo "box_os: $(docker exec "$NAME" sh -c '. /etc/os-release && echo "$PRETTY_NAME"') debian_version $(docker exec "$NAME" cat /etc/debian_version)"
  echo "box_packages: $(docker exec "$NAME" dpkg-query -W -f '${Package}=${Version} ' openssh-server procps)"
  echo "box_binaries: $(docker exec "$NAME" sha256sum /usr/local/bin/srtop /usr/local/bin/srui-ssh-bridge | awk '{printf "%s=%s ", $2, $1}')"
  echo "box_srtop_args: $(docker exec "$NAME" sh -c 'echo "$SRTOP_ARGS"')"
  echo "docker: $(docker version -f 'client {{.Client.Version}} server {{.Server.Version}} {{.Server.KernelVersion}}')"
  echo "macos: $(sw_vers -productVersion) ($(sw_vers -buildVersion)) $(uname -m)"
  echo "swift: $(swift --version 2>&1 | head -1)"
  echo "xcode: $(xcodebuild -version 2>/dev/null | tr '\n' ' ' || echo none)"
  echo "client_ssh: $(ssh -V 2>&1)"
} | sed 's/[[:space:]]*$//' > "$evidence/environment.txt"
echo "live-checks: environment"
cat "$evidence/environment.txt"

export SRTOP_R0_DEVBOX_PORT=$PORT
export SRTOP_R0_DEVBOX_KEY=$STATE_DIR/id_ed25519
export SRTOP_R0_DEVBOX_KNOWN_HOSTS=$STATE_DIR/known_hosts
export SRTOP_R0_DOCKER=$docker
export SRTOP_R0_DEVBOX_CONTAINER=$NAME

log=$evidence/live-checks-swift-test.log
# Its own process group, so the deadline below can end the run and everything it started, and
# only that. The output goes to a file, never a pipe a lingering child could hold open.
set -m
swift test --package-path "$REPO_ROOT/client-macos" --filter "$SUITE" >"$log" 2>&1 &
test_pid=$!
set +m
deadline=$((SECONDS + TIMEOUT))
while kill -0 "$test_pid" 2>/dev/null; do
  if [ "$SECONDS" -ge "$deadline" ]; then
    echo "live-checks: the test run passed its ${TIMEOUT}s deadline; stopping it" >&2
    kill -TERM -- "-$test_pid" 2>/dev/null || true
    break
  fi
  sleep 2
done
set +e
wait "$test_pid"
status=$?
set -e
test_pid=""

grep -E "^PX-026-G01 R1 live evidence:|Test run with|passed after|failed after|recorded an issue" "$log" || true
# Anchored: swift-testing echoes a failing suite's doc comment, which names the prefix too.
evidence_lines=$(grep -c '^PX-026-G01 R1 live evidence:' "$log" || true)
# A skipped or partial run must not read as a pass.
if [ "$evidence_lines" -ne "$EXPECTED_TESTS" ] \
  || ! grep -Eq "Test run with $EXPECTED_TESTS tests( in 1 suite)? passed" "$log"; then
  echo "live-checks: expected $EXPECTED_TESTS passing tests with one evidence line each; got $evidence_lines evidence lines" >&2
  [ "$status" -eq 0 ] && status=1
fi

# Every application log the run collected (the box's srtop and sshd output, the client's unified
# log), the saved-connections file the real client store wrote, and the environment record must
# hold no credential or private path. The test runner's own
# log is the developer's build output, with its source paths; it is kept, not audited.
audited=("$evidence/environment.txt" "$evidence/devbox-srtop.log" "$evidence/devbox-container.log"
         "$evidence/client-unified.log" "$evidence/saved-connections.json")
if ! "$R1_DIR/privacy-audit.sh" "${audited[@]}"; then
  echo "live-checks: the privacy audit found something in $evidence" >&2
  [ "$status" -eq 0 ] && status=1
fi

echo "live-checks: evidence in $evidence; status $status"
exit "$status"
