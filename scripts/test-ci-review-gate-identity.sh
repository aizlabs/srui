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
# One matcher per place the bot is identified: the review query in the workflow, and the
# clean-verdict acceptance in its own script.
rest_hits=$(grep -c '\.user\.id == 199175422' "$workflow" || true)
[ "$rest_hits" -eq 1 ] || fail "expected 1 REST identity check in $workflow, found $rest_hits"
grep -q 'CODEX_BOT_ID=199175422' scripts/ci-accept-codex-verdict.sh ||
    fail "scripts/ci-accept-codex-verdict.sh does not identify Codex by account id"
grep -q 'startswith(\"chatgpt-codex-connector' scripts/ci-accept-codex-verdict.sh &&
    fail "scripts/ci-accept-codex-verdict.sh identifies Codex by login prefix"

# 3. The GraphQL matcher compares the login exactly and requires a Bot author.
grep -q '== "chatgpt-codex-connector"' "$workflow" ||
    fail "the GraphQL thread matcher does not compare the author login exactly"
grep -q '__typename == "Bot"' "$workflow" ||
    fail "the GraphQL thread matcher does not require a Bot author"

# 3b. The gate script must be checked out before it is invoked, from the base revision.
#     Extracting the logic into a script broke the gate once: the review-gate job had no
#     `actions/checkout`, so on a fresh runner the script did not exist, every clean verdict polled
#     for fifteen minutes and then failed closed - unmergeable without the explicit bypass. And the
#     checkout must pin the *base* revision: for a fork pull request the workflow comes from the base
#     branch while the head is the contributor's code, so checking out the head would let a pull
#     request rewrite the script that judges it.
checkout_line=$(grep -n 'pull_request.base.sha' "$workflow" | head -1 | cut -d: -f1)
verdict_line=$(grep -n 'ci-accept-codex-verdict.sh' "$workflow" | head -1 | cut -d: -f1)
if [ -z "$checkout_line" ]; then
    fail "the review gate invokes a script without checking out the repository at the base revision"
elif [ -z "$verdict_line" ]; then
    fail "$workflow no longer invokes scripts/ci-accept-codex-verdict.sh"
elif [ "$checkout_line" -ge "$verdict_line" ]; then
    fail "the gate's checkout (line $checkout_line) comes after it runs the script (line $verdict_line)"
fi

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

# 5. The clean-verdict acceptance, driven through a `gh` stand-in. Each case answers the three
#    endpoints the script calls - the PR's comments, commit resolution, and the PR's commit list -
#    from fixtures, so the decision is exercised with no network and no live pull request.
#
#    The attack under test: collect a verdict on a benign head, then grind a replacement commit that
#    shares the comment's abbreviated sha. 10 hex is 40 bits and 7 hex is 28, both cheap, so the gate
#    must never accept a stale verdict for a head nobody reviewed.
verdict_script=scripts/ci-accept-codex-verdict.sh
[ -x "$verdict_script" ] || fail "$verdict_script is not executable"

reviewed=aaaaaaaaaa111111111111111111111111111111   # the benign head Codex reviewed
ground=aaaaaaaaaa222222222222222222222222222222     # same 10-hex prefix, never reviewed
abbrev=aaaaaaaaaa

stub_dir=$(mktemp -d "${TMPDIR:-/tmp}/gate-gh-stub.XXXXXX")
cat >"$stub_dir/gh" <<'STUB'
#!/bin/sh
# Answers only what ci-accept-codex-verdict.sh asks of `gh api`:
#   GATE_COMMENT_SHA  the abbreviation the bot's verdict names ('' => no verdict comment)
#   GATE_RESOLVES_TO  what the API resolves that abbreviation to ('' => HTTP 422)
#   GATE_PR_SHAS      commits of the pull request, newline separated
api_path=
for arg in "$@"; do
    case $arg in repos/*) api_path=$arg ;; esac
done
case $api_path in
    */issues/*/comments)
        [ -n "${GATE_COMMENT_SHA:-}" ] || exit 0
        printf '%s\n' "$GATE_COMMENT_SHA"
        ;;
    */commits/*)
        if [ -z "${GATE_RESOLVES_TO:-}" ]; then
            echo 'gh: No commit found for SHA (HTTP 422)' >&2
            exit 1
        fi
        printf '%s\n' "$GATE_RESOLVES_TO"
        ;;
    */pulls/*/commits)
        printf '%b\n' "${GATE_PR_SHAS:-}"
        ;;
esac
STUB
chmod +x "$stub_dir/gh"

accepts_verdict() {
    PATH="$stub_dir:$PATH" GITHUB_REPOSITORY=aizlabs/srui \
        GATE_COMMENT_SHA="$1" GATE_RESOLVES_TO="$2" GATE_PR_SHAS="$3" \
        bash "$verdict_script" 72 "$4" >/dev/null 2>&1
}

if accepts_verdict "$abbrev" "$reviewed" "$reviewed" "$reviewed"; then
    echo "  ok: a genuine verdict for this head is accepted"
else
    fail "a genuine verdict for the current head was rejected"
fi

# The grind: head is the ground commit, while the verdict's abbreviation resolves to the reviewed one.
if accepts_verdict "$abbrev" "$reviewed" "$reviewed\n$ground" "$ground"; then
    fail "a stale verdict satisfied the gate for an unreviewed head sharing its abbreviation"
else
    echo "  ok: a verdict naming a different commit is rejected for a ground look-alike head"
fi

# The same grind after a force-push that drops the reviewed commit, so the pull request contains only
# the ground one. Nothing is ambiguous any more, and the collision guard cannot help: comparing the
# resolved sha in full is the only thing left that can reject this.
if accepts_verdict "$abbrev" "$reviewed" "$ground" "$ground"; then
    fail "a stale verdict satisfied the gate after a force-push that removed the reviewed commit"
else
    echo "  ok: rejected even when the reviewed commit is no longer in the pull request"
fi

# An abbreviation the API will not resolve (ambiguous or unknown) leaves nothing to compare.
if accepts_verdict "$abbrev" "" "$reviewed" "$reviewed"; then
    fail "a verdict whose abbreviation cannot be resolved was accepted"
else
    echo "  ok: an unresolvable abbreviation is refused"
fi

if accepts_verdict "" "$reviewed" "$reviewed" "$reviewed"; then
    fail "the gate accepted a head with no verdict comment"
else
    echo "  ok: no verdict comment means no acceptance"
fi

rm -rf "$stub_dir"

if [ "$status" -eq 0 ]; then
    echo "Review gate identifies Codex by account identity: genuine verdict accepted, forged rejected."
fi
exit "$status"
