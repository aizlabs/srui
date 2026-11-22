#!/usr/bin/env bash
# PX-026-G01: self-tests of the R1 package scripts, with no build, no Docker and no SSH. Each runs
# in a throwaway HOME under one temporary directory, which is removed at the end.
#
#   apps/srtop/r1/selftest.sh
#
# 1. privacy-audit.sh --self-test: one planted secret of every class is found, a clean file passes.
# 2. srui-r1-install.sh: refuses root; install, update, retention, rollback; refuses a foreign or
#    out-of-home prefix; uninstall removes exactly what it installed, keeps planted files inside
#    the prefix and the settings directory, and touches no ~/.ssh file.
# 3. clean-account-walkthrough.sh: refuses root before writing anything; a --dry-run (under
#    sandbox-exec, allowed to write only inside the throwaway HOME) ends DRY-RUN-OK, verifies its
#    own checksum, leaves a planted file intact and lists every file it created; a failing
#    prerequisite makes its step FAIL, stops the run there and makes the summary FAIL; an edited
#    result fails --verify. A HOME that already holds an R1 install, saved connections, devbox
#    state or a srtop-devbox container is refused before any write, byte for byte unchanged; with
#    --allow-existing-srui-data, saved connections it did not create are kept.
# 4. ssh-config-alias.sh (checklist-session.sh's ~/.ssh/config edit): after add and remove, an
#    absent, empty, unterminated, ordinary or 0600 config is byte for byte, mode and existence what
#    it was; a symbolic link, an existing block and an existing srtop-devbox host are refused; a
#    config edited during the session loses only the block.
# Bash 3.2 compatible. Exit 0 only if every case passes.
set -uo pipefail

R1_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(mktemp -d "${TMPDIR:-/tmp}/srui-r1-selftest.XXXXXX")
ROOT=$(cd "$ROOT" && pwd -P)
# Removes only this run's own directory.
# shellcheck disable=SC2064
trap "rm -rf '$ROOT'" EXIT
FAILURES=0
CASES=0

pass() { CASES=$((CASES + 1)); echo "PASS $*"; }
fail() { CASES=$((CASES + 1)); FAILURES=$((FAILURES + 1)); echo "FAIL $*"; }
check() { # description command...
  local description=$1
  shift
  if "$@"; then pass "$description"; else fail "$description"; fi
}
sha() { if [ -f "$1" ]; then shasum -a 256 "$1" | awk '{print $1}'; else echo absent; fi; }

# ---------------------------------------------------------------------------------------------
echo "== privacy-audit.sh --self-test"
if bash "$R1_DIR/privacy-audit.sh" --self-test >"$ROOT/privacy.log" 2>&1; then
  pass "privacy audit: $(tail -1 "$ROOT/privacy.log")"
else
  fail "privacy audit self-test"
  sed 's/^/  | /' "$ROOT/privacy.log"
fi

# ---------------------------------------------------------------------------------------------
echo "== srui-r1-install.sh"
H=$ROOT/install-home
mkdir -p "$H/.ssh" "$H/Library/Application Support/SRUI" "$H/bin"
printf '[example.invalid]:22 ssh-ed25519 AAAAplanted\n' > "$H/.ssh/known_hosts"
printf '{"connections":[],"version":1}\n' > "$H/Library/Application Support/SRUI/saved-connections.json"
printf 'not srui\n' > "$H/Library/Application Support/SRUI/other-app-notes.txt"
for name in a b c; do
  printf '#!/bin/sh\necho stand-in %s\n' "$name" > "$H/bin/client-$name"
  chmod 755 "$H/bin/client-$name"
done
known_before=$(sha "$H/.ssh/known_hosts")
notes_before=$(sha "$H/Library/Application Support/SRUI/other-app-notes.txt")
inst() { HOME=$H SRUI_R1_PREFIX=${PREFIX_OVERRIDE:-$H/.local/share/srui-r1} bash "$R1_DIR/srui-r1-install.sh" "$@"; }
P=$H/.local/share/srui-r1
current() { sha "$P/current/bin/RendererDemoApp"; }

SRUI_R1_SELFTEST_AS_ROOT=1 inst status >/dev/null 2>&1
check "install: refuses to run as root (exit 3)" [ "$?" -eq 3 ]
check "install: the refusal created nothing" [ ! -e "$P" ]
inst install --from "$H/bin/client-a" --revision aaaa111 >/dev/null
check "install: release a is current" [ "$(current)" = "$(sha "$H/bin/client-a")" ]
inst update --from "$H/bin/client-b" --revision bbbb222 >/dev/null
check "update: release b is current" [ "$(current)" = "$(sha "$H/bin/client-b")" ]
check "update: a is previous" [ "$(sha "$P/previous/bin/RendererDemoApp")" = "$(sha "$H/bin/client-a")" ]
check "update: the launcher runs the current release" [ "$("$P/bin/srui-client")" = "stand-in b" ]
inst rollback >/dev/null
check "rollback: a is current again" [ "$(current)" = "$(sha "$H/bin/client-a")" ]
check "rollback: the launcher follows" [ "$("$P/bin/srui-client")" = "stand-in a" ]
inst rollback >/dev/null
inst update --from "$H/bin/client-c" --revision cccc333 >/dev/null
check "retention: only current and previous are kept" [ "$(ls "$P/releases" | grep -c .)" -eq 2 ]
ok=1; [ "$(current)" = "$(sha "$H/bin/client-c")" ] && [ "$(sha "$P/previous/bin/RendererDemoApp")" = "$(sha "$H/bin/client-b")" ] && ok=0
check "retention: c current, b previous" [ "$ok" -eq 0 ]
mkdir -p "$H/foreign" && printf 'mine\n' > "$H/foreign/keep.txt"
PREFIX_OVERRIDE=$H/foreign inst install --from "$H/bin/client-a" --revision aaaa111 >/dev/null 2>&1
check "install: refuses a non-empty directory it did not create" [ "$?" -ne 0 ]
check "install: the foreign directory is untouched" [ "$(ls -A "$H/foreign")" = keep.txt ]
ok=1; PREFIX_OVERRIDE=$ROOT/outside inst install --from "$H/bin/client-a" --revision aaaa111 >/dev/null 2>&1 || ok=0
[ -e "$ROOT/outside" ] && ok=1
check "install: refuses a prefix outside the home directory" [ "$ok" -eq 0 ]
printf 'my notes\n' > "$P/notes.txt"
mkdir -p "$P/releases/not-ours" && printf 'x\n' > "$P/releases/not-ours/file"
notes_in_prefix=$(sha "$P/notes.txt")
PREFIX_OVERRIDE=$H/foreign inst uninstall >/dev/null 2>&1
check "uninstall: a foreign prefix is left alone" [ "$(ls -A "$H/foreign")" = keep.txt ]
inst uninstall --purge-settings >"$ROOT/uninstall.log" 2>&1
check "uninstall: exit 0" [ "$?" -eq 0 ]
ok=1; [ ! -e "$P/current" ] && [ ! -e "$P/previous" ] && [ ! -e "$P/bin" ] && [ ! -e "$P/.srui-r1-prefix" ] && ok=0
check "uninstall: no release, link or marker is left" [ "$ok" -eq 0 ]
check "uninstall: the planted file in the prefix is intact" [ "$(sha "$P/notes.txt")" = "$notes_in_prefix" ]
check "uninstall: a directory it did not write is kept" [ -f "$P/releases/not-ours/file" ]
check "uninstall --purge-settings: the saved connections are gone" [ ! -e "$H/Library/Application Support/SRUI/saved-connections.json" ]
check "uninstall --purge-settings: other files in the settings directory are intact" \
  [ "$(sha "$H/Library/Application Support/SRUI/other-app-notes.txt")" = "$notes_before" ]
check "uninstall: ~/.ssh/known_hosts is intact" [ "$(sha "$H/.ssh/known_hosts")" = "$known_before" ]

# ---------------------------------------------------------------------------------------------
echo "== clean-account-walkthrough.sh"
W=$ROOT/walk-home
STUBS=$ROOT/stubs
mkdir -p "$W/Documents" "$STUBS/pass" "$STUBS/fail"
printf 'a document the walkthrough must not touch\n' > "$W/Documents/keep-me.txt"
keep_before=$(sha "$W/Documents/keep-me.txt")
# Docker stand-ins: a running daemon with no srtop-devbox container, no daemon, and a daemon that
# has one.
mkdir -p "$STUBS/container"
printf '#!/bin/sh\ncase "$*" in version*) echo "28.3.2 6.10.14-linuxkit linux/arm64" ;; esac\nexit 0\n' > "$STUBS/pass/docker"
printf '#!/bin/sh\necho "Cannot connect to the Docker daemon" >&2\nexit 1\n' > "$STUBS/fail/docker"
printf '#!/bin/sh\ncase "$*" in version*) echo "28.3.2 6.10.14-linuxkit linux/arm64" ;; ps*) echo 0123abcd4567 ;; esac\nexit 0\n' > "$STUBS/container/docker"
chmod 755 "$STUBS/pass/docker" "$STUBS/fail/docker" "$STUBS/container/docker"

walk() { # stubs-dir args...
  local stubs=$1
  shift
  HOME=${WALK_HOME:-$W} PATH="$stubs:$PATH" TMPDIR=${WALK_HOME:-$W}/tmp bash "$R1_DIR/clean-account-walkthrough.sh" "$@"
}
# Every path under a directory, with each file's digest and mode: what "nothing changed" means.
snapshot() {
  (cd "$1" && find . | LC_ALL=C sort | while IFS= read -r f; do
    if [ -f "$f" ] && [ ! -L "$f" ]; then echo "$f $(stat -f '%Lp' "$f") $(shasum -a 256 "$f" | awk '{print $1}')"
    else echo "$f"; fi
  done)
}

before=$(cd "$W" && find . | LC_ALL=C sort)
SRUI_R1_SELFTEST_AS_ROOT=1 walk "$STUBS/pass" --dry-run >/dev/null 2>&1
check "walkthrough: refuses to run as root (exit 3)" [ "$?" -eq 3 ]
check "walkthrough: the refusal wrote nothing" [ "$(cd "$W" && find . | LC_ALL=C sort)" = "$before" ]

# The dry run may write only inside the throwaway HOME (and /dev): anything else is denied.
SANDBOX=""
if command -v sandbox-exec >/dev/null 2>&1; then
  SANDBOX="(version 1)(allow default)(deny file-write*)
(allow file-write* (subpath \"$W\") (literal \"/dev/null\") (literal \"/dev/tty\") (literal \"/dev/dtracehelper\") (regex #\"^/dev/fd/\"))"
fi
if [ -n "$SANDBOX" ]; then
  # The proof is only as good as the sandbox: a write outside the HOME must be refused.
  ok=1; sandbox-exec -p "$SANDBOX" sh -c ": > '$ROOT/escape-probe'" 2>/dev/null || ok=0
  [ -e "$ROOT/escape-probe" ] && ok=1
  check "sandbox: a write outside the throwaway HOME is refused" [ "$ok" -eq 0 ]
fi
sandboxed_walk() { # stubs output [extra args]; HOME is WALK_HOME or $W
  local home=${WALK_HOME:-$W} stubs=$1 output=$2
  shift 2
  if [ -n "$SANDBOX" ]; then
    local profile="(version 1)(allow default)(deny file-write*)
(allow file-write* (subpath \"$home\") (literal \"/dev/null\") (literal \"/dev/tty\") (literal \"/dev/dtracehelper\") (regex #\"^/dev/fd/\"))"
    HOME=$home PATH="$stubs:$PATH" TMPDIR=$home/tmp sandbox-exec -p "$profile" bash "$R1_DIR/clean-account-walkthrough.sh" --dry-run --output "$output" "$@"
  else
    walk "$stubs" --dry-run --output "$output" "$@"
  fi
}
sandboxed_walk "$STUBS/pass" "$W/result-pass.txt" >"$ROOT/walk-pass.log" 2>&1
status=$?
check "walkthrough --dry-run: exit 0 ($([ -n "$SANDBOX" ] && echo 'under sandbox-exec, writes only inside HOME' || echo 'no sandbox-exec here'))" [ "$status" -eq 0 ]
[ "$status" -eq 0 ] || sed 's/^/  | /' "$ROOT/walk-pass.log"
check "walkthrough --dry-run: DRY-RUN-OK with no failed step" grep -q '^SUMMARY DRY-RUN-OK mode=dry-run steps=[0-9]* pass=[0-9]* fail=0 ' "$W/result-pass.txt"
check "walkthrough --verify: the checksum and summary hold" walk "$STUBS/pass" --verify "$W/result-pass.txt"
check "walkthrough --dry-run: install, update, rollback, roll-forward and uninstall ran and passed" \
  [ "$(grep -cE '^STEP [0-9]+ (install-previous|update-to-r1|rollback|roll-forward|uninstall|verify-unrelated-data|privacy-audit|final-output-audit) PASS ' "$W/result-pass.txt")" -eq 8 ]
check "walkthrough --dry-run: the planted document is intact" [ "$(sha "$W/Documents/keep-me.txt")" = "$keep_before" ]
ok=0; grep -qF "$W" "$W/result-pass.txt" && ok=1
check "walkthrough --dry-run: no home path in the result" [ "$ok" -eq 0 ]
ok=1; [ ! -e "$W/.local/share/srui-r1" ] && [ ! -e "$W/Library/Application Support/SRUI" ] \
  && [ -z "$(cd "$W" && find . -name 'srui-r1-unrelated-sentinel.txt')" ] && ok=0
check "walkthrough --dry-run: no install prefix, settings or sentinel left" [ "$ok" -eq 0 ]
echo "  files the dry run left in its HOME (all inside it by construction):"
(cd "$W" && find . -type f | LC_ALL=C sort | sed 's/^/    /')

sandboxed_walk "$STUBS/fail" "$W/result-fail.txt" >"$ROOT/walk-fail.log" 2>&1
check "walkthrough: a failed prerequisite fails the run (non-zero exit)" [ "$?" -ne 0 ]
check "walkthrough: the prerequisites step reports FAIL" grep -q '^STEP 2 prerequisites FAIL exit=1 ' "$W/result-fail.txt"
check "walkthrough: the summary is FAIL" grep -q '^SUMMARY FAIL mode=dry-run ' "$W/result-fail.txt"
ok=1; grep -q '^STOPPED after prerequisites failed' "$W/result-fail.txt" && ! grep -q '^STEP 3 ' "$W/result-fail.txt" \
  && [ -z "$(cd "$W" && find . -name 'srui-r1-unrelated-sentinel.txt')" ] && [ ! -e "$W/.local/share/srui-r1" ] && ok=0
check "walkthrough: a failed prerequisite stops the run before anything is planted or installed" [ "$ok" -eq 0 ]
# A failed preflight (a previous release revision the clone lacks) stops before the prerequisites.
sandboxed_walk "$STUBS/pass" "$W/result-preflight.txt" --previous-rev 0000000000000000000000000000000000000000 >"$ROOT/walk-preflight.log" 2>&1
status=$?
ok=1; [ "$status" -ne 0 ] && grep -q '^STEP 1 preflight FAIL exit=1 ' "$W/result-preflight.txt" \
  && grep -q '^STOPPED after preflight failed' "$W/result-preflight.txt" && ! grep -q '^STEP 2 ' "$W/result-preflight.txt" \
  && grep -q '^SUMMARY FAIL mode=dry-run steps=1 pass=0 fail=1 skip=0$' "$W/result-preflight.txt" && ok=0
check "walkthrough: a failed preflight stops the run with SUMMARY FAIL (exit $status)" [ "$ok" -eq 0 ]
ok=1; walk "$STUBS/pass" --verify "$W/result-fail.txt" >"$ROOT/verify-fail.log" 2>&1 || ok=0
grep -q '^verify: OK checksum matches; SUMMARY FAIL ' "$ROOT/verify-fail.log" || ok=1
check "walkthrough --verify: the failed result's checksum holds and its verdict fails" [ "$ok" -eq 0 ]
sed 's/^STEP 2 prerequisites FAIL/STEP 2 prerequisites PASS/' "$W/result-fail.txt" > "$W/result-edited.txt"
ok=1; walk "$STUBS/pass" --verify "$W/result-edited.txt" >"$ROOT/verify-edited.log" 2>&1 || ok=0
grep -q 'checksum mismatch' "$ROOT/verify-edited.log" || ok=1
check "walkthrough --verify: an edited result is refused" [ "$ok" -eq 0 ]

# Not a fresh account: each of these is refused before any write, and the HOME is unchanged.
not_fresh() { # name stubs setup-command...
  local name=$1 stubs=$2 home=$ROOT/main-$1 before after status
  shift 2
  mkdir -p "$home/Documents" && printf 'mine\n' > "$home/Documents/notes.txt"
  (cd "$home" && "$@")
  before=$(snapshot "$home")
  WALK_HOME=$home walk "$stubs" --dry-run >"$ROOT/main-$name.log" 2>&1
  status=$?
  after=$(snapshot "$home")
  ok=1; [ "$status" -eq 4 ] && [ "$before" = "$after" ] && grep -q 'not a fresh account' "$ROOT/main-$name.log" && ok=0
  check "walkthrough: refuses a HOME with $name (exit $status), nothing written or removed" [ "$ok" -eq 0 ]
}
not_fresh saved-connections "$STUBS/pass" sh -c 'mkdir -p "Library/Application Support/SRUI" && printf "{\"connections\":[],\"version\":1}\n" > "Library/Application Support/SRUI/saved-connections.json"'
not_fresh unreadable-backup "$STUBS/pass" sh -c 'mkdir -p "Library/Application Support/SRUI" && printf "x" > "Library/Application Support/SRUI/saved-connections.json.unreadable"'
not_fresh an-r1-install "$STUBS/pass" sh -c 'mkdir -p .local/share/srui-r1/releases && printf "srui-r1 install prefix v1\n" > .local/share/srui-r1/.srui-r1-prefix'
not_fresh devbox-state "$STUBS/pass" sh -c 'mkdir -p .codex/srtop-devbox && printf "k\n" > .codex/srtop-devbox/known_hosts'
not_fresh a-devbox-container "$STUBS/container" true
echo "  the main-account-like HOME with saved connections, before and after (identical):"
snapshot "$ROOT/main-saved-connections" | sed 's/^/    /'

# The dangerous override: the run proceeds, and saved connections it did not create are kept.
O=$ROOT/override-home
mkdir -p "$O/Library/Application Support/SRUI"
printf '{"connections":[],"version":1}\n' > "$O/Library/Application Support/SRUI/saved-connections.json"
saved_before=$(sha "$O/Library/Application Support/SRUI/saved-connections.json")
WALK_HOME=$O sandboxed_walk "$STUBS/pass" "$O/result.txt" --allow-existing-srui-data >"$ROOT/override.log" 2>&1
status=$?
ok=1; [ "$status" -eq 0 ] && grep -q '^WARNING --allow-existing-srui-data' "$O/result.txt" \
  && [ "$(sha "$O/Library/Application Support/SRUI/saved-connections.json")" = "$saved_before" ] && ok=0
check "walkthrough --allow-existing-srui-data: proceeds, warns, and keeps saved connections it did not create" [ "$ok" -eq 0 ]
[ "$ok" -eq 0 ] || sed 's/^/  | /' "$ROOT/override.log" | tail -20

# ---------------------------------------------------------------------------------------------
echo "== ssh-config-alias.sh"
ALIAS=$R1_DIR/ssh-config-alias.sh
BEGIN_MARK="# >>> srui-r1 checklist alias (added by apps/srtop/r1/checklist-session.sh; removed when it ends) >>>"
config_state() { # home: existence, mode and digest of ~/.ssh and ~/.ssh/config
  local c=$1/.ssh/config
  if [ -L "$c" ]; then echo "link $(readlink "$c") $(sha "$1/real-config") $(stat -f '%Lp' "$1/real-config")"
  elif [ -f "$c" ]; then echo "file $(stat -f '%Lp' "$c") $(sha "$c") inode=$(stat -f '%i' "$c") dir=$([ -d "$1/.ssh" ] && echo yes)"
  else echo "absent dir=$([ -d "$1/.ssh" ] && echo yes || echo no)"; fi
}
alias_cycle() { # name setup-command...
  local name=$1 home=$ROOT/alias-$1 state before after added=1
  shift
  mkdir -p "$home"
  (cd "$home" && "$@")
  state=$(mktemp -d "$ROOT/alias-state.XXXXXX")
  before=$(config_state "$home")
  HOME=$home bash "$ALIAS" add "$state" 2222 "$home/.codex/srtop-devbox" >/dev/null 2>&1 || added=0
  grep -qxF "$BEGIN_MARK" "$home/.ssh/config" 2>/dev/null || added=0
  HOME=$home bash "$ALIAS" remove "$state" >/dev/null 2>&1
  after=$(config_state "$home")
  ok=1; [ "$added" -eq 1 ] && [ "$before" = "$after" ] && ok=0
  check "ssh-config-alias: $name config is byte for byte, mode and existence what it was ($after)" [ "$ok" -eq 0 ]
}
alias_cycle absent true
alias_cycle empty sh -c 'mkdir -m 700 .ssh && : > .ssh/config && chmod 644 .ssh/config'
alias_cycle no-final-newline sh -c 'mkdir -m 700 .ssh && printf "Host a\n  User x" > .ssh/config && chmod 644 .ssh/config'
alias_cycle ordinary sh -c 'mkdir -m 700 .ssh && printf "Host a\n  User x\n\nHost b\n  Port 2200\n" > .ssh/config && chmod 644 .ssh/config'
alias_cycle mode-0600 sh -c 'mkdir -m 700 .ssh && printf "Host a\n  User x\n" > .ssh/config && chmod 600 .ssh/config'

refused_alias() { # name setup-command...
  local name=$1 home=$ROOT/alias-$1 state before after status
  shift
  mkdir -p "$home"
  (cd "$home" && "$@")
  state=$(mktemp -d "$ROOT/alias-state.XXXXXX")
  before=$(config_state "$home")
  HOME=$home bash "$ALIAS" add "$state" 2222 "$home/.codex/srtop-devbox" >"$ROOT/alias-$name.log" 2>&1
  status=$?
  HOME=$home bash "$ALIAS" remove "$state" >/dev/null 2>&1
  after=$(config_state "$home")
  ok=1; [ "$status" -ne 0 ] && [ "$before" = "$after" ] && ok=0
  check "ssh-config-alias: refuses ${REFUSED_LABEL:-$name}, nothing changed ($(head -1 "$ROOT/alias-$name.log"))" [ "$ok" -eq 0 ]
}
refused_alias a-symlinked-config sh -c 'mkdir -m 700 .ssh && printf "Host a\n" > real-config && ln -s ../real-config .ssh/config'
refused_alias an-existing-block sh -c "mkdir -m 700 .ssh && printf '%s\n' 'Host a' '$BEGIN_MARK' > .ssh/config"
refused_alias a-symlinked-ssh-directory sh -c 'mkdir -m 700 real-ssh && printf "Host a\n" > real-ssh/config && ln -s real-ssh .ssh'
# Every way ssh_config(5) lets a Host or Match line name srtop-devbox is refused (round 4, V2-F1).
host_case=0
for line in 'Host srtop-devbox' 'Host\tsrtop-devbox' 'host srtop-devbox' '  Host srtop-devbox' \
            'Host srtop-devbox other' 'Host other srtop-devbox' 'Host=srtop-devbox' 'Host = srtop-devbox' \
            'HOST SRTOP-DEVBOX' 'Host "srtop-devbox"' 'Match host srtop-devbox' 'Match host a,srtop-devbox'; do
  host_case=$((host_case + 1))
  REFUSED_LABEL="the line [$line]" refused_alias "host-line-$host_case" sh -c 'mkdir -m 700 .ssh && printf "Host a\n  User x\n$1\n  Port 1\n" > .ssh/config' _ "$line"
done
# Near misses are not srtop-devbox: the alias is added and removed again byte for byte.
alias_cycle near-miss-srtop-devbox2 sh -c 'mkdir -m 700 .ssh && printf "Host srtop-devbox2\n  Port 1\n" > .ssh/config'
alias_cycle near-miss-hostname-option sh -c 'mkdir -m 700 .ssh && printf "Host a\n  HostName srtop-devbox\n" > .ssh/config'
alias_cycle near-miss-comment sh -c 'mkdir -m 700 .ssh && printf "# Host srtop-devbox\nHost a\n" > .ssh/config'

# Edited during the session: only the block goes, the user's own later lines stay.
E=$ROOT/alias-edited
mkdir -p "$E/.ssh" && chmod 700 "$E/.ssh"
printf 'Host a\n  User x\n' > "$E/.ssh/config"
state=$(mktemp -d "$ROOT/alias-state.XXXXXX")
HOME=$E bash "$ALIAS" add "$state" 2222 "$E/.codex/srtop-devbox" >/dev/null 2>&1
printf 'Host added-later\n  Port 2201\n' >> "$E/.ssh/config"
HOME=$E bash "$ALIAS" remove "$state" >/dev/null 2>&1
expected=$(printf 'Host a\n  User x\nHost added-later\n  Port 2201\n' | shasum -a 256 | awk '{print $1}')
check "ssh-config-alias: a config edited during the session loses only the block" [ "$(sha "$E/.ssh/config")" = "$expected" ]

echo "selftest: $((CASES - FAILURES)) of $CASES cases passed"
[ "$FAILURES" -eq 0 ]
