#!/usr/bin/env bash
# PX-027: the release-notes claim check. R1 is a read-only showcase, not htop parity, and a
# measured comparison with htop never becomes a claim that SRUI is smaller, better or the same.
#
#   apps/srtop/r1/notes-lint.sh [FILE...]   exit 0 clean, 1 a claim was found, 2 a file is missing
#                                           (default: the R1 notes and the READMEs that describe R1)
#   apps/srtop/r1/notes-lint.sh --self-test plants every claim below and its allowed counterparts,
#                                           and fails unless every claim is caught and every
#                                           allowed statement passes
#
# Each line is cut into clauses at , ; : ( ) ! ? at a full stop followed by a space, and at a dash
# between spaces. A clause is a claim when it says, about htop or an ncurses program:
#   parity       parity, at parity, on par with, on a par with (parity needs no htop in the clause);
#   equivalence  matches, equals, equal to, equivalent to, as good as, as capable as, comparable
#                to, rivals, the same as;
#   replacement  replaces, replacement, supersedes, substitute, drop-in, successor to, obsoletes;
#   superiority  superior, superiority, outperforms, beats, better than;
#   universal    always, automatically, inherently, by design, in every case, regardless of ...
#                less, fewer, smaller, lighter, cheaper, lower (no htop needed);
#   comparative  less, fewer, smaller, lighter, cheaper, lower, leaner, more (bandwidth-)efficient
#                ... than / vs / versus / compared to ... htop or ncurses, or htop ... larger,
#                bigger, heavier, costlier ... than.
# A claim clause passes only when that same clause negates it (not, no, none, nothing, never,
# without, nor, cannot, any n't, out of scope; "no matter" and "not only" are not negations), so
# "a replacement for htop, not a toy" is a claim and "R1 is a read-only showcase, not htop
# parity" is not. A comparative clause also passes when its sentence is explicitly qualified as
# measured (measured, matched, median): the published comparison may say what was measured. Lines under a
# Markdown heading that names "Out of scope" list what R1 does not claim and are not checked. The
# R1 notes must also call R1 a read-only showcase. Bash 3.2 and BSD awk compatible.
set -euo pipefail

self="${BASH_SOURCE[0]}"
R1_DIR=$(cd "$(dirname "$self")" && pwd)
REPO_ROOT=$(cd "$R1_DIR/../../.." && pwd)
NOTES=apps/srtop/srui-process-explorer-plan/docs/process-explorer/releases/R1.md

# Prints "CLAIM file:line" for every claim clause, and exits 1 if there was one.
LINT_AWK='
function negated(c) {
  gsub(/no matter|not only/, "", c)
  return c ~ /(^|[^a-z])(not|no|none|nothing|never|without|nor|cannot)([^a-z]|$)/ \
      || c ~ /n.t([^a-z]|$)/ || c ~ /out of scope/
}
function measured(c) { return c ~ /(^|[^a-z])(measured|matched|median|medians)([^a-z]|$)/ }
# then(a, b): word list a, later in the same clause word list b, each a whole word.
function then(a, b) { return "(^|[^a-z])" a "[^a-z]([^,;]*[^a-z])?" b "([^a-z]|$)" }
function strong(c,   t) {
  t = "(htop|ncurses)"
  if (c ~ /(^|[^a-z])(parity|on par with|on a par with|at par)([^a-z]|$)/) return 1
  if (c ~ then("(match|matches|matching|equals|equal to|equivalent to|as good as|as capable as|comparable to|rival|rivals|the same as)", t)) return 1
  if (c ~ then("(replace|replaces|replacing|replacement|supersede|supersedes|substitute|drop-in|successor to|obsoletes)", t)) return 1
  if (c ~ then(t, "(replacement|substitute|successor)")) return 1
  if (c ~ then("(superior|superiority|outperforms|outperform|beats|beat|better than)", t)) return 1
  if (c ~ then("(always|automatically|inherently|by design|in every case|regardless of)", "(less|fewer|smaller|lighter|cheaper|lower)")) return 1
  return 0
}
function comparative(c,   t) {
  t = "(htop|ncurses)"
  if (c ~ then("(less|fewer|smaller|lighter|cheaper|lower|leaner|more efficient|more bandwidth-efficient|more bandwidth efficient)", "(than|vs|versus|compared to|compared with)") && c ~ then("(than|vs|versus|compared to|compared with)", t)) return 1
  if (c ~ then(t, "(larger|bigger|heavier|costlier)") && c ~ then("(larger|bigger|heavier|costlier)", "than")) return 1
  return 0
}
/^#+[ \t]/ { exempt = (tolower($0) ~ /out of scope/); next }
{
  if (exempt) next
  line = tolower($0)
  gsub(/[.!?] /, "\n", line)
  ns = split(line, sentences, "\n")
  hit = 0
  for (s = 1; s <= ns && !hit; s++) {
    sentence = sentences[s]
    clause_text = sentence
    gsub(/ (\xe2\x80\x94|--|-) /, "\n", clause_text)
    gsub(/[,;:()]/, "\n", clause_text)
    n = split(clause_text, clauses, "\n")
    for (i = 1; i <= n; i++) {
      c = clauses[i]
      # Negation counts only in the claim clause; a measured qualifier anywhere in its sentence.
      if ((strong(c) && !negated(c)) || (comparative(c) && !negated(c) && !measured(sentence))) {
        hit = 1
        break
      }
    }
  }
  if (hit) { print "CLAIM " FILENAME ":" FNR; found = 1 }
}
END { exit found }
'

lint() { # files...
  local file found=0
  for file in "$@"; do
    [ -f "$file" ] || { echo "notes-lint: no such file: $file" >&2; return 2; }
  done
  # awk exits 1 for a claim; anything else non-zero is the lint's own failure, never a pass.
  LC_ALL=C awk "$LINT_AWK" "$@" || {
    local status=$?
    [ "$status" -eq 1 ] || { echo "notes-lint: awk failed with exit $status" >&2; return 2; }
    found=1
  }
  for file in "$@"; do
    case "$file" in
      *releases/R1.md)
        grep -qi 'read-only showcase' "$file" || { echo "MISSING $file: R1 is not called a read-only showcase"; found=1; }
        ;;
    esac
  done
  return "$found"
}

self_test() {
  local dir status=0 claim allowed
  dir=$(mktemp -d "${TMPDIR:-/tmp}/srui-notes-lint.XXXXXX")
  # shellcheck disable=SC2064 # the directory is fixed now.
  trap "rm -rf '$dir'" RETURN
  for claim in \
    'R1 reaches htop parity.' \
    'The explorer is an htop replacement.' \
    'SRUI replaces htop for remote monitoring.' \
    'SRUI always uses less bandwidth than htop.' \
    'Semantic replication is automatically smaller than an ncurses screen.' \
    'It is at parity with htop.' \
    'SRUI uses less bandwidth than htop.' \
    'srtop matches htop for remote monitoring.' \
    'srtop is on par with htop.' \
    'srtop is equivalent to htop for everyday use.' \
    'srtop supersedes htop.' \
    'srtop needs fewer bytes than htop.' \
    'SRUI is more bandwidth-efficient than htop.' \
    'srtop is a full replacement for htop, not a toy.' \
    'SRUI is always smaller, no matter the workload.' \
    'SRUI is always smaller than htop no matter the workload.' \
    'Semantic replication outperforms an ncurses program.' \
    'htop is larger than SRUI on the wire.' \
    'Not a toy: srtop is a drop-in for htop.' \
    'It sends fewer bytes than htop (see below), not only on idle hosts.'; do
    printf '# R1\nR1 is a read-only showcase.\n%s\n' "$claim" >"$dir/claim.md"
    if lint "$dir/claim.md" >/dev/null; then
      echo "notes-lint self-test: missed the claim: $claim" >&2
      status=1
    fi
  done
  for allowed in \
    'R1 is a read-only showcase, not htop parity.' \
    'Full htop parity is out of scope.' \
    'It does not replace htop.' \
    'SRUI is not automatically smaller than htop; see the measured table.' \
    'Measured: in the idle workload htop sent fewer bytes.' \
    "In the matched steady-state workloads below, SRUI's median was 0.41–0.50 of htop's." \
    "In the matched workloads, SRUI's encrypted stream was smaller than htop's." \
    'PX-027 does not claim automatic bandwidth superiority over an ncurses application.' \
    'None of this makes SRUI automatically smaller than an ncurses program.' \
    "In the steady state of these three workloads, htop's stream was the larger one." \
    'R1 is a read-only showcase, not an htop replacement.'; do
    printf '# R1\nR1 is a read-only showcase.\n%s\n' "$allowed" >"$dir/allowed.md"
    if ! lint "$dir/allowed.md" >/dev/null; then
      echo "notes-lint self-test: flagged an allowed statement: $allowed" >&2
      status=1
    fi
  done
  # An "Out of scope" section lists what is not claimed; the same words elsewhere are a claim.
  printf '%s\n' '# R1' 'R1 is a read-only showcase.' '## Out of scope' '- Full htop parity.' \
    '## Known gaps' '- none here' >"$dir/scope.md"
  lint "$dir/scope.md" >/dev/null || { echo "notes-lint self-test: flagged an Out of scope list" >&2; status=1; }
  printf '%s\n' '# R1' 'R1 is a read-only showcase.' '## Scope' '- Full htop parity.' >"$dir/in-scope.md"
  if lint "$dir/in-scope.md" >/dev/null; then
    echo "notes-lint self-test: missed a parity claim under Scope" >&2
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
