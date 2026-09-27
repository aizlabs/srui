#!/usr/bin/env bash
# The review gate must identify Codex by account identity, never by a login prefix.
#
# `select((.user.login // "") | startswith("chatgpt-codex-connector"))` accepts any account whose
# login merely *begins* with the bot's name. On a public repository anyone can register
# `chatgpt-codex-connector-anything`, copy the bot's clean-verdict sentence verbatim onto their own
# pull request, and the gate then waves the change through unreviewed. Measured before the fix: a
# forged comment satisfied the matcher (count 1).
#
# Identity as measured against this repository's API:
#   REST     {"login": "chatgpt-codex-connector[bot]", "id": 199175422, "type": "Bot"}
#   GraphQL  {"login": "chatgpt-codex-connector", "__typename": "Bot"}   (no `[bot]` suffix)
# The REST matcher keys on the numeric id, which also survives a rename; GraphQL exposes no id on
# `author`, so it compares the login exactly and requires the author to be a Bot.
set -uo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"
workflow=.github/workflows/ci.yml
status=0

fail() { echo "FAIL: $*" >&2; status=1; }

# 1. The vulnerable spelling must not reappear, in any of the three matchers.
if grep -q 'startswith("chatgpt-codex-connector' "$workflow"; then
    fail "$workflow still identifies Codex by login prefix:"
    grep -n 'startswith("chatgpt-codex-connector' "$workflow" >&2
fi

# 2. Both REST matchers (the review query and the clean-verdict query) key on the account id.
rest_hits=$(grep -c '\.user\.id == 199175422' "$workflow" || true)
[ "$rest_hits" -eq 2 ] || fail "expected 2 REST identity checks (.user.id == 199175422), found $rest_hits"

# 3. The GraphQL matcher compares the login exactly and requires a Bot author.
grep -q '== "chatgpt-codex-connector"' "$workflow" ||
    fail "the GraphQL thread matcher does not compare the author login exactly"
grep -q '__typename == "Bot"' "$workflow" ||
    fail "the GraphQL thread matcher does not require a Bot author"

# 4. Behavioural check: a byte-identical verdict from a look-alike login must not count, and the
#    genuine one still must. Both run through the same jq program the workflow uses.
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
verdict='Codex Review: Didn'"'"'t find any major issues.\n\n**Reviewed commit:** `40497362`\n'

cat > "$work/real.json" <<EOF
[{"user": {"login": "chatgpt-codex-connector[bot]", "id": 199175422, "type": "Bot"},
  "body": "$verdict"}]
EOF
cat > "$work/forged.json" <<EOF
[{"user": {"login": "chatgpt-codex-connector-attacker", "id": 66666666, "type": "User"},
  "body": "$verdict"}]
EOF

matcher='[ .[]
    | select(.user.id == 199175422)
    | (.body // "")
    | select(startswith("Codex Review: Didn'"'"'t find any major issues."))
    | capture("Reviewed commit:\\*\\*\\s*`(?<sha>[0-9a-fA-F]{7,40})`").sha
    | ascii_downcase as $sha
    | select(($ENV.HEAD_SHA | ascii_downcase) | startswith($sha))
  ] | length'

export HEAD_SHA=40497362aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
real=$(jq "$matcher" "$work/real.json")
forged=$(jq "$matcher" "$work/forged.json")
[ "$real" = "1" ] || fail "the genuine clean verdict is no longer accepted (count $real)"
[ "$forged" = "0" ] || fail "a forged clean verdict from a look-alike login is accepted (count $forged)"

if [ "$status" -eq 0 ]; then
    echo "Review gate identifies Codex by account identity: genuine verdict accepted, forged rejected."
fi
exit "$status"
