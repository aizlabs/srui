#!/usr/bin/env bash
# PX-026-G01: user-owned installation, update, rollback and uninstall of the R1 macOS client.
# No privileged step, helper or installer: everything lives under one prefix in the user's home,
# and only files this script wrote are ever removed. Bash 3.2 compatible (the macOS /bin/bash).
#
#   srui-r1-install.sh install  [--from BINARY --revision REV]   build (or take) a client and make it current
#   srui-r1-install.sh update   [--from BINARY --revision REV]   the same; the old current becomes previous
#   srui-r1-install.sh rollback                                  swap current and previous
#   srui-r1-install.sh status                                    what is installed
#   srui-r1-install.sh uninstall [--purge-settings]              remove what this script installed
#
# Without --from, the client is built from this checkout with its pinned toolchain:
#   swift build -c release --package-path client-macos --product RendererDemoApp
# and is not signed beyond the ad-hoc signature the linker applies (no Apple account, no
# notarization; R1 is distributed as source). Launch it as $PREFIX/bin/srui-client: with no
# arguments it opens the Connections window; --ssh HOST ... connects directly.
#
# Layout under SRUI_R1_PREFIX (default ~/.local/share/srui-r1):
#   .srui-r1-prefix                    marker; nothing is installed into or removed from a
#                                      directory without it (an empty or absent one is adopted)
#   releases/<id>/bin/RendererDemoApp  one release; <id> is the revision and the binary's digest
#   releases/<id>/MANIFEST             revision, sha256, toolchain, signature, install time
#   current, previous                  symlinks to releases; at most these two are kept
#   bin/srui-client                    symlink to current/bin/RendererDemoApp
# --purge-settings also removes the saved-connections file the client writes,
# ~/Library/Application Support/SRUI/saved-connections.json (and its .unreadable backup), and
# nothing else in that directory.
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "$SCRIPT_DIR/../../.." && pwd)
PREFIX=${SRUI_R1_PREFIX:-$HOME/.local/share/srui-r1}
MARKER_TEXT="srui-r1 install prefix v1"
MARKER=$PREFIX/.srui-r1-prefix
SETTINGS_DIR="$HOME/Library/Application Support/SRUI"

die() { echo "srui-r1-install: $*" >&2; exit 1; }
say() { echo "srui-r1-install: $*"; }

refuse_privilege() {
  # SRUI_R1_SELFTEST_AS_ROOT can only add the refusal, never remove it.
  if [ "$(id -u)" -eq 0 ] || [ "${SRUI_R1_SELFTEST_AS_ROOT:-}" = 1 ]; then
    echo "srui-r1-install: refusing to run as root; install as the user who will run the client" >&2
    exit 3
  fi
}

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }

# The prefix must be a private, user-owned directory inside the home directory.
check_prefix() {
  case "$PREFIX" in
    "$HOME"/?*) ;;
    *) die "SRUI_R1_PREFIX must be inside your home directory ($PREFIX is not)" ;;
  esac
  case "$PREFIX" in
    *"/../"*|*"/.."|*"/./"*) die "SRUI_R1_PREFIX must be a plain path: $PREFIX" ;;
  esac
  [ ! -L "$PREFIX" ] || die "$PREFIX is a symbolic link; refusing"
}

is_ours() { [ -f "$MARKER" ] && [ "$(cat "$MARKER")" = "$MARKER_TEXT" ]; }

adopt_prefix() {
  check_prefix
  if [ -e "$PREFIX" ] && ! is_ours; then
    [ -d "$PREFIX" ] && [ -z "$(ls -A "$PREFIX")" ] \
      || die "$PREFIX exists and is not an SRUI R1 prefix (no $MARKER); refusing to install into it"
  fi
  mkdir -p "$PREFIX/releases" "$PREFIX/bin"
  chmod 700 "$PREFIX"
  printf '%s\n' "$MARKER_TEXT" > "$MARKER"
}

release_of() { # link -> release id, or empty
  [ -L "$PREFIX/$1" ] || return 0
  basename "$(readlink "$PREFIX/$1")"
}

manifest_value() { # release key
  sed -n "s/^$2=//p" "$PREFIX/releases/$1/MANIFEST" 2>/dev/null | head -1
}

is_release() { [ -f "$PREFIX/releases/$1/MANIFEST" ] && [ "$(manifest_value "$1" format)" = "$MARKER_TEXT" ]; }

set_link() { # name release
  ln -sfn "releases/$2" "$PREFIX/$1.new"
  # -h: replace the link itself, never move into the directory it points at.
  mv -fh "$PREFIX/$1.new" "$PREFIX/$1"
}

remove_release() { # release: only the two files this script wrote, then the empty directories
  local dir=$PREFIX/releases/$1
  is_release "$1" || { say "keeping $dir: not a release this script wrote"; return 0; }
  rm -f "$dir/bin/RendererDemoApp" "$dir/MANIFEST"
  rmdir "$dir/bin" 2>/dev/null || say "keeping $dir/bin: it holds files this script did not write"
  rmdir "$dir" 2>/dev/null || say "keeping $dir: it holds files this script did not write"
}

# Retention: a release that is neither current nor previous is removed.
prune() {
  local keep_current keep_previous release
  keep_current=$(release_of current)
  keep_previous=$(release_of previous)
  for release in $(ls "$PREFIX/releases" 2>/dev/null); do
    [ "$release" = "$keep_current" ] || [ "$release" = "$keep_previous" ] || remove_release "$release"
  done
}

build_client() { # -> path of the built binary on stdout; build output on stderr
  local bin
  say "building the client: swift build -c release --package-path client-macos --product RendererDemoApp" >&2
  swift build -c release --package-path "$REPO_ROOT/client-macos" --product RendererDemoApp >&2
  bin=$(swift build -c release --package-path "$REPO_ROOT/client-macos" --show-bin-path)/RendererDemoApp
  [ -x "$bin" ] || die "the build produced no $bin"
  printf '%s\n' "$bin"
}

cmd_install() {
  local from="" revision="" digest id dir signature
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --from) from=${2:?--from needs a path}; shift 2 ;;
      --revision) revision=${2:?--revision needs a value}; shift 2 ;;
      *) die "unknown option: $1" ;;
    esac
  done
  if [ -z "$from" ]; then
    from=$(build_client)
    revision=$(git -C "$REPO_ROOT" rev-parse HEAD)
    git -C "$REPO_ROOT" diff --quiet HEAD -- client-macos || revision="$revision-modified"
  fi
  [ -n "$revision" ] || die "--from needs --revision"
  [ -f "$from" ] && [ -x "$from" ] || die "not an executable file: $from"
  case "$revision" in *[!A-Za-z0-9._-]*) die "invalid revision: $revision" ;; esac
  adopt_prefix
  digest=$(sha256 "$from")
  id=$(printf '%s' "$revision" | cut -c1-12)-$(printf '%s' "$digest" | cut -c1-12)
  dir=$PREFIX/releases/$id
  if ! is_release "$id"; then
    [ ! -e "$dir" ] || die "$dir exists and is not a release this script wrote"
    mkdir -p "$dir/bin"
    cp "$from" "$dir/bin/RendererDemoApp.tmp"
    chmod 755 "$dir/bin/RendererDemoApp.tmp"
    mv "$dir/bin/RendererDemoApp.tmp" "$dir/bin/RendererDemoApp"
    [ "$(sha256 "$dir/bin/RendererDemoApp")" = "$digest" ] || die "the copy in $dir does not match $from"
    signature=$(codesign -dv "$dir/bin/RendererDemoApp" 2>&1 | sed -n 's/^Signature=//p' | head -1 || true)
    {
      echo "format=$MARKER_TEXT"
      echo "revision=$revision"
      echo "sha256=$digest"
      echo "signature=${signature:-none}"
      echo "swift=$(swift --version 2>/dev/null | head -1 || echo unknown)"
      echo "macos=$(sw_vers -productVersion 2>/dev/null || echo unknown)"
      echo "installed_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    } > "$dir/MANIFEST"
  fi
  local old
  old=$(release_of current)
  if [ "$old" != "$id" ]; then
    [ -n "$old" ] && set_link previous "$old"
    set_link current "$id"
  fi
  ln -sfn ../current/bin/RendererDemoApp "$PREFIX/bin/srui-client"
  prune
  say "current: $id (revision $revision); previous: $(release_of previous)"
  say "run it: $PREFIX/bin/srui-client"
}

cmd_rollback() {
  check_prefix
  is_ours || die "nothing is installed in $PREFIX"
  local current previous
  current=$(release_of current)
  previous=$(release_of previous)
  [ -n "$previous" ] && is_release "$previous" || die "there is no previous release to roll back to"
  set_link current "$previous"
  [ -n "$current" ] && set_link previous "$current"
  say "current: $previous (revision $(manifest_value "$previous" revision)); previous: $current"
}

cmd_status() {
  check_prefix
  if ! is_ours; then
    say "not installed ($PREFIX)"
    return 0
  fi
  local name release
  for name in current previous; do
    release=$(release_of "$name")
    if [ -n "$release" ]; then
      say "$name: $release revision=$(manifest_value "$release" revision) sha256=$(manifest_value "$release" sha256) signature=$(manifest_value "$release" signature)"
    else
      say "$name: none"
    fi
  done
}

cmd_uninstall() {
  local purge="" release
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --purge-settings) purge=1; shift ;;
      *) die "unknown option: $1" ;;
    esac
  done
  check_prefix
  if is_ours; then
    rm -f "$PREFIX/current" "$PREFIX/previous" "$PREFIX/current.new" "$PREFIX/previous.new"
    [ -L "$PREFIX/bin/srui-client" ] && rm -f "$PREFIX/bin/srui-client"
    for release in $(ls "$PREFIX/releases" 2>/dev/null); do
      remove_release "$release"
    done
    rmdir "$PREFIX/releases" 2>/dev/null || true
    rmdir "$PREFIX/bin" 2>/dev/null || true
    rm -f "$MARKER"
    if rmdir "$PREFIX" 2>/dev/null; then
      say "removed $PREFIX"
    else
      say "removed the client; kept $PREFIX, which holds files this script did not write:"
      ls -A "$PREFIX" | sed 's/^/  /'
    fi
  else
    say "no client installed in $PREFIX"
  fi
  if [ -n "$purge" ]; then
    if [ ! -d "$SETTINGS_DIR" ]; then
      say "no saved connections ($SETTINGS_DIR does not exist)"
    else
      rm -f "$SETTINGS_DIR/saved-connections.json" "$SETTINGS_DIR/saved-connections.json.unreadable"
      if rmdir "$SETTINGS_DIR" 2>/dev/null; then
        say "removed the saved connections and $SETTINGS_DIR"
      else
        say "removed the saved connections; kept everything else in $SETTINGS_DIR"
      fi
    fi
  else
    say "saved connections kept (remove them with --purge-settings)"
  fi
}

refuse_privilege
case "${1:-}" in
  install|update) shift; cmd_install "$@" ;;
  rollback) cmd_rollback ;;
  status) cmd_status ;;
  uninstall) shift; cmd_uninstall "$@" ;;
  *) sed -n '2,27p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
