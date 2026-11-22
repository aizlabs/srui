#!/usr/bin/env bash
# PX-027: a backstop for the release notes' claims about htop. R1 is a read-only showcase, not
# htop parity, and a measured comparison with htop never becomes a claim that SRUI is smaller,
# better or the same. This is a regular-expression lint over English: it catches the claim forms
# listed below and nothing else. The notes' wording is reviewed by a person; the lint does not
# guarantee it.
#
#   apps/srtop/r1/notes-lint.sh [FILE...]   exit 0 clean, 1 a claim was found, 2 a file is missing
#                                           or awk failed (default: the R1 notes and the READMEs
#                                           that describe R1)
#   apps/srtop/r1/notes-lint.sh --self-test plants every claim form below and its allowed
#                                           counterparts, and fails unless every planted claim is
#                                           caught and every allowed statement passes
#
# Paragraphs, not lines: consecutive non-blank lines are joined, so a claim hard-wrapped across lines
# is read whole. A heading, a blank line, each list item and each table row start a new one.
# Each paragraph is cut into sentences at . ! ? followed by a space, and each sentence into clauses
# at , ; : ( ) and at a dash between spaces. A target is htop, top, btop, glances, ncurses, or a
# terminal monitor (whole words). A clause is a claim when it says:
#   parity       parity, at parity, on par with, on a par with (no target needed);
#   equivalence  matches, equals, equal to, equivalent to, as good as, as capable as, comparable
#                to, competitive with, rivals, the same as ... a target;
#   replacement  replaces, replacement, supersedes, substitute, drop-in, successor to, obsoletes
#                ... a target, or a target ... replacement, substitute, successor;
#   superiority  superior, superiority, outperforms, outclasses, outdoes, surpasses, beats, better
#                than ... a target;
#   universal    always, automatically, inherently, by design, in every case, regardless of ...
#                less, fewer, smaller, lighter, cheaper, lower (no target needed);
#   comparative  less, fewer, smaller, lighter, cheaper, lower, leaner, faster, more (bandwidth-)
#                efficient ... than / vs / versus / compared to ... a target; or, reversed, a
#                target ... more, larger, bigger, heavier, costlier, slower ... than.
# Negation counts only inside the claim clause, and only the whole words not, no, never and any
# n't (doesn't, isn't). "not just", "not merely", "not simply", "not only" and "no matter" are not
# negations. So "a replacement for htop, not a toy" is a claim, and "R1 is a read-only showcase,
# not htop parity" is not. A comparative clause also passes when its sentence says it was measured
# (measured, matched, median). Under a heading that is exactly "Out of scope", the list items are
# what R1 does not claim and are not checked; prose there is. The R1 notes must also call R1 a
# read-only showcase. Text inside an inline `code` span is not read, so a claim written in one is
# not caught. Bash 3.2 and BSD awk compatible.
set -euo pipefail

self="${BASH_SOURCE[0]}"
R1_DIR=$(cd "$(dirname "$self")" && pwd)
REPO_ROOT=$(cd "$R1_DIR/../../.." && pwd)
NOTES=apps/srtop/srui-process-explorer-plan/docs/process-explorer/releases/R1.md

# Prints "CLAIM file:line" for every claim clause, and exits 1 if there was one.
LINT_AWK='
function negated(c) {
  gsub(/not just|not merely|not simply|not only|no matter/, "", c)
  return c ~ /(^|[^a-z])(not|no|never)([^a-z]|$)/ || c ~ /[a-z]n(\047|\xe2\x80\x99)t([^a-z]|$)/
}
function measured(c) { return c ~ /(^|[^a-z])(measured|matched|median|medians)([^a-z]|$)/ }
# then(a, b): word list a, later in the same clause word list b, each a whole word.
function then(a, b) { return "(^|[^a-z])" a "[^a-z]([^,;]*[^a-z])?" b "([^a-z]|$)" }
function strong(c,   t) {
  t = "(htop|top|btop|glances|ncurses|terminal monitors?)"
  if (c ~ /(^|[^a-z])(parity|on par with|on a par with|at par)([^a-z]|$)/) return 1
  if (c ~ then("(match|matches|matching|equals|equal to|equivalent to|as good as|as capable as|comparable to|competitive with|rival|rivals|the same as)", t)) return 1
  if (c ~ then("(replace|replaces|replacing|replacement|supersede|supersedes|substitute|drop-in|successor to|obsoletes)", t)) return 1
  if (c ~ then(t, "(replacement|substitute|successor)")) return 1
  if (c ~ then("(superior|superiority|outperforms|outperform|outclasses|outdoes|surpasses|beats|beat|better than)", t)) return 1
  if (c ~ then("(always|automatically|inherently|by design|in every case|regardless of)", "(less|fewer|smaller|lighter|cheaper|lower)")) return 1
  return 0
}
function comparative(c,   t, k) {
  t = "(htop|top|btop|glances|ncurses|terminal monitors?)"
  k = "(than|vs|versus|compared to|compared with)"
  if (c ~ then("(less|fewer|smaller|lighter|cheaper|lower|leaner|faster|more efficient|more bandwidth-efficient|more bandwidth efficient)", k) && c ~ then(k, t)) return 1
  if (c ~ then(t, "(more|larger|bigger|heavier|costlier|slower)") && c ~ then("(more|larger|bigger|heavier|costlier|slower)", "than")) return 1
  return 0
}
function check(text,   line, ns, s, sentence, clause_text, n, i, c) {
  line = tolower(text)
  # Inline code spans are quotations (command names, the forms this lint looks for), not prose.
  gsub(/`[^`]*`/, "", line)
  gsub(/[.!?] /, "\n", line)
  ns = split(line, sentences, "\n")
  for (s = 1; s <= ns; s++) {
    sentence = sentences[s]
    clause_text = sentence
    gsub(/ (\xe2\x80\x94|--|-) /, "\n", clause_text)
    gsub(/[,;:()]/, "\n", clause_text)
    n = split(clause_text, clauses, "\n")
    for (i = 1; i <= n; i++) {
      c = clauses[i]
      # Negation counts only in the claim clause; a measured qualifier anywhere in its sentence.
      if ((strong(c) && !negated(c)) || (comparative(c) && !negated(c) && !measured(sentence))) return 1
    }
  }
  return 0
}
function flush() {
  if (para != "" && !(exempt && item) && check(para)) { print "CLAIM " pfile ":" start; found = 1 }
  para = ""
}
FNR == 1 { flush(); exempt = 0 }
/^[ \t]*$/ { flush(); next }
/^#+[ \t]/ {
  flush()
  heading = tolower($0); sub(/^#+[ \t]+/, "", heading); sub(/[ \t#]+$/, "", heading)
  exempt = (heading == "out of scope")
  next
}
/^[ \t]*([-*+]|[0-9]+\.)[ \t]/ { flush(); para = $0; start = FNR; pfile = FILENAME; item = 1; next }
/^[ \t]*\|/ { flush(); para = $0; start = FNR; pfile = FILENAME; item = 0; next }
{
  if (para == "") { para = $0; start = FNR; pfile = FILENAME; item = 0 } else para = para " " $0
}
END { flush(); exit found }
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
    'It sends fewer bytes than htop (see below), not only on idle hosts.' \
    'srtop is competitive with htop.' \
    'SRUI uses less bandwidth than terminal monitors.' \
    'SRUI is not just smaller than htop.' \
    'SRUI is faster than htop.' \
    'htop needs more bandwidth than SRUI.' \
    'htop uses more bytes than SRUI does.' \
    'SRUI uses less bandwidth than htop on the internet.' \
    'srtop beats htop against every workload.' \
    'srtop is not merely as good as htop.' \
    'srtop is as good as btop.' \
    'srtop surpasses glances.' \
    'srtop sends fewer bytes than top.' \
    'srtop is not simply lighter than htop.' \
    'Unit tests show srtop is cheaper than htop.'; do
    printf '# R1\nR1 is a read-only showcase.\n%s\n' "$claim" >"$dir/claim.md"
    if lint "$dir/claim.md" >/dev/null; then
      echo "notes-lint self-test: missed the claim: $claim" >&2
      status=1
    fi
  done
  # A claim hard-wrapped across two lines of one paragraph, or of one list item.
  printf '%s\n' '# R1' 'R1 is a read-only showcase.' '' 'SRUI uses less bandwidth' 'than htop.' >"$dir/wrapped.md"
  if lint "$dir/wrapped.md" >/dev/null; then echo "notes-lint self-test: missed a wrapped claim" >&2; status=1; fi
  printf '%s\n' '# R1' 'R1 is a read-only showcase.' '- In short, srtop is a drop-in' '  replacement for htop.' >"$dir/wrapped-item.md"
  if lint "$dir/wrapped-item.md" >/dev/null; then echo "notes-lint self-test: missed a wrapped list item" >&2; status=1; fi
  # A heading that only contains the phrase exempts nothing.
  printf '%s\n' '# R1' 'R1 is a read-only showcase.' '## Why it is no longer out of scope' '- srtop beats htop.' >"$dir/heading.md"
  if lint "$dir/heading.md" >/dev/null; then echo "notes-lint self-test: a heading containing \"out of scope\" exempted a claim" >&2; status=1; fi
  # Under "Out of scope" only the list is exempt: prose there is checked.
  printf '%s\n' '# R1' 'R1 is a read-only showcase.' '## Out of scope' '- Full htop parity.' '' 'Even so, srtop beats htop.' >"$dir/scope-prose.md"
  if lint "$dir/scope-prose.md" >/dev/null; then echo "notes-lint self-test: prose under Out of scope was exempted" >&2; status=1; fi
  for allowed in \
    'R1 is a read-only showcase, not htop parity.' \
    'Full htop parity is not in scope.' \
    'It does not replace htop.' \
    "It doesn't replace htop." \
    'SRUI is not automatically smaller than htop; see the measured table.' \
    'Measured: in the idle workload htop sent fewer bytes.' \
    "In the matched steady-state workloads below, SRUI's median was 0.41–0.50 of htop's." \
    "In the matched workloads, SRUI's encrypted stream was smaller than htop's." \
    'PX-027 does not claim automatic bandwidth superiority over an ncurses application.' \
    'This does not make SRUI automatically smaller than an ncurses program.' \
    "In the steady state of these three workloads, htop's stream was the larger one." \
    'R1 is a read-only showcase, not an htop replacement.' \
    'The relay sits on top of the SSH stream.' \
    'htop repaints its meters every second.'; do
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
