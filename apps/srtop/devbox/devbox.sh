#!/usr/bin/env bash
# Start and stop a local Linux box that runs srtop against a real /proc, reachable
# from the unmodified macOS client over a real OpenSSH subsystem.
#
#   apps/srtop/devbox/devbox.sh up        build the image if needed and start it
#   apps/srtop/devbox/devbox.sh client    build and launch the macOS client at it
#   apps/srtop/devbox/devbox.sh spawn 30  run `sleep 30` inside the box
#   apps/srtop/devbox/devbox.sh down      stop and remove the container
#
# State (keypair, known_hosts, build context) lives outside every checkout, in
# $SRTOP_DEVBOX_STATE. Nothing here is a test: it is an eyes-on harness.
set -euo pipefail

DEVBOX_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "$DEVBOX_DIR/../../.." && pwd)

STATE_DIR=${SRTOP_DEVBOX_STATE:-$HOME/.codex/srtop-devbox}
NAME=${SRTOP_DEVBOX_NAME:-srtop-devbox}
IMAGE=${SRTOP_DEVBOX_IMAGE:-srtop-devbox:latest}
PORT=${SRTOP_DEVBOX_PORT:-2222}
REV=${SRTOP_DEVBOX_REV:-HEAD}
# Pre-PX-004 revisions have no --refresh-interval-ms: use SRTOP_ARGS=--live-source.
SRTOP_ARGS=${SRTOP_ARGS:---live-source --refresh-interval-ms 1000}

KEY="$STATE_DIR/id_ed25519"
KNOWN_HOSTS="$STATE_DIR/known_hosts"
CTX="$STATE_DIR/ctx"

die() { echo "devbox: $*" >&2; exit 1; }

ensure_key() {
  mkdir -p "$STATE_DIR"
  chmod 700 "$STATE_DIR"
  [ -f "$KEY" ] || ssh-keygen -t ed25519 -N '' -C srtop-devbox -f "$KEY" >/dev/null
}

cmd_build() {
  ensure_key
  rm -rf "$CTX"
  mkdir -p "$CTX"
  # git archive, not the working tree: no .git, no target/, no keys in the image.
  git -C "$REPO_ROOT" archive "$REV" | tar -x -C "$CTX"
  mkdir -p "$CTX/devbox"
  cp "$DEVBOX_DIR/Dockerfile" "$DEVBOX_DIR/sshd_config" "$DEVBOX_DIR/entrypoint.sh" "$CTX/devbox/"
  cp "$DEVBOX_DIR/Dockerfile" "$CTX/Dockerfile"
  echo "devbox: building $IMAGE from $(git -C "$REPO_ROOT" rev-parse --short "$REV")"
  docker build -t "$IMAGE" "$CTX"
}

cmd_up() {
  ensure_key
  docker image inspect "$IMAGE" >/dev/null 2>&1 || cmd_build
  docker rm -f "$NAME" >/dev/null 2>&1 || true
  docker run -d --name "$NAME" -p "$PORT:22" \
    -e SRTOP_ARGS="$SRTOP_ARGS" \
    -v "$STATE_DIR:/keys:ro" "$IMAGE" >/dev/null

  for _ in $(seq 1 50); do
    docker logs "$NAME" 2>&1 | grep -q 'Server listening on' && break
    sleep 0.2
  done
  docker logs "$NAME" 2>&1 | grep -q 'Server listening on' \
    || { docker logs "$NAME" >&2; die "container never started sshd"; }

  # Fresh host keys every run, so the known_hosts file is rewritten every run.
  ssh-keyscan -p "$PORT" -t ed25519 127.0.0.1 2>/dev/null > "$KNOWN_HOSTS"
  [ -s "$KNOWN_HOSTS" ] || die "could not read the container's host key on port $PORT"

  docker logs "$NAME" 2>&1 | head -1
  cat <<EOF
devbox: $NAME up on 127.0.0.1:$PORT
devbox: client -> apps/srtop/devbox/devbox.sh client
devbox: shell  -> apps/srtop/devbox/devbox.sh ssh
devbox: stop   -> apps/srtop/devbox/devbox.sh down
EOF
}

cmd_client() {
  [ -f "$KNOWN_HOSTS" ] || die "no known_hosts yet; run 'devbox.sh up' first"
  swift build --package-path "$REPO_ROOT/client-macos" --product RendererDemoApp
  exec "$REPO_ROOT/client-macos/.build/debug/RendererDemoApp" \
    --ssh 127.0.0.1 --port "$PORT" --user srui --subsystem srui \
    --identity "$KEY" --known-hosts "$KNOWN_HOSTS"
}

cmd_spawn() {
  local seconds=${1:-30} count=${2:-1}
  docker exec -d -u srui "$NAME" sh -c \
    "for i in \$(seq 1 $count); do sleep $seconds & done; wait"
  echo "devbox: spawned $count sleeper(s) of ${seconds}s"
}

cmd_ssh() {
  exec ssh -p "$PORT" -i "$KEY" -o IdentitiesOnly=yes \
    -o UserKnownHostsFile="$KNOWN_HOSTS" -o BatchMode=yes srui@127.0.0.1 "$@"
}

cmd_down() {
  docker rm -f "$NAME" >/dev/null 2>&1 && echo "devbox: $NAME removed" \
    || echo "devbox: $NAME was not running"
}

case "${1:-}" in
  build) cmd_build ;;
  up) cmd_up ;;
  client) cmd_client ;;
  spawn) shift; cmd_spawn "$@" ;;
  ssh) shift; cmd_ssh "$@" ;;
  logs) docker logs -f "$NAME" ;;
  status)
    docker ps --filter "name=$NAME" --format '{{.Names}} {{.Status}} {{.Ports}}'
    docker exec "$NAME" ps -eo pid,comm 2>/dev/null || true
    ;;
  down) cmd_down ;;
  *)
    sed -n '2,11p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
    exit 1
    ;;
esac
