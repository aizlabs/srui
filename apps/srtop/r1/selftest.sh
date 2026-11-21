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
#    prerequisite makes its step FAIL and the summary FAIL; an edited result fails --verify.
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
# Docker stand-ins: a running daemon, and none.
printf '#!/bin/sh\ncase "$*" in version*) echo "28.3.2 6.10.14-linuxkit linux/arm64" ;; esac\nexit 0\n' > "$STUBS/pass/docker"
printf '#!/bin/sh\necho "Cannot connect to the Docker daemon" >&2\nexit 1\n' > "$STUBS/fail/docker"
chmod 755 "$STUBS/pass/docker" "$STUBS/fail/docker"

walk() { # stubs-dir args...
  local stubs=$1
  shift
  HOME=$W PATH="$stubs:$PATH" TMPDIR=$W/tmp bash "$R1_DIR/clean-account-walkthrough.sh" "$@"
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
sandboxed_walk() {
  if [ -n "$SANDBOX" ]; then
    HOME=$W PATH="$1:$PATH" TMPDIR=$W/tmp sandbox-exec -p "$SANDBOX" bash "$R1_DIR/clean-account-walkthrough.sh" --dry-run --output "$2"
  else
    walk "$1" --dry-run --output "$2"
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
ok=1; walk "$STUBS/pass" --verify "$W/result-fail.txt" >"$ROOT/verify-fail.log" 2>&1 || ok=0
grep -q '^verify: OK checksum matches; SUMMARY FAIL ' "$ROOT/verify-fail.log" || ok=1
check "walkthrough --verify: the failed result's checksum holds and its verdict fails" [ "$ok" -eq 0 ]
sed 's/^STEP 2 prerequisites FAIL/STEP 2 prerequisites PASS/' "$W/result-fail.txt" > "$W/result-edited.txt"
ok=1; walk "$STUBS/pass" --verify "$W/result-edited.txt" >"$ROOT/verify-edited.log" 2>&1 || ok=0
grep -q 'checksum mismatch' "$ROOT/verify-edited.log" || ok=1
check "walkthrough --verify: an edited result is refused" [ "$ok" -eq 0 ]

echo "selftest: $((CASES - FAILURES)) of $CASES cases passed"
[ "$FAILURES" -eq 0 ]
