#!/usr/bin/env bash
# PX-026-G01: adds, and removes again byte for byte, the marked `srtop-devbox` Host block that
# checklist-session.sh puts in ~/.ssh/config so the Connect form can reach the devbox. Bash 3.2.
#
#   ssh-config-alias.sh add STATE_DIR PORT DEVBOX_STATE   back up ~/.ssh/config into STATE_DIR
#                                                          (cp -p), then append the block in place
#   ssh-config-alias.sh remove STATE_DIR                  put ~/.ssh/config back as it was
#
# add refuses a symbolic-link config (a dotfile manager's), a config that already holds the block,
# and a config that already defines a host named srtop-devbox. It writes in place, so the file
# keeps its inode and mode; a config it creates is mode 0600 in a 0700 ~/.ssh.
#
# remove restores the original byte for byte, in place and with its mode, whenever the file is
# still exactly the original plus the block: an originally absent file is removed (and a ~/.ssh it
# created, if empty), an originally empty one is left empty. If the file was edited meanwhile, it
# removes only the lines from the begin marker to the end marker, keeps everything else, and says
# the result may differ from the original in that block's surroundings.
set -euo pipefail

CONFIG=$HOME/.ssh/config
BEGIN_MARK="# >>> srui-r1 checklist alias (added by apps/srtop/r1/checklist-session.sh; removed when it ends) >>>"
END_MARK="# <<< srui-r1 checklist alias <<<"

die() { echo "ssh-config-alias: $*" >&2; exit 1; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }

block() { # port devbox-state
  printf '%s\n' "$BEGIN_MARK" "Host srtop-devbox" "  HostName 127.0.0.1" "  Port $1" \
    "  IdentityFile $2/id_ed25519" "  IdentitiesOnly yes" "  UserKnownHostsFile $2/known_hosts" "$END_MARK"
}

cmd_add() {
  local state=$1 port=$2 devbox=$3
  [ -d "$state" ] || die "no state directory $state"
  [ ! -L "$CONFIG" ] || die "~/.ssh/config is a symbolic link; refusing to edit it (add the srtop-devbox host yourself)"
  [ ! -L "$HOME/.ssh" ] || die "~/.ssh is a symbolic link; refusing to edit it"
  if [ -e "$CONFIG" ]; then
    [ -f "$CONFIG" ] || die "~/.ssh/config is not a regular file"
    ! grep -qxF "$BEGIN_MARK" "$CONFIG" || die "~/.ssh/config already holds a srui-r1 checklist alias; remove that block first"
    ! grep -qiE '^[[:space:]]*Host[[:space:]].*(^|[[:space:]])srtop-devbox([[:space:]]|$)' "$CONFIG" \
      || die "~/.ssh/config already defines a host named srtop-devbox; it is left alone"
    cp -p "$CONFIG" "$state/config.original"
    echo present > "$state/config.state"
  else
    echo absent > "$state/config.state"
    if [ ! -d "$HOME/.ssh" ]; then
      mkdir -m 700 "$HOME/.ssh"
      echo created > "$state/ssh-dir.state"
    fi
    (umask 077 && : > "$CONFIG")
  fi
  {
    # Separate the block from a last line that has no newline, so neither is changed.
    if [ -s "$CONFIG" ] && [ "$(tail -c 1 "$CONFIG" | od -An -c | tr -d ' ')" != '\n' ]; then echo; fi
    block "$port" "$devbox"
  } >> "$CONFIG"
  sha "$CONFIG" > "$state/config.expected-sha256"
}

cmd_remove() {
  local state=$1 original
  [ -f "$state/config.state" ] || return 0
  original=$(cat "$state/config.state")
  if [ ! -e "$CONFIG" ]; then
    echo "ssh-config-alias: ~/.ssh/config is gone; nothing to remove"
  elif [ "$(sha "$CONFIG")" = "$(cat "$state/config.expected-sha256")" ]; then
    if [ "$original" = absent ]; then
      rm -f "$CONFIG"
      [ -f "$state/ssh-dir.state" ] && rmdir "$HOME/.ssh" 2>/dev/null || true
    else
      # In place: the inode, the mode and every byte are the original's.
      cat "$state/config.original" > "$CONFIG"
      chmod "$(stat -f '%Lp' "$state/config.original")" "$CONFIG"
    fi
    echo "ssh-config-alias: restored ~/.ssh/config exactly as it was"
  else
    awk -v begin="$BEGIN_MARK" -v end="$END_MARK" '
      $0 == begin { skipping = 1; next }
      skipping && $0 == end { skipping = 0; next }
      !skipping { print }' "$CONFIG" > "$state/config.edited"
    cat "$state/config.edited" > "$CONFIG"
    echo "ssh-config-alias: ~/.ssh/config changed during the session; removed only the srtop-devbox block" >&2
  fi
  rm -f "$state/config.state" "$state/config.original" "$state/config.expected-sha256" \
        "$state/config.edited" "$state/ssh-dir.state"
}

case "${1:-}" in
  add) shift; [ "$#" -eq 3 ] || die "usage: ssh-config-alias.sh add STATE_DIR PORT DEVBOX_STATE"; cmd_add "$@" ;;
  remove) shift; [ "$#" -eq 1 ] || die "usage: ssh-config-alias.sh remove STATE_DIR"; cmd_remove "$1" ;;
  *) sed -n '2,17p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
