#!/usr/bin/env bash
# PX-026-G01: sets up, and afterwards takes down, what voiceover-keyboard-checklist.md needs: the
# devbox, the installed R1 client, and an ssh_config alias so the Connect form can reach the box
# with the box's own key and known_hosts. Run it as a normal user; it asks before it writes
# ~/.ssh/config, and removes exactly the block it added.
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
CONFIG=$HOME/.ssh/config
BEGIN_MARK="# >>> srui-r1 checklist alias (added by apps/srtop/r1/checklist-session.sh; removed when it ends) >>>"
END_MARK="# <<< srui-r1 checklist alias <<<"

die() { echo "checklist-session: $*" >&2; exit 1; }
[ "$(id -u)" -ne 0 ] && [ -z "${SUDO_USER:-}" ] || die "run it as your own user, never as root or under sudo"

added_alias="" created_ssh_dir="" client_pid="" box_up=""
teardown() {
  set +e
  if [ -n "$client_pid" ] && kill -0 "$client_pid" 2>/dev/null; then
    case "$(ps -p "$client_pid" -o comm= 2>/dev/null)" in
      *srui-client|*RendererDemoApp) kill -TERM "$client_pid" ;;
    esac
  fi
  if [ -n "$added_alias" ] && [ -f "$CONFIG" ]; then
    awk -v begin="$BEGIN_MARK" -v end="$END_MARK" '
      $0 == begin { skipping = 1; next }
      skipping && $0 == end { skipping = 0; next }
      !skipping { print }' "$CONFIG" > "$CONFIG.srui-r1.tmp" && mv "$CONFIG.srui-r1.tmp" "$CONFIG"
    [ -s "$CONFIG" ] || rm -f "$CONFIG"
    echo "checklist-session: removed the srtop-devbox alias from ~/.ssh/config"
  fi
  [ -n "$created_ssh_dir" ] && rmdir "$HOME/.ssh" 2>/dev/null
  [ -n "$box_up" ] && "$DEVBOX" down
}
trap teardown EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

"$DEVBOX" up
box_up=1
if [ ! -x "$PREFIX/bin/srui-client" ]; then
  "$R1_DIR/srui-r1-install.sh" install
fi

if [ -f "$CONFIG" ] && grep -qxF "$BEGIN_MARK" "$CONFIG"; then
  die "~/.ssh/config already holds a srui-r1 checklist alias from an earlier session; remove that block first"
fi
if [ -f "$CONFIG" ] && grep -qiE '^[[:space:]]*Host[[:space:]].*\bsrtop-devbox\b' "$CONFIG"; then
  die "~/.ssh/config already defines a host named srtop-devbox; this script will not change it"
fi
echo "checklist-session: the Connect form reaches the box through an ssh_config alias. This adds"
echo "a marked block for host 'srtop-devbox' to ~/.ssh/config (port $PORT, the box's own key and"
echo "known_hosts under $STATE), and removes exactly that block when the session ends."
printf 'Add it? [y/N] '
read -r answer
case "$answer" in y|Y|yes|YES) ;; *) die "not added; nothing was changed" ;; esac
if [ ! -d "$HOME/.ssh" ]; then
  mkdir -m 700 "$HOME/.ssh"
  created_ssh_dir=1
fi
{
  [ -s "$CONFIG" ] && echo
  echo "$BEGIN_MARK"
  echo "Host srtop-devbox"
  echo "  HostName 127.0.0.1"
  echo "  Port $PORT"
  echo "  IdentityFile $STATE/id_ed25519"
  echo "  IdentitiesOnly yes"
  echo "  UserKnownHostsFile $STATE/known_hosts"
  echo "$END_MARK"
} >> "$CONFIG"
chmod 600 "$CONFIG"
added_alias=1

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
