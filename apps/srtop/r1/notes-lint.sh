#!/usr/bin/env bash
# PX-027: the release-notes claim check. R1 is a read-only showcase, not htop parity, and a
# measured comparison with htop never becomes a claim that SRUI is automatically smaller.
#
#   apps/srtop/r1/notes-lint.sh [FILE...]   exit 0 clean, 1 a claim was found, 2 a file is missing
#                                           (default: the R1 notes and the READMEs that describe R1)
#   apps/srtop/r1/notes-lint.sh --self-test plants each claim and its negation, and fails unless
#                                           every claim is caught and every negation passes
#
# A line is a claim when it says R1 is at parity with, replaces, or is a replacement for htop, or
# that SRUI is always, automatically or inherently smaller, lighter or cheaper on the wire than
# htop or an ncurses program, unless the same line negates it (not, no, never, without, nor,
# out of scope, does not, is not). The R1 notes must also call R1 a read-only showcase.
# Bash 3.2 compatible.
set -euo pipefail

self="${BASH_SOURCE[0]}"
R1_DIR=$(cd "$(dirname "$self")" && pwd)
REPO_ROOT=$(cd "$R1_DIR/../../.." && pwd)
NOTES=apps/srtop/srui-process-explorer-plan/docs/process-explorer/releases/R1.md

CLAIM='(htop[- ]parity|parity with htop|at parity|feature[- ]parity|(replaces|replacement for|substitute for|drop-in for) htop|htop replacement|(always|automatically|inherently|by design) (uses? )?(less|fewer|smaller|lighter|cheaper|lower)[a-z ]*(than|bandwidth|bytes|traffic)|(beats|outperforms) htop)'
NEGATION='(^|[^a-z])(not|no|never|without|nor|isn.t|doesn.t|does not|is not|out of scope|do not|cannot)([^a-z]|$)'

lint() { # files...
  local file found=0 line number
  for file in "$@"; do
    [ -f "$file" ] || { echo "notes-lint: no such file: $file" >&2; return 2; }
    number=0
    while IFS= read -r line || [ -n "$line" ]; do
      number=$((number + 1))
      if printf '%s\n' "$line" | grep -Eiq "$CLAIM" && ! printf '%s\n' "$line" | grep -Eiq "$NEGATION"; then
        echo "CLAIM $file:$number"
        found=1
      fi
    done <"$file"
    case "$file" in
      *releases/R1.md)
        grep -qi 'read-only showcase' "$file" || { echo "MISSING $file: R1 is not called a read-only showcase"; found=1; }
        ;;
    esac
  done
  return "$found"
}

self_test() {
  local dir status=0 claim
  dir=$(mktemp -d "${TMPDIR:-/tmp}/srui-notes-lint.XXXXXX")
  # shellcheck disable=SC2064 # the directory is fixed now.
  trap "rm -rf '$dir'" RETURN
  for claim in \
    'R1 reaches htop parity.' \
    'The explorer is an htop replacement.' \
    'SRUI replaces htop for remote monitoring.' \
    'SRUI always uses less bandwidth than htop.' \
    'Semantic replication is automatically smaller than an ncurses screen.' \
    'It is at parity with htop.'; do
    printf '# R1\nR1 is a read-only showcase.\n%s\n' "$claim" >"$dir/claim.md"
    if lint "$dir/claim.md" >/dev/null; then
      echo "notes-lint self-test: missed the claim: $claim" >&2
      status=1
    fi
  done
  printf '%s\n' '# R1' 'R1 is a read-only showcase, not htop parity.' \
    'Full htop parity is out of scope.' 'It does not replace htop.' \
    'SRUI is not automatically smaller than htop; see the measured table.' \
    'Measured: in the idle workload htop sent fewer bytes.' >"$dir/clean.md"
  if ! lint "$dir/clean.md" >/dev/null; then
    echo "notes-lint self-test: flagged a negated statement" >&2
    status=1
  fi
  mkdir -p "$dir/releases"
  printf '%s\n' '# R1' 'The installable explorer.' >"$dir/releases/R1.md"
  if lint "$dir/releases/R1.md" >/dev/null; then
    echo "notes-lint self-test: R1 notes without \"read-only showcase\" passed" >&2
    status=1
  fi
  [ "$status" -eq 0 ] && echo "notes-lint self-test: ok"
  return "$status"
}

if [ "${1:-}" = --self-test ]; then
  self_test
  exit $?
fi
if [ "$#" -eq 0 ]; then
  set -- "$REPO_ROOT/$NOTES" "$REPO_ROOT/apps/srtop/README.md" "$REPO_ROOT/apps/srtop/r1/README.md"
fi
lint "$@"
