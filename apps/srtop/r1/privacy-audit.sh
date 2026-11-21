#!/usr/bin/env bash
# PX-026-G01: the repeatable privacy and disclosure audit of R1's artifacts and settings
# (D1 §§25-27). Scans files, or every regular file under a directory, for key material, tokens,
# passwords and private paths, and names each finding by class and location only: the matched text
# is never printed, so the audit's own output cannot leak what it found.
#
#   apps/srtop/r1/privacy-audit.sh PATH...    exit 0 clean, 1 findings, 2 a path is missing
#   apps/srtop/r1/privacy-audit.sh --self-test
#                                             plants one secret of every class (built at run
#                                             time, so this file holds none) and fails unless
#                                             each is found and a clean file passes
#
# Classes:
#   private-key   PEM/OpenSSH private key blocks, PuTTY key files
#   token         GitHub, GitLab, AWS, Slack and OpenAI-style tokens, JWTs, bearer credentials
#   password      password/passwd/passphrase/secret/pwd assignments, credentials in a URL
#   private-path  the auditing user's home directory, or any /Users/<name>/ path but /Users/Shared
# A binary file (one with a NUL byte) is scanned for the first three classes only: a from-source
# build embeds the builder's own checkout path in its binaries, which is reported once per binary
# as BINARY-PATH information, not as a finding. Bash 3.2 compatible (the macOS /bin/bash).
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

finding() { # class file line
  findings=$((findings + 1))
  echo "FINDING $1 $2:$3"
}

scan_rule() { # file class flags regex
  local line
  for line in $(LC_ALL=C grep -a -n $3 -e "$4" "$1" 2>/dev/null | cut -d: -f1 | sort -un); do
    finding "$2" "$1" "$line"
  done
}

scan_paths() { # file
  local line
  if [ -n "${HOME:-}" ] && [ "${#HOME}" -gt 1 ]; then
    for line in $(LC_ALL=C grep -a -n -F -e "$HOME" "$1" 2>/dev/null | cut -d: -f1 | sort -un); do
      finding private-path "$1" "$line"
    done
  fi
  for line in $(LC_ALL=C grep -a -n -o -E '/Users/[^/[:space:]"]+/' "$1" 2>/dev/null \
                | grep -v ':/Users/Shared/$' | cut -d: -f1 | sort -un); do
    finding private-path "$1" "$line"
  done
}

scan_file() { # file
  local class flags regex binary=""
  scanned=$((scanned + 1))
  if [ -s "$1" ] && ! LC_ALL=C grep -I -q '' "$1" 2>/dev/null; then
    binary=1
    binaries=$((binaries + 1))
  fi
  local rule IFS='
'
  set -f
  for rule in $(rules); do
    class=${rule%%	*}
    rule=${rule#*	}
    flags=${rule%%	*}
    regex=${rule#*	}
    scan_rule "$1" "$class" "$flags" "$regex"
  done
  set +f
  unset IFS
  if [ -z "$binary" ]; then
    scan_paths "$1"
  elif [ -n "${HOME:-}" ] && [ "${#HOME}" -gt 1 ] && LC_ALL=C grep -a -q -F -e "$HOME" "$1"; then
    echo "BINARY-PATH $1: embeds the builder's home directory (build paths; not a finding)"
  fi
}

audit() {
  local path file missing=0
  [ "$#" -gt 0 ] || { echo "usage: privacy-audit.sh PATH... | --self-test" >&2; return 2; }
  for path in "$@"; do
    if [ -f "$path" ]; then
      scan_file "$path"
    elif [ -d "$path" ]; then
      local IFS='
'
      set -f
      for file in $(find "$path" -type f | LC_ALL=C sort); do
        scan_file "$file"
      done
      set +f
      unset IFS
    else
      echo "MISSING $path"
      missing=1
    fi
  done
  echo "privacy-audit: $scanned file(s) scanned ($binaries binary), $findings finding(s)"
  if [ "$missing" -ne 0 ]; then
    echo "privacy-audit: FAIL (a path to audit does not exist)"
    return 2
  fi
  if [ "$findings" -ne 0 ]; then
    echo "privacy-audit: FAIL"
    return 1
  fi
  echo "privacy-audit: PASS"
}

random_chars() { # count alphabet
  LC_ALL=C tr -dc "$2" </dev/urandom | head -c "$1"
}

self_test() {
  local dir failures=0 cases=0 name expected output status
  dir=$(mktemp -d "${TMPDIR:-/tmp}/srui-privacy-selftest.XXXXXX")
  # Removes only this test's own directory, on every exit path.
  # shellcheck disable=SC2064
  trap "rm -rf '$dir'" EXIT
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
  plant private-path-home "${HOME:-/Users/nobody}/Library/plant.txt"
  plant private-path-users "/Users/$(random_chars 8 'a-z')/Documents/plant.txt"
  mkdir -p "$dir/nested/deeper"
  printf 'x %s y\n' "ghp_$(random_chars 36 "$alnum")" > "$dir/nested/deeper/token-nested"
  printf 'Read-only · Live process snapshot\nconnected to 127.0.0.1:2222 as srui\n/Users/Shared/ok\nsession 0123456789abcdef0123456789abcdef\nTest run with 6 tests passed\n' > "$dir/clean"
  printf 'bin\000ary %s\000tail\n' "ghp_$(random_chars 36 "$alnum")" > "$dir/binary-token"
  printf 'bin\000ary %s\000tail\n' "${HOME:-/Users/nobody}/src/main.swift" > "$dir/binary-path"

  check() { # name expected-class-or-clean target
    cases=$((cases + 1))
    set +e
    output=$(bash "$self" "$3" 2>&1)
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
      if printf '%s\n' "$output" | grep -v -E '^(FINDING [a-z-]+ .+:[0-9]+|BINARY-PATH .+|MISSING .+|privacy-audit: .+)$' | grep -q .; then
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
              password-flag password-json password-url private-path-home private-path-users; do
    expected=${name%%-*}
    case "$name" in
      private-key-*) expected=private-key ;;
      private-path-*) expected=private-path ;;
    esac
    check "$name" "$expected" "$dir/$name"
  done
  check nested-directory token "$dir/nested"
  check clean-file clean "$dir/clean"
  check binary-token token "$dir/binary-token"
  check binary-build-path clean "$dir/binary-path"
  cases=$((cases + 1))
  set +e
  output=$(bash "$self" "$dir/does-not-exist" 2>&1)
  status=$?
  set -e
  if [ "$status" -eq 2 ]; then
    echo "PASS missing-path: refused with status 2"
  else
    echo "FAIL missing-path: status $status: $output"
    failures=$((failures + 1))
  fi
  echo "privacy-audit self-test: $((cases - failures)) of $cases cases passed"
  [ "$failures" -eq 0 ]
}

case "${1:-}" in
  --self-test) self_test ;;
  -h|--help) sed -n '2,19p' "$self" | sed 's/^# \{0,1\}//' ;;
  *) audit "$@" ;;
esac
