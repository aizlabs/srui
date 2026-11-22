#!/usr/bin/env bash
# PX-026-G01: the R1 clean-account walkthrough. Run it as a normal user, in a freshly created
# macOS account, from a clone of the repository; see CLEAN_ACCOUNT_WALKTHROUGH.md. It checks the
# prerequisites, builds the client from source, installs it under the account's home, starts the
# Linux devbox, connects and uses it, updates, rolls back and rolls forward, audits what it wrote
# for secrets, uninstalls, and proves that data it did not write survived. Idempotent: a re-run
# starts again from the prerequisites.
#
#   clean-account-walkthrough.sh                 the walkthrough (about 30-45 minutes)
#   clean-account-walkthrough.sh --dry-run       prerequisites, planting, install, update,
#                                                rollback, uninstall and verification against
#                                                stand-in binaries; no build, no Docker, no SSH
#   clean-account-walkthrough.sh --verify FILE   re-check a result file's checksum and summary
#   --output FILE        the result file (default ~/srui-r1-walkthrough-<UTC time>.txt)
#   --previous-rev REV   the release updated from and rolled back to (default the PX-026 merge)
#   --allow-existing-srui-data
#                        DANGEROUS. Without it the script refuses, before writing anything, an
#                        account that already has an R1 install (~/.local/share/srui-r1), saved
#                        connections (~/Library/Application Support/SRUI/saved-connections.json or
#                        its .unreadable backup), devbox state (~/.codex/srtop-devbox) or a
#                        srtop-devbox container. With it, the run replaces and then UNINSTALLS the
#                        existing R1 install, and replaces and then REMOVES the existing srtop-devbox
#                        container. It still never removes saved connections or devbox state it did
#                        not create. Use it only in a throwaway account.
#
# Writes only: the result file; ~/srui-r1-walkthrough/ (logs, the previous release's source, the
# run's TMPDIR); ~/.local/share/srui-r1/ (the install, through srui-r1-install.sh); ~/.codex/
# srtop-devbox/ (the devbox's own keypair and known_hosts) and the devbox container and image in
# the account's Docker; ~/Library/Application Support/SRUI/ (only a sentinel file this script plants
# and removes); four sentinel files listed in the result; the checkout's own .build directories;
# SwiftPM's and Docker's caches under ~/Library; and the /tmp/srui-* fixture directories the
# native tests create and remove. Never sudo, never ~/.ssh, never a system setting. A failed
# preflight or prerequisites step ends the run there, before anything is planted, built or
# installed.
#
# The result file holds timestamps, commands, exit codes and versions, with the home directory
# written as ~ and the host name as <host>; never a key, a token or an environment dump. Every
# step ends with a line "STEP <n> <name> PASS|FAIL|SKIP exit=<status> seconds=<s>", the file ends
# with "SUMMARY <verdict> ..." and "CHECKSUM sha256=<digest of every line before it>". A real run's
# verdict is PASS only if every step passed; a dry run's best verdict is DRY-RUN-OK.
set -uo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "$SCRIPT_DIR/../../.." && pwd)
# The PX-026 merge (PR #115): the last main before the R1 package, the release updated from.
DEFAULT_PREVIOUS_REV=e549ee8ac965f17cc4290330c3f1fda5733bf50c

MODE=real
OUTPUT=""
PREVIOUS_REV=$DEFAULT_PREVIOUS_REV
VERIFY=""
ALLOW_EXISTING=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --dry-run) MODE=dry-run; shift ;;
    --output) OUTPUT=${2:?--output needs a file}; shift 2 ;;
    --previous-rev) PREVIOUS_REV=${2:?--previous-rev needs a revision}; shift 2 ;;
    --verify) VERIFY=${2:?--verify needs a file}; shift 2 ;;
    --allow-existing-srui-data) ALLOW_EXISTING=1; shift ;;
    -h|--help) sed -n '2,40p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "clean-account-walkthrough: unknown option: $1" >&2; exit 2 ;;
  esac
done

# ---------------------------------------------------------------------------------------------
# --verify: the result file checks itself.
# ---------------------------------------------------------------------------------------------
verify_result() {
  local file=$1 recorded actual steps pass fail skip summary verdict mode expected
  [ -f "$file" ] || { echo "verify: no such file: $file"; return 2; }
  recorded=$(sed -n 's/^CHECKSUM sha256=//p' "$file" | tail -1)
  actual=$(sed '/^CHECKSUM sha256=/,$d' "$file" | shasum -a 256 | awk '{print $1}')
  [ -n "$recorded" ] || { echo "verify: FAIL no CHECKSUM line (the run did not finish)"; return 1; }
  [ "$recorded" = "$actual" ] || { echo "verify: FAIL checksum mismatch (the file was changed or truncated)"; return 1; }
  steps=$(grep -c '^STEP ' "$file")
  pass=$(grep -c '^STEP [0-9]* [a-z0-9-]* PASS ' "$file")
  fail=$(grep -c '^STEP [0-9]* [a-z0-9-]* FAIL ' "$file")
  skip=$(grep -c '^STEP [0-9]* [a-z0-9-]* SKIP ' "$file")
  summary=$(grep '^SUMMARY ' "$file" | tail -1)
  verdict=$(printf '%s\n' "$summary" | awk '{print $2}')
  mode=$(printf '%s\n' "$summary" | sed -n 's/.* mode=\([a-z-]*\).*/\1/p')
  if [ "$fail" -gt 0 ]; then expected=FAIL
  elif [ "$mode" = dry-run ]; then expected=DRY-RUN-OK
  elif [ "$skip" -gt 0 ]; then expected=FAIL
  else expected=PASS
  fi
  [ "$summary" = "SUMMARY $expected mode=$mode steps=$steps pass=$pass fail=$fail skip=$skip" ] \
    || { echo "verify: FAIL the summary does not follow from the steps ($summary; steps=$steps pass=$pass fail=$fail skip=$skip)"; return 1; }
  echo "verify: OK checksum matches; $summary"
  [ "$verdict" = PASS ] || [ "$verdict" = DRY-RUN-OK ]
}
if [ -n "$VERIFY" ]; then
  verify_result "$VERIFY"
  exit $?
fi

# ---------------------------------------------------------------------------------------------
# Never privileged. SRUI_R1_SELFTEST_AS_ROOT can only add the refusal, never remove it.
# ---------------------------------------------------------------------------------------------
if [ "$(id -u)" -eq 0 ] || [ -n "${SUDO_USER:-}" ] || [ -n "${SUDO_UID:-}" ] \
   || [ "${SRUI_R1_SELFTEST_AS_ROOT:-}" = 1 ]; then
  echo "clean-account-walkthrough: refusing to run as root or under sudo; run it as the new account's own user" >&2
  exit 3
fi
[ -n "${HOME:-}" ] && [ "$HOME" != / ] && [ -d "$HOME" ] || { echo "clean-account-walkthrough: HOME is not a directory" >&2; exit 2; }

STAMP=$(date -u +%Y%m%dT%H%M%SZ)
WROOT=$HOME/srui-r1-walkthrough
OUTPUT=${OUTPUT:-$HOME/srui-r1-walkthrough-$STAMP.txt}
LOGS=$WROOT/logs
EVIDENCE=$WROOT/evidence
export TMPDIR=$WROOT/tmp
export SRUI_R1_PREFIX=$HOME/.local/share/srui-r1
PREFIX=$SRUI_R1_PREFIX
export SRTOP_DEVBOX_STATE=$HOME/.codex/srtop-devbox
STATE=$SRTOP_DEVBOX_STATE
export SRTOP_DEVBOX_NAME=srtop-devbox SRTOP_DEVBOX_IMAGE=srtop-devbox:latest SRTOP_DEVBOX_PORT=2222
NAME=$SRTOP_DEVBOX_NAME
IMAGE=$SRTOP_DEVBOX_IMAGE
PORT=$SRTOP_DEVBOX_PORT
SETTINGS="$HOME/Library/Application Support/SRUI"
INSTALL=$SCRIPT_DIR/srui-r1-install.sh
AUDIT=$SCRIPT_DIR/privacy-audit.sh
DEVBOX=$REPO_ROOT/apps/srtop/devbox/devbox.sh
HOST_NAME=$(hostname -s 2>/dev/null || true)

# ---------------------------------------------------------------------------------------------
# Only in a fresh account (PX-026-G01 round 2, verifier F3): the run installs, then uninstalls and
# removes what it finds under its own names, so anything already there under those names would be
# lost. Checked before the first write; a refusal writes nothing at all.
# ---------------------------------------------------------------------------------------------
EXISTING=""
note_existing() { EXISTING="$EXISTING
  $1"; }
[ -e "$PREFIX" ] || [ -L "$PREFIX" ] && note_existing "$PREFIX (an SRUI R1 install)"
for name in saved-connections.json saved-connections.json.unreadable; do
  [ -e "$SETTINGS/$name" ] && note_existing "$SETTINGS/$name (saved connections)"
done
[ -e "$STATE" ] && note_existing "$STATE (devbox keypair and known_hosts)"
CONTAINER_EXISTED=0
if command -v docker >/dev/null 2>&1 && docker ps -aq --filter "name=^/$NAME\$" 2>/dev/null | grep -q .; then
  CONTAINER_EXISTED=1
  note_existing "a Docker container named $NAME"
fi
SAVED_PREEXISTED=0
[ -e "$SETTINGS/saved-connections.json" ] || [ -e "$SETTINGS/saved-connections.json.unreadable" ] && SAVED_PREEXISTED=1
if [ -n "$EXISTING" ] && [ -z "$ALLOW_EXISTING" ]; then
  echo "clean-account-walkthrough: this is not a fresh account; refusing before writing anything." >&2
  echo "These already exist, and the walkthrough would replace or remove them:$EXISTING" >&2
  echo "Run it in a newly created macOS account (see CLEAN_ACCOUNT_WALKTHROUGH.md)." >&2
  exit 4
fi

mkdir -p "$LOGS" "$EVIDENCE" "$TMPDIR" || exit 2
: > "$OUTPUT" || exit 2
chmod 600 "$OUTPUT"

# The checkout as <checkout>, the home directory as ~, the host name as <host>: literal
# replacement, never a pattern. Trailing blanks are dropped.
redact() {
  awk -v checkout="$REPO_ROOT" -v home="$HOME" -v host="$HOST_NAME" '
    function swap(text, from, to,   at, done) {
      if (length(from) < 3) return text
      done = ""
      while ((at = index(text, from)) > 0) {
        done = done substr(text, 1, at - 1) to
        text = substr(text, at + length(from))
      }
      return done text
    }
    { line = swap($0, checkout, "<checkout>"); line = swap(line, home, "~"); line = swap(line, host, "<host>")
      sub(/[ \t]+$/, "", line); print line }'
}
out() { printf '%s\n' "$*" | redact | tee -a "$OUTPUT"; }

STEP_NO=0 PASSES=0 FAILS=0 SKIPS=0
finish_step() { # name verdict status seconds log
  case "$2" in
    PASS) PASSES=$((PASSES + 1)) ;;
    FAIL) FAILS=$((FAILS + 1)) ;;
    SKIP) SKIPS=$((SKIPS + 1)) ;;
  esac
  if [ -n "$5" ] && [ -f "$5" ]; then
    grep '^FACT ' "$5" | redact | sed 's/^/  /' | tee -a "$OUTPUT"
    if [ "$2" = FAIL ]; then
      out "  last lines of the step's log ($(basename "$5")):"
      tail -n 30 "$5" | grep -v '^FACT ' | redact | sed 's/^/  | /' | tee -a "$OUTPUT"
    fi
  fi
  out "STEP $STEP_NO $1 $2 exit=$3 seconds=$4"
}

# run_step NAME COMMAND... : the step passes exactly when COMMAND exits 0.
run_step() {
  local name=$1 log start status
  shift
  STEP_NO=$((STEP_NO + 1))
  log=$LOGS/$(printf '%02d' "$STEP_NO")-$name.log
  out "BEGIN $STEP_NO $name $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  out "  command: $*"
  start=$(date +%s)
  "$@" >"$log" 2>&1
  status=$?
  if [ "$status" -eq 0 ]; then
    finish_step "$name" PASS "$status" $(( $(date +%s) - start )) "$log"
  else
    finish_step "$name" FAIL "$status" $(( $(date +%s) - start )) "$log"
  fi
}

skip_step() { # name reason command...
  local name=$1 reason=$2
  shift 2
  STEP_NO=$((STEP_NO + 1))
  out "BEGIN $STEP_NO $name $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  out "  command (not run): $*"
  out "  skipped: $reason"
  finish_step "$name" SKIP - 0 ""
}

fact() { echo "FACT $*"; }
sha_of() { if [ -f "$1" ]; then shasum -a 256 "$1" | awk '{print $1}'; else echo absent; fi; }

# ---------------------------------------------------------------------------------------------
# Steps. Each prints FACT lines (copied into the result) and returns non-zero on failure.
# ---------------------------------------------------------------------------------------------
step_preflight() {
  fact "mode: $MODE"
  fact "script: $SCRIPT_DIR/clean-account-walkthrough.sh"
  git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1 || { fact "FAIL: $REPO_ROOT is not a git checkout"; return 1; }
  HEAD_REV=$(git -C "$REPO_ROOT" rev-parse HEAD)
  fact "checkout: $REPO_ROOT at $HEAD_REV ($(git -C "$REPO_ROOT" status --porcelain | grep -c .) changed paths)"
  fact "previous release revision: $PREVIOUS_REV"
  git -C "$REPO_ROOT" cat-file -e "$PREVIOUS_REV^{commit}" 2>/dev/null || { fact "FAIL: the clone lacks $PREVIOUS_REV; clone the full history"; return 1; }
  fact "user id: $(id -u) (not root)"
  fact "result file: $OUTPUT"
}

version_at_least() { # have want (dotted)
  [ "$(printf '%s\n%s\n' "$2" "$1" | sort -t. -k1,1n -k2,2n -k3,3n | head -1)" = "$2" ]
}

step_prerequisites() {
  local ok=0 macos swift_line swift_version docker_server
  macos=$(sw_vers -productVersion 2>/dev/null || echo unknown)
  fact "macOS: $macos ($(sw_vers -buildVersion 2>/dev/null)) $(uname -m)"
  version_at_least "$macos" 14.0 || { fact "FAIL: macOS 14 or later is required"; ok=1; }
  fact "developer directory: $(xcode-select -p 2>/dev/null || echo none)"
  fact "xcode: $(xcodebuild -version 2>/dev/null | tr '\n' ' ' || true)"
  swift_line=$(swift --version 2>&1 | grep -m1 'Swift version' || true)
  fact "swift: ${swift_line:-absent}"
  swift_version=$(printf '%s\n' "$swift_line" | sed -n 's/.*Swift version \([0-9.]*\).*/\1/p')
  if [ -z "$swift_version" ] || ! version_at_least "$swift_version" 6.0; then
    fact "FAIL: Swift 6.0 or later is required (tested with 6.2.3); install Xcode or the Command Line Tools"; ok=1
  fi
  fact "git: $(git --version 2>/dev/null || echo absent)"
  fact "ssh: $(ssh -V 2>&1 || echo absent)"
  if command -v docker >/dev/null 2>&1; then
    fact "docker client: $(docker version -f '{{.Client.Version}}' 2>/dev/null || echo unknown)"
    if docker_server=$(docker version -f '{{.Server.Version}} {{.Server.KernelVersion}} {{.Server.Os}}/{{.Server.Arch}}' 2>/dev/null); then
      fact "docker server: $docker_server"
    else
      fact "FAIL: Docker is not running; start Docker Desktop in this account"; ok=1
    fi
  else
    fact "FAIL: docker is not installed"; ok=1
  fi
  fact "rust (optional; the Linux server is built inside the devbox image): $(rustc --version 2>/dev/null || echo absent) / $(cargo --version 2>/dev/null || echo absent)"
  if [ "$MODE" = real ] && command -v lsof >/dev/null 2>&1 && lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1; then
    fact "FAIL: port $PORT is already in use (stop the devbox in your other account first)"; ok=1
  fi
  return "$ok"
}

SENTINELS=""
step_plant_unrelated() {
  local file
  SENTINELS="$HOME/Documents/srui-r1-unrelated-sentinel.txt
$HOME/.codex/srui-r1-unrelated-sentinel.txt
$SETTINGS/srui-r1-unrelated-sentinel.txt
$HOME/.local/share/srui-r1-unrelated-sentinel.txt"
  PLANTED_DIRS=""
  for dir in "$HOME/Documents" "$HOME/.codex" "$SETTINGS" "$HOME/.local/share"; do
    [ -d "$dir" ] || PLANTED_DIRS="$PLANTED_DIRS$dir
"
  done
  STATE_PREEXISTED=0
  [ -e "$STATE" ] && STATE_PREEXISTED=1
  # What the settings directory held before (only with --allow-existing-srui-data can it hold
  # anything), so the check after the uninstall expects exactly that plus the sentinel.
  SETTINGS_BEFORE=$(ls -A "$SETTINGS" 2>/dev/null)
  SAVED_SUMS=""
  for file in "$SETTINGS/saved-connections.json" "$SETTINGS/saved-connections.json.unreadable"; do
    [ -f "$file" ] && SAVED_SUMS="$SAVED_SUMS$file $(sha_of "$file")
"
  done
  local IFS='
'
  for file in $SENTINELS; do
    mkdir -p "$(dirname "$file")" || return 1
    printf 'unrelated data that the R1 uninstall must not touch (%s)\n' "$STAMP" > "$file" || return 1
    fact "planted $file sha256=$(sha_of "$file")"
  done
  SENTINEL_SUMS=$(for file in $SENTINELS; do echo "$file $(sha_of "$file")"; done)
  unset IFS
  SSH_KNOWN_HOSTS_BEFORE=$(sha_of "$HOME/.ssh/known_hosts")
  SSH_CONFIG_BEFORE=$(sha_of "$HOME/.ssh/config")
  fact "~/.ssh/known_hosts before: $SSH_KNOWN_HOSTS_BEFORE; ~/.ssh/config before: $SSH_CONFIG_BEFORE (stat only)"
  fact "devbox state $STATE existed before: $STATE_PREEXISTED"
}

HEAD_BIN="" PREV_BIN=""
binary_facts() { # binary
  fact "binary sha256=$(sha_of "$1") size=$(wc -c <"$1" | tr -d ' ') B"
  fact "file: $(file -b "$1" 2>/dev/null)"
  fact "signature: $(codesign -dv "$1" 2>&1 | grep -E '^(Signature|CodeDirectory)' | tr '\n' ' ')"
}

step_build_client() {
  swift build -c release --package-path "$REPO_ROOT/client-macos" --product RendererDemoApp || return 1
  HEAD_BIN=$(swift build -c release --package-path "$REPO_ROOT/client-macos" --show-bin-path)/RendererDemoApp
  [ -x "$HEAD_BIN" ] || { fact "FAIL: no client binary"; return 1; }
  fact "built the R1 client at $HEAD_REV: $HEAD_BIN"
  binary_facts "$HEAD_BIN"
}

step_build_previous() {
  rm -rf "$WROOT/previous" && mkdir -p "$WROOT/previous" || return 1
  git -C "$REPO_ROOT" archive "$PREVIOUS_REV" client-macos | tar -x -C "$WROOT/previous" || return 1
  swift build -c release --package-path "$WROOT/previous/client-macos" --product RendererDemoApp || return 1
  PREV_BIN=$(swift build -c release --package-path "$WROOT/previous/client-macos" --show-bin-path)/RendererDemoApp
  [ -x "$PREV_BIN" ] || { fact "FAIL: no previous client binary"; return 1; }
  fact "built the previous client at $PREVIOUS_REV"
  binary_facts "$PREV_BIN"
}

# Dry run: two distinct stand-in executables in place of the builds.
step_stand_ins() {
  mkdir -p "$WROOT/stand-in/head" "$WROOT/stand-in/previous" || return 1
  printf '#!/bin/sh\necho "stand-in for the R1 client (%s)"\n' "$HEAD_REV" > "$WROOT/stand-in/head/RendererDemoApp"
  printf '#!/bin/sh\necho "stand-in for the previous client (%s)"\n' "$PREVIOUS_REV" > "$WROOT/stand-in/previous/RendererDemoApp"
  chmod 755 "$WROOT/stand-in/head/RendererDemoApp" "$WROOT/stand-in/previous/RendererDemoApp"
  HEAD_BIN=$WROOT/stand-in/head/RendererDemoApp
  PREV_BIN=$WROOT/stand-in/previous/RendererDemoApp
  fact "dry run: stand-in executables, not builds"
}

current_sha() { sha_of "$PREFIX/current/bin/RendererDemoApp"; }

expect_current() { # binary label
  local want have
  want=$(sha_of "$1")
  have=$(current_sha)
  fact "current release sha256=$have (expected the $2 client, $want)"
  [ "$have" = "$want" ]
}

step_install_previous() {
  "$INSTALL" install --from "$PREV_BIN" --revision "$PREVIOUS_REV" || return 1
  "$INSTALL" status
  expect_current "$PREV_BIN" previous
}

step_update() {
  "$INSTALL" update --from "$HEAD_BIN" --revision "$HEAD_REV" || return 1
  "$INSTALL" status
  expect_current "$HEAD_BIN" R1 || return 1
  # Plant data inside the prefix too: the uninstall must remove only what it installed.
  printf 'unrelated notes kept inside the install prefix (%s)\n' "$STAMP" > "$PREFIX/notes-unrelated.txt" || return 1
  PREFIX_SENTINEL_SUM=$(sha_of "$PREFIX/notes-unrelated.txt")
  fact "planted $PREFIX/notes-unrelated.txt sha256=$PREFIX_SENTINEL_SUM"
}

step_devbox_up() {
  IMAGE_PREEXISTED=0
  docker image inspect "$IMAGE" >/dev/null 2>&1 && IMAGE_PREEXISTED=1
  fact "image $IMAGE existed before: $IMAGE_PREEXISTED"
  "$DEVBOX" up || return 1
  fact "image label org.srui.revision=$(docker image inspect -f '{{index .Config.Labels "org.srui.revision"}}' "$IMAGE")"
  fact "image bases: $(grep -E '^FROM ' "$REPO_ROOT/apps/srtop/devbox/Dockerfile" | tr '\n' ' ')"
  fact "box kernel: $(docker exec "$NAME" uname -srm)"
  fact "box os: $(docker exec "$NAME" sh -c '. /etc/os-release && echo "$PRETTY_NAME"') ($(docker exec "$NAME" cat /etc/debian_version))"
  fact "box packages: $(docker exec "$NAME" dpkg-query -W -f '${Package}=${Version} ' openssh-server procps)"
  fact "box binaries: $(docker exec "$NAME" sha256sum /usr/local/bin/srtop /usr/local/bin/srui-ssh-bridge | awk '{printf "%s=%s ", $2, $1}')"
}

# Launches the installed client against the box, waits for its SSH subsystem (the box's bridge) to
# start, checks the client is still running, and stops exactly that process.
connect_check() { # label
  local client=$PREFIX/bin/srui-client pid found="" alive="" comm _
  [ -x "$client" ] || { fact "FAIL: no installed client at $client"; return 1; }
  "$client" --ssh 127.0.0.1 --port "$PORT" --user srui --subsystem srui \
    --identity "$STATE/id_ed25519" --known-hosts "$STATE/known_hosts" >"$LOGS/client-$1.log" 2>&1 &
  pid=$!
  for _ in $(seq 1 60); do
    if docker exec "$NAME" ps -eo comm= | grep -qx srui-ssh-bridge; then found=1; break; fi
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.5
  done
  sleep 2
  kill -0 "$pid" 2>/dev/null && alive=1
  fact "$1 client: the box's bridge started: ${found:-no}; the client still running 2 s later: ${alive:-no}"
  comm=$(ps -p "$pid" -o comm= 2>/dev/null || true)
  case "$comm" in
    *srui-client|*RendererDemoApp)
      kill -TERM "$pid"
      for _ in $(seq 1 20); do kill -0 "$pid" 2>/dev/null || break; sleep 0.5; done
      ;;
  esac
  if kill -0 "$pid" 2>/dev/null; then
    comm=$(ps -p "$pid" -o comm= 2>/dev/null || true)
    case "$comm" in *srui-client|*RendererDemoApp) kill -KILL "$pid" ;; esac
  fi
  wait "$pid" 2>/dev/null
  fact "$1 client stopped (it was PID $pid, $comm)"
  [ -n "$found" ] && [ -n "$alive" ]
}

step_connect() { connect_check r1; }

step_use_r0_scenario() {
  SRTOP_R0_KEEP_BOX=1 PX008_EVIDENCE_DIR=$EVIDENCE/r0 bash "$REPO_ROOT/apps/srtop/devbox/r0-scenario.sh" || return 1
  fact "$(grep -m1 -o 'PX-008 devbox evidence: window [0-9]* retained; [0-9]* published states' "$LOGS"/*-use-r0-scenario.log 2>/dev/null || echo 'R0 scenario passed')"
}

step_live_checks() {
  SRTOP_R1_KEEP_BOX=1 PX026G01_EVIDENCE_DIR=$EVIDENCE bash "$SCRIPT_DIR/live-checks.sh" || return 1
  grep -o 'PX-026-G01 R1 live evidence: [a-z ]*:' "$EVIDENCE/live-checks-swift-test.log" | sed 's/^/FACT passed /'
}

step_rollback() {
  "$INSTALL" rollback || return 1
  "$INSTALL" status
  expect_current "$PREV_BIN" previous || return 1
  if [ "$MODE" = real ]; then
    connect_check previous || return 1
  else
    fact "dry run: the rolled-back client is not launched"
  fi
}

step_roll_forward() {
  "$INSTALL" update --from "$HEAD_BIN" --revision "$HEAD_REV" || return 1
  "$INSTALL" status
  expect_current "$HEAD_BIN" R1
}

step_privacy_audit() {
  local targets
  targets="$OUTPUT
$PREFIX"
  [ -d "$SETTINGS" ] && targets="$targets
$SETTINGS"
  for file in "$LOGS"/client-*.log "$EVIDENCE/environment.txt" "$EVIDENCE/devbox-srtop.log" \
              "$EVIDENCE/devbox-container.log" "$EVIDENCE/client-unified.log"; do
    [ -f "$file" ] && targets="$targets
$file"
  done
  if [ "$MODE" = real ]; then
    for file in "$EVIDENCE/environment.txt" "$EVIDENCE/devbox-srtop.log" "$EVIDENCE/client-unified.log"; do
      [ -f "$file" ] || { fact "FAIL: $file was not collected"; return 1; }
    done
  fi
  fact "audited: $(printf '%s\n' "$targets" | tr '\n' ' ')"
  local IFS='
'
  # shellcheck disable=SC2086 # one path per line
  "$AUDIT" $targets
}

step_uninstall() {
  local status=0
  if [ "$MODE" = real ]; then
    "$DEVBOX" down || status=1
    if [ "${IMAGE_PREEXISTED:-1}" = 0 ]; then
      docker image rm "$IMAGE" >/dev/null && fact "removed the image $IMAGE this walkthrough built" || status=1
    else
      fact "kept the image $IMAGE, which existed before the walkthrough"
    fi
  fi
  if [ "$SAVED_PREEXISTED" = 0 ]; then
    "$INSTALL" uninstall --purge-settings || status=1
  else
    "$INSTALL" uninstall || status=1
    fact "kept the saved connections, which existed before the walkthrough"
  fi
  if [ "${STATE_PREEXISTED:-1}" = 0 ] && [ -d "$STATE" ]; then
    case "$STATE" in
      "$HOME"/.codex/srtop-devbox) rm -rf "$STATE" && fact "removed the devbox state $STATE this walkthrough created" ;;
    esac
  fi
  rm -rf "$WROOT/previous" "$WROOT/stand-in"
  return "$status"
}

step_verify_unrelated() {
  local ok=0 file want have line left IFS='
'
  for line in $SENTINEL_SUMS; do
    file=${line% *}
    want=${line##* }
    have=$(sha_of "$file")
    if [ "$have" = "$want" ]; then fact "intact: $file"; else fact "FAIL: $file changed or removed ($have)"; ok=1; fi
  done
  unset IFS
  have=$(sha_of "$PREFIX/notes-unrelated.txt")
  if [ "$have" = "${PREFIX_SENTINEL_SUM:-missing}" ]; then fact "intact: $PREFIX/notes-unrelated.txt"; else fact "FAIL: $PREFIX/notes-unrelated.txt changed or removed"; ok=1; fi
  left=$(ls -A "$PREFIX" 2>/dev/null | tr '\n' ' ')
  [ "$left" = "notes-unrelated.txt " ] && fact "the install prefix holds only the planted file" || { fact "FAIL: the install prefix still holds: $left"; ok=1; }
  local want_left
  want_left=$(printf '%s\n%s\n' "$SETTINGS_BEFORE" srui-r1-unrelated-sentinel.txt | grep . | LC_ALL=C sort | tr '\n' ' ')
  left=$(ls -A "$SETTINGS" 2>/dev/null | LC_ALL=C sort | tr '\n' ' ')
  [ "$left" = "$want_left" ] && fact "the settings directory holds what it held before, plus the planted file" \
    || { fact "FAIL: the settings directory holds: $left (expected: $want_left)"; ok=1; }
  local IFS='
'
  for line in $SAVED_SUMS; do
    file=${line% *}
    want=${line##* }
    [ "$(sha_of "$file")" = "$want" ] && fact "intact: $file (it existed before the walkthrough)" \
      || { fact "FAIL: $file existed before the walkthrough and changed or was removed"; ok=1; }
  done
  unset IFS
  [ "$(sha_of "$HOME/.ssh/known_hosts")" = "$SSH_KNOWN_HOSTS_BEFORE" ] && [ "$(sha_of "$HOME/.ssh/config")" = "$SSH_CONFIG_BEFORE" ] \
    && fact "~/.ssh/known_hosts and ~/.ssh/config unchanged" || { fact "FAIL: ~/.ssh changed"; ok=1; }
  if [ "$MODE" = real ]; then
    [ -z "$(docker ps -aq --filter "name=^/$NAME\$")" ] && fact "no devbox container" || { fact "FAIL: the devbox container is still there"; ok=1; }
    if [ "${STATE_PREEXISTED:-1}" = 0 ]; then
      [ ! -e "$STATE" ] && fact "no devbox state" || { fact "FAIL: $STATE is still there"; ok=1; }
    fi
  fi
  pgrep -f "$PREFIX/" >/dev/null 2>&1 && { fact "FAIL: a process from the install prefix is still running"; ok=1; } || fact "no process from the install prefix is running"
  # The sentinels did their job: remove them, and the directories planting them created.
  local IFS='
'
  for file in $SENTINELS; do rm -f "$file"; done
  rm -f "$PREFIX/notes-unrelated.txt"
  rmdir "$PREFIX" 2>/dev/null
  for file in $PLANTED_DIRS; do rmdir "$file" 2>/dev/null; done
  unset IFS
  fact "removed the sentinel files"
  return "$ok"
}

step_final_audit() {
  "$AUDIT" "$OUTPUT"
}

# The summary follows from the STEP lines alone; --verify recomputes it.
finish_run() {
  local verdict
  if [ "$FAILS" -gt 0 ]; then verdict=FAIL
  elif [ "$MODE" = dry-run ]; then verdict=DRY-RUN-OK
  elif [ "$SKIPS" -gt 0 ]; then verdict=FAIL
  else verdict=PASS
  fi
  out "finished $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  out "SUMMARY $verdict mode=$MODE steps=$STEP_NO pass=$PASSES fail=$FAILS skip=$SKIPS"
  printf 'CHECKSUM sha256=%s\n' "$(shasum -a 256 "$OUTPUT" | awk '{print $1}')" | tee -a "$OUTPUT"
  if [ "$verdict" = PASS ] && [ "$MODE" = real ]; then
    rm -rf "$WROOT"
    echo "The walkthrough passed. Send back: $OUTPUT"
  else
    echo "The walkthrough's verdict is $verdict. Send back: $OUTPUT (the step logs are in $LOGS)"
  fi
  [ "$verdict" = PASS ] || [ "$verdict" = DRY-RUN-OK ]
  exit $?
}

# A failed preflight or prerequisites step ends the run: nothing is planted, built or installed.
stop_run() { # step
  out "STOPPED after $1 failed: no later step ran, and nothing was planted, built or installed"
  finish_run
}

# ---------------------------------------------------------------------------------------------
out "SRUI R1 clean-account walkthrough (PX-026-G01)"
out "started $(date -u +%Y-%m-%dT%H:%M:%SZ) mode=$MODE"
if [ -n "$EXISTING" ]; then
  out "WARNING --allow-existing-srui-data: not a fresh account; existing:$(printf '%s' "$EXISTING" | tr '\n' ';')"
fi
HEAD_REV=unknown
run_step preflight step_preflight
[ "$FAILS" -eq 0 ] || stop_run preflight
run_step prerequisites step_prerequisites
[ "$FAILS" -eq 0 ] || stop_run prerequisites
run_step plant-unrelated step_plant_unrelated
if [ "$MODE" = real ]; then
  run_step build-client step_build_client
  run_step build-previous step_build_previous
else
  skip_step build-client "dry run" swift build -c release --package-path client-macos --product RendererDemoApp
  skip_step build-previous "dry run" swift build -c release "(git archive $PREVIOUS_REV client-macos)"
  run_step stand-ins step_stand_ins
fi
run_step install-previous step_install_previous
run_step update-to-r1 step_update
if [ "$MODE" = real ]; then
  run_step devbox-up step_devbox_up
  run_step connect step_connect
  run_step use-r0-scenario step_use_r0_scenario
  run_step r1-live-checks step_live_checks
else
  skip_step devbox-up "dry run" "$DEVBOX" up
  skip_step connect "dry run" "$PREFIX/bin/srui-client --ssh 127.0.0.1 --port $PORT ..."
  skip_step use-r0-scenario "dry run" apps/srtop/devbox/r0-scenario.sh
  skip_step r1-live-checks "dry run" apps/srtop/r1/live-checks.sh
fi
run_step rollback step_rollback
run_step roll-forward step_roll_forward
run_step privacy-audit step_privacy_audit
run_step uninstall step_uninstall
run_step verify-unrelated-data step_verify_unrelated
run_step final-output-audit step_final_audit

finish_run
