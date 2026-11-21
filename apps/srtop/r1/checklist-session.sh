#!/usr/bin/env bash
# PX-026-G01: sets up, and afterwards takes down, what voiceover-keyboard-checklist.md needs: the
# devbox, the installed R1 client, and an ssh_config alias so the Connect form can reach the box
# with the box's own key and known_hosts. Run it as a normal user; it asks before it writes
# ~/.ssh/config, refuses a symbolic-link one, and puts it back byte for byte when it ends, also on
# HUP, INT and TERM (ssh-config-alias.sh).
#
#   apps/srtop/r1/checklist-session.sh
#
# While it waits for Enter, run these in another Terminal window when the checklist says so:
#   apps/srtop/devbox/devbox.sh drop-bridges     a disconnect: srtop keeps its session
#   apps/srtop/devbox/devbox.sh restart-srtop    a replacement: a new session
# Then press Enter here: the client is quit, the alias removed and the box stopped.
set -euo pipefail

R1_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "$R1_DIR/../../.." && pwd)
DEVBOX=$REPO_ROOT/apps/srtop/devbox/devbox.sh
STATE=${SRTOP_DEVBOX_STATE:-$HOME/.codex/srtop-devbox}
PORT=${SRTOP_DEVBOX_PORT:-2222}
PREFIX=${SRUI_R1_PREFIX:-$HOME/.local/share/srui-r1}
ALIAS=$R1_DIR/ssh-config-alias.sh

die() { echo "checklist-session: $*" >&2; exit 1; }
[ "$(id -u)" -ne 0 ] && [ -z "${SUDO_USER:-}" ] || die "run it as your own user, never as root or under sudo"

# The backup of ~/.ssh/config and what it takes to put it back (ssh-config-alias.sh).
ALIAS_STATE=$(mktemp -d "${TMPDIR:-/tmp}/srui-r1-checklist.XXXXXX")
client_pid="" box_up=""
teardown() {
  set +e
  if [ -n "$client_pid" ] && kill -0 "$client_pid" 2>/dev/null; then
    case "$(ps -p "$client_pid" -o comm= 2>/dev/null)" in
      *srui-client|*RendererDemoApp) kill -TERM "$client_pid" ;;
    esac
  fi
  "$ALIAS" remove "$ALIAS_STATE"
  rm -rf "$ALIAS_STATE"
  [ -n "$box_up" ] && "$DEVBOX" down
}
trap teardown EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

"$DEVBOX" up
box_up=1
if [ ! -x "$PREFIX/bin/srui-client" ]; then
  "$R1_DIR/srui-r1-install.sh" install
fi

echo "checklist-session: the Connect form reaches the box through an ssh_config alias. This adds"
echo "a marked block for host 'srtop-devbox' to ~/.ssh/config (port $PORT, the box's own key and"
echo "known_hosts under $STATE). When the session ends, the file is put back exactly as it was."
printf 'Add it? [y/N] '
read -r answer
case "$answer" in y|Y|yes|YES) ;; *) die "not added; nothing was changed" ;; esac
# Refuses a symbolic-link config, an existing block and an existing srtop-devbox host.
"$ALIAS" add "$ALIAS_STATE" "$PORT" "$STATE"

"$PREFIX/bin/srui-client" >/dev/null 2>&1 &
client_pid=$!
printf '%s\n' "" \
  "checklist-session: the R1 client is open (its Connections window). In the Connect form use" \
  "  Host: srtop-devbox    Remote User: srui    (Port and Name empty)" \
  "In another Terminal window, when the checklist asks for them:" \
  "  $DEVBOX drop-bridges" \
  "  $DEVBOX restart-srtop" \
  "Press Enter here when the checklist is done; the client is quit, the alias removed, the box stopped."
read -r _
