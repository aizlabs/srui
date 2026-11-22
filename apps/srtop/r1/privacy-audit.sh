#!/usr/bin/env bash
# PX-026-G01: the repeatable privacy and disclosure audit of R1's artifacts and settings
# (D1 §§25-27). Scans files, or every regular file under a directory, for key material, tokens,
# passwords and private paths, and names each finding by class and location only: the matched text
# is never printed, so the audit's own output cannot leak what it found.
#
#   apps/srtop/r1/privacy-audit.sh [--file FILE | PATH]...
#                                             exit 0 clean, 1 findings, 2 not fully audited: a
#                                             path is missing or cannot be read (any finding
#                                             is still printed)
#   apps/srtop/r1/privacy-audit.sh --self-test
#                                             plants one secret of every class (built at run
#                                             time, so this file holds none) and fails unless
#                                             each is found and a clean file passes
#
# Classes:
#   private-key   PEM/OpenSSH private key blocks, PuTTY key files
#   token         GitHub, GitLab, AWS, Slack and OpenAI-style tokens, JWTs, bearer credentials
#   password      password/passwd/passphrase/secret/pwd assignments, credentials in a URL
#   private-path  the auditing user's home directory ($HOME, wherever it is), any /Users/<name>/
#                 path but /Users/Shared, and any Linux /home/<name>/ path but /home/srui, the
#                 devbox's own throwaway account
# A binary file (one with a NUL byte) is scanned for the first three classes only: a from-source
# build embeds the builder's own checkout path in its binaries, which is reported once per binary
# as BINARY-PATH information, not as a finding. Bash 3.2 compatible (the macOS /bin/bash).
#
# Nothing is passed without being read (PX-026-G01, Codex review of PR #117). Each of these is an
# ERROR line naming the path, and the audit exits 2:
#   - a named PATH that does not exist (MISSING): a caller names only artifacts it requires;
#   - a named PATH that is a dangling symbolic link, or not a regular file or directory (a FIFO,
#     a device, a socket): it is refused without being opened, so a FIFO cannot block the audit;
#   - --file FILE that is not a regular file (a directory where a file was expected);
#   - a regular file that cannot be read: no read permission, or a read error from grep (exit 2
#     or more; exit 1 is "no match" and means clean), for example /proc/1/mem;
#   - any error while walking a directory (an unreadable or unsearchable subdirectory), and any
#     entry in it that is a FIFO, device, socket or dangling symbolic link.
# A symbolic link inside a walked directory that resolves to an existing file or directory is not
# followed and reported as SKIP: the link is not the data, and its target is audited only when it
# is itself inside an audited path. A named symbolic link to a file or directory is followed.
set -euo pipefail

self="${BASH_SOURCE[0]}"

# class <TAB> grep flags <TAB> extended regular expression. printf, not a here-document: bash 3.2
# writes here-documents to /tmp whatever TMPDIR says, and the walkthrough writes nowhere but home.
rules() {
  printf '%s\n' \
    'private-key	-E	-----BEGIN ([A-Z0-9]+ )*PRIVATE KEY( BLOCK)?-----' \
    'private-key	-E	PuTTY-User-Key-File-[0-9]+:' \
    'token	-E	gh[pousr]_[A-Za-z0-9]{36}' \
    'token	-E	github_pat_[A-Za-z0-9_]{22,}' \
    'token	-E	glpat-[A-Za-z0-9_-]{20,}' \
    'token	-E	(AKIA|ASIA)[0-9A-Z]{16}' \
    'token	-E	xox[abprs]-[A-Za-z0-9-]{10,}' \
    'token	-E	(^|[^A-Za-z0-9])sk-[A-Za-z0-9_-]{20,}' \
    'token	-E	eyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.' \
    'token	-Ei	bearer [A-Za-z0-9._~+/-]{20,}' \
    'password	-Ei	(password|passwd|passphrase|secret|pwd)["'"'"']?[[:space:]]*[:=][[:space:]]*["'"'"']?[^[:space:]"'"'"',;)}%]{4,}' \
    'password	-Ei	[a-z][a-z0-9+.-]*://[^/[:space:]:@]+:[^/[:space:]@]+@'
}

findings=0
scanned=0
binaries=0
errors=0

finding() { # class file line
  findings=$((findings + 1))
  echo "FINDING $1 $2:$3"
}

# One line per path that could not be audited, in a fixed shape: the reason is the audit's own
# words, never the file's content.
read_error() { # path reason
  errors=$((errors + 1))
  echo "ERROR $1: $2"
}

# The matching lines of grep over one file in GREP_OUT, and its exit status in GREP_STATUS. grep
# exits 1 when nothing matched, which is clean; 2 or more is a read error, which the caller
# reports. Its own stderr is dropped because the caller names the error itself.
grep_file() { # grep arguments..., the file last
  GREP_STATUS=0
  # NUL bytes from a binary file's matched lines are dropped here, not by the shell, which warns
  # about them on stderr (bash 5); the exit status kept is grep's own.
  GREP_OUT=$(LC_ALL=C grep "$@" 2>/dev/null | LC_ALL=C tr -d '\000'; exit "${PIPESTATUS[0]}") || GREP_STATUS=$?
}

# Line numbers (sorted, unique) of the grep matches in GREP_OUT, read as "N:..." lines.
match_lines() {
  [ -n "$GREP_OUT" ] || return 0
  printf '%s\n' "$GREP_OUT" | cut -d: -f1 | sort -un
}

scan_rule() { # file class flags regex; returns 2 on a read error
  local line
  grep_file -a -n $3 -e "$4" "$1"
  [ "$GREP_STATUS" -le 1 ] || return 2
  for line in $(match_lines); do
    finding "$2" "$1" "$line"
  done
}

scan_paths() { # file; returns 2 on a read error
  local line
  if [ -n "${HOME:-}" ] && [ "${#HOME}" -gt 1 ]; then
    grep_file -a -n -F -e "$HOME" "$1"
    [ "$GREP_STATUS" -le 1 ] || return 2
    for line in $(match_lines); do
      finding private-path "$1" "$line"
    done
  fi
  grep_file -a -n -o -E '/Users/[^/[:space:]"]+/' "$1"
  [ "$GREP_STATUS" -le 1 ] || return 2
  GREP_OUT=$(printf '%s\n' "$GREP_OUT" | grep -v ':/Users/Shared/$' || true)
  for line in $(match_lines); do
    finding private-path "$1" "$line"
  done
  grep_file -a -n -o -E '/home/[^/[:space:]"]+/' "$1"
  [ "$GREP_STATUS" -le 1 ] || return 2
  GREP_OUT=$(printf '%s\n' "$GREP_OUT" | grep -v ':/home/srui/$' || true)
  for line in $(match_lines); do
    finding private-path "$1" "$line"
  done
}

scan_file() { # file: a regular file, already checked to be one
  local class flags regex binary="" rule
  scanned=$((scanned + 1))
  # No access(2) pre-check: grep's own exit status is the one test of whether the file was read,
  # so permission errors and read errors (EIO from /proc/1/mem) take the same path. An empty file
  # is still read once, by the first rule.
  if [ -s "$1" ]; then
    # -I treats a binary file as matching nothing: exit 1 is binary, 0 text, 2 a read error.
    grep_file -I -q '' "$1"
    case "$GREP_STATUS" in
      0) ;;
      1) binary=1; binaries=$((binaries + 1)) ;;
      *) read_error "$1" "cannot be read (grep exit $GREP_STATUS)"; return 0 ;;
    esac
  fi
  local IFS='
'
  set -f
  for rule in $(rules); do
    class=${rule%%	*}
    rule=${rule#*	}
    flags=${rule%%	*}
    regex=${rule#*	}
    if ! scan_rule "$1" "$class" "$flags" "$regex"; then
      set +f
      unset IFS
      read_error "$1" "cannot be read (grep exit $GREP_STATUS)"
      return 0
    fi
  done
  set +f
  unset IFS
  if [ -z "$binary" ]; then
    scan_paths "$1" || read_error "$1" "cannot be read (grep exit $GREP_STATUS)"
  elif [ -n "${HOME:-}" ] && [ "${#HOME}" -gt 1 ]; then
    grep_file -a -q -F -e "$HOME" "$1"
    case "$GREP_STATUS" in
      0) echo "BINARY-PATH $1: embeds the builder's home directory (build paths; not a finding)" ;;
      1) ;;
      *) read_error "$1" "cannot be read (grep exit $GREP_STATUS)" ;;
    esac
  fi
}

# Walks a directory without following links inside it (find -P; -H follows only the named path).
# Each entry comes back tagged, and anything untagged is find's own error message, which is
# reported as a traversal error, as is a non-zero exit from find.
walk_dir() { # directory
  local listing status=0 entry kind
  listing=$(find -H "$1" \( -type d -o -type f -exec printf 'FILE %s\n' {} + \) \
              -o \( -type l -exec printf 'LINK %s\n' {} + \) \
              -o -exec printf 'OTHER %s\n' {} + 2>&1) || status=$?
  local IFS='
'
  set -f
  for entry in $(printf '%s\n' "$listing" | LC_ALL=C sort); do
    kind=${entry%% *}
    case "$kind" in
      FILE) scan_file "${entry#FILE }" ;;
      LINK)
        if [ -e "${entry#LINK }" ]; then
          echo "SKIP ${entry#LINK }: a symbolic link inside an audited directory is not followed"
        else
          read_error "${entry#LINK }" "dangling symbolic link"
        fi
        ;;
      OTHER) read_error "${entry#OTHER }" "not a regular file or directory (FIFO, device or socket); not opened" ;;
      *) read_error "$1" "traversal error: $entry" ;;
    esac
  done
  set +f
  unset IFS
  [ "$status" -eq 0 ] || read_error "$1" "traversal incomplete (find exit $status)"
}

audit() {
  local path missing=0 want_file
  [ "$#" -gt 0 ] || { echo "usage: privacy-audit.sh [--file FILE | PATH]... | --self-test" >&2; return 2; }
  while [ "$#" -gt 0 ]; do
    want_file=""
    if [ "$1" = --file ]; then
      want_file=1
      shift
      [ "$#" -gt 0 ] || { echo "usage: --file needs a path" >&2; return 2; }
    fi
    path=$1
    shift
    if [ -L "$path" ] && [ ! -e "$path" ]; then
      read_error "$path" "dangling symbolic link"
    elif [ ! -e "$path" ]; then
      echo "MISSING $path"
      missing=1
    elif [ -f "$path" ]; then
      scan_file "$path"
    elif [ -d "$path" ] && [ -z "$want_file" ]; then
      walk_dir "$path"
    elif [ -d "$path" ]; then
      read_error "$path" "a directory where a file was expected (--file)"
    else
      read_error "$path" "not a regular file or directory (FIFO, device or socket); not opened"
    fi
  done
  echo "privacy-audit: $scanned file(s) scanned ($binaries binary), $findings finding(s), $errors error(s)"
  if [ "$missing" -ne 0 ] || [ "$errors" -ne 0 ]; then
    echo "privacy-audit: FAIL (not fully audited: a path is missing or cannot be read)"
    return 2
  fi
  if [ "$findings" -ne 0 ]; then
    echo "privacy-audit: FAIL"
    return 1
  fi
  echo "privacy-audit: PASS"
}

random_chars() { # count alphabet
  # Bounded input, never an endless `tr </dev/urandom | head`: that one stops only on SIGPIPE, and
  # under a parent that ignores SIGPIPE it never stops (PX-026-G01 round 4, verifier V2-F2).
  local out=""
  while [ "${#out}" -lt "$1" ]; do
    out="$out$(head -c 1024 /dev/urandom | LC_ALL=C tr -dc "$2")"
  done
  printf '%s' "$out" | cut -c "1-$1"
}

self_test() {
  local dir failures=0 cases=0 name expected output status
  dir=$(mktemp -d "${TMPDIR:-/tmp}/srui-privacy-selftest.XXXXXX")
  # Removes only this test's own directory, on every exit path.
  # shellcheck disable=SC2064
  trap "chmod -R u+rwx '$dir' 2>/dev/null; rm -rf '$dir'" EXIT
  local alnum='A-Za-z0-9' upper='A-Z0-9'
  plant() { # name content
    printf 'line one\nsome context %s trailing\nline three\n' "$2" > "$dir/$1"
  }
  plant private-key-openssh "$(printf -- '-----BEGIN %s PRIVATE KEY-----' OPENSSH)"
  plant private-key-rsa "$(printf -- '-----BEGIN %s PRIVATE KEY-----' RSA)"
  plant private-key-putty "$(printf 'PuTTY-User-Key-File-%s: ssh-ed25519' 3)"
  plant token-github "ghp_$(random_chars 36 "$alnum")"
  plant token-github-pat "github_pat_$(random_chars 30 "$alnum")"
  plant token-gitlab "glpat-$(random_chars 20 "$alnum")"
  plant token-aws "AKIA$(random_chars 16 "$upper")"
  plant token-slack "xoxb-$(random_chars 24 "$alnum")"
  plant token-openai "sk-$(random_chars 32 "$alnum")"
  plant token-jwt "eyJ$(random_chars 16 "$alnum").eyJ$(random_chars 16 "$alnum").$(random_chars 16 "$alnum")"
  plant token-bearer "Authorization: Bearer $(random_chars 32 "$alnum")"
  plant password-flag "--password=$(random_chars 12 "$alnum")"
  plant password-json "\"passphrase\": \"$(random_chars 12 "$alnum")\""
  plant password-url "https://alice:$(random_chars 10 "$alnum")@example.invalid/x"
  # A home outside /Users and /home, so only the $HOME rule can find it. The path need not exist:
  # the rule matches the text of $HOME.
  local other_home
  other_home=/private/var/empty/srui-r1-selftest-home-$(random_chars 8 'a-z')
  plant private-path-home "$other_home/Library/plant.txt"
  plant private-path-linux "/home/$(random_chars 8 'a-z')/.config/plant.txt"
  plant private-path-users "/Users/$(random_chars 8 'a-z')/Documents/plant.txt"
  mkdir -p "$dir/nested/deeper"
  printf 'x %s y\n' "ghp_$(random_chars 36 "$alnum")" > "$dir/nested/deeper/token-nested"
  printf 'Read-only · Live process snapshot\nconnected to 127.0.0.1:2222 as srui\n/Users/Shared/ok\nsrtop listening on /home/srui/run/srtop.sock\nsession 0123456789abcdef0123456789abcdef\nTest run with 6 tests passed\n' > "$dir/clean"
  printf 'bin\000ary %s\000tail\n' "ghp_$(random_chars 36 "$alnum")" > "$dir/binary-token"
  printf 'bin\000ary %s\000tail\n' "${HOME:-/Users/nobody}/src/main.swift" > "$dir/binary-path"

  check() { # name expected-class-or-clean target [home]
    cases=$((cases + 1))
    set +e
    output=$(HOME=${4:-$HOME} bash "$self" "$3" 2>&1)
    status=$?
    set -e
    if [ "$2" = clean ]; then
      if [ "$status" -eq 0 ] && ! printf '%s\n' "$output" | grep -q '^FINDING '; then
        echo "PASS $1: clean"
      else
        echo "FAIL $1: expected clean, got status $status: $output"
        failures=$((failures + 1))
      fi
    elif [ "$status" -eq 1 ] && printf '%s\n' "$output" | grep -q "^FINDING $2 "; then
      # Every line the audit prints has a fixed shape, so none can carry what it found.
      if printf '%s\n' "$output" | grep -v -E '^(FINDING [a-z-]+ .+:[0-9]+|BINARY-PATH .+|MISSING .+|ERROR .+|SKIP .+|privacy-audit: .+)$' | grep -q .; then
        echo "FAIL $1: the audit printed free text"
        failures=$((failures + 1))
      else
        echo "PASS $1: found as $2"
      fi
    else
      echo "FAIL $1: expected a $2 finding, got status $status: $output"
      failures=$((failures + 1))
    fi
  }
  for name in private-key-openssh private-key-rsa private-key-putty token-github token-github-pat \
              token-gitlab token-aws token-slack token-openai token-jwt token-bearer \
              password-flag password-json password-url private-path-users private-path-linux; do
    expected=${name%%-*}
    case "$name" in
      private-key-*) expected=private-key ;;
      private-path-*) expected=private-path ;;
    esac
    check "$name" "$expected" "$dir/$name"
  done
  check private-path-home private-path "$dir/private-path-home" "$other_home"
  check nested-directory token "$dir/nested"
  check clean-file clean "$dir/clean"
  check binary-token token "$dir/binary-token"
  check binary-build-path clean "$dir/binary-path"
  # Inputs that cannot be audited fail with status 2 and an ERROR (or MISSING) line naming the
  # path: nothing unread counts as clean (Codex review of PR #117).
  unaudited() { # name pattern-in-output arguments...
    local name=$1 want=$2
    shift 2
    cases=$((cases + 1))
    set +e
    output=$(bash "$self" "$@" 2>&1)
    status=$?
    set -e
    if [ "$status" -eq 2 ] && printf '%s\n' "$output" | grep -qE "$want"; then
      echo "PASS $name: refused with status 2 ($(printf '%s\n' "$output" | grep -E '^(ERROR|MISSING) ' | head -1 | sed "s|$dir|<dir>|"))"
    else
      echo "FAIL $name: expected status 2 and /$want/, got status $status: $output"
      failures=$((failures + 1))
    fi
  }
  unaudited missing-path '^MISSING ' "$dir/does-not-exist"
  mkdir -p "$dir/trees/clean/sub" "$dir/trees/locked-file" "$dir/trees/locked-dir/inner" \
           "$dir/trees/fifo" "$dir/trees/dangling" "$dir/trees/linked"
  cp "$dir/clean" "$dir/trees/clean/a" && cp "$dir/clean" "$dir/trees/clean/sub/b"
  cp "$dir/clean" "$dir/trees/locked-file/ok"
  printf 'unread\n' > "$dir/trees/locked-file/mode-000" && chmod 000 "$dir/trees/locked-file/mode-000"
  printf 'unread\n' > "$dir/trees/locked-dir/inner/f" && chmod 000 "$dir/trees/locked-dir/inner"
  mkfifo "$dir/trees/fifo/pipe" "$dir/named-fifo"
  ln -s "$dir/nowhere" "$dir/trees/dangling/link" && ln -s "$dir/nowhere" "$dir/named-dangling"
  cp "$dir/clean" "$dir/trees/linked/real" && ln -s real "$dir/trees/linked/alias"
  if [ "$(id -u)" -ne 0 ]; then
    unaudited mode-000-file 'ERROR .*mode-000: cannot be read' "$dir/trees/locked-file/mode-000"
    unaudited mode-000-file-in-a-tree 'ERROR .*mode-000: cannot be read' "$dir/trees/locked-file"
    unaudited unreadable-subdirectory 'ERROR .*locked-dir: traversal' "$dir/trees/locked-dir"
  else
    echo "NOTE mode-000-file, mode-000-file-in-a-tree, unreadable-subdirectory: not run as root, which reads them anyway"
  fi
  unaudited named-fifo 'ERROR .*named-fifo: not a regular file' "$dir/named-fifo"
  unaudited fifo-in-a-tree 'ERROR .*pipe: not a regular file' "$dir/trees/fifo"
  unaudited named-device 'ERROR /dev/null: not a regular file' /dev/null
  unaudited named-dangling-symlink 'ERROR .*named-dangling: dangling symbolic link' "$dir/named-dangling"
  unaudited dangling-symlink-in-a-tree 'ERROR .*link: dangling symbolic link' "$dir/trees/dangling"
  unaudited directory-where-a-file-was-expected 'ERROR .*clean: a directory where a file was expected' --file "$dir/trees/clean"
  unaudited unaudited-path-among-clean-ones 'ERROR .*named-fifo' "$dir/clean" "$dir/named-fifo" "$dir/trees/clean"
  if [ -e /proc/1/mem ] && [ "$(id -u)" -ne 0 ]; then
    unaudited proc-1-mem 'ERROR /proc/1/mem: cannot be read' --file /proc/1/mem
  else
    echo "NOTE proc-1-mem: no /proc/1/mem here, or running as root"
  fi
  check clean-tree clean "$dir/trees/clean"
  cases=$((cases + 1))
  set +e
  output=$(bash "$self" --file "$dir/clean" "$dir/trees/clean" 2>&1)
  status=$?
  set -e
  if [ "$status" -eq 0 ] && printf '%s\n' "$output" | grep -q '^privacy-audit: PASS$'; then
    echo "PASS clean-file-with---file-and-a-clean-tree: clean"
  else
    echo "FAIL clean-file-with---file-and-a-clean-tree: status $status: $output"
    failures=$((failures + 1))
  fi
  check symlink-in-a-tree-is-skipped clean "$dir/trees/linked"
  echo "privacy-audit self-test: $((cases - failures)) of $cases cases passed"
  [ "$failures" -eq 0 ]
}

case "${1:-}" in
  --self-test) self_test ;;
  -h|--help) sed -n '2,19p' "$self" | sed 's/^# \{0,1\}//' ;;
  *) audit "$@" ;;
esac
