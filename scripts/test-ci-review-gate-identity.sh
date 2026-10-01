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
#
# `\\?"` is load-bearing. A `--jq` program written inside a double-quoted shell string spells its own
# quotes `\"`, which is exactly how `origin/main`'s REST matcher spelled this one:
#   select((.user.login // \"\") | startswith(\"chatgpt-codex-connector\"))
# A pattern looking for `startswith("` could not see it - verified against the base revision, where
# `grep -c 'startswith("chatgpt-codex-connector'` finds 1 of the 2 occurrences, the GraphQL one - so
# reintroducing the forgeable matcher in the REST query, or OR-ing it into the id test, passed this
# check.
codex_login_prefix='startswith\(\\?"chatgpt-codex-connector'
if grep -qE "$codex_login_prefix" "$workflow"; then
    fail "$workflow still identifies Codex by login prefix:"
    grep -nE "$codex_login_prefix" "$workflow" >&2
fi

# 2. Both REST matchers (the review query and the clean-verdict query) key on the account id.
# One matcher per place the bot is identified: the review query in the workflow, and the
# clean-verdict acceptance in its own script.
rest_hits=$(grep -c '\.user\.id == 199175422' "$workflow" || true)
[ "$rest_hits" -eq 1 ] || fail "expected 1 REST identity check in $workflow, found $rest_hits"
grep -q 'CODEX_BOT_ID=199175422' scripts/ci-accept-codex-verdict.sh ||
    fail "scripts/ci-accept-codex-verdict.sh does not identify Codex by account id"
grep -qE "$codex_login_prefix" scripts/ci-accept-codex-verdict.sh &&
    fail "scripts/ci-accept-codex-verdict.sh identifies Codex by login prefix"

# 3. The GraphQL matcher compares the login exactly and requires a Bot author.
grep -q '== "chatgpt-codex-connector"' "$workflow" ||
    fail "the GraphQL thread matcher does not compare the author login exactly"
grep -q '__typename == "Bot"' "$workflow" ||
    fail "the GraphQL thread matcher does not require a Bot author"

# 3b. The gate script must be checked out before it is invoked, inside the review-gate job.
#     Extracting the logic into a script broke the gate twice. First the job had no
#     `actions/checkout` at all, so on a fresh runner the script did not exist. Then the checkout was
#     pinned to the base revision, which is worse in a subtler way: a pull request that *adds* a gate
#     script cannot find it in the base, and because the invocation is an `if` condition, `set -e`
#     does not stop the loop - the gate polls for fifteen minutes and fails closed. Both shapes are
#     caught here by requiring a checkout ahead of the invocation within this job, and no base pin.
gate_job_block=$(awk '
    /^  review-gate:/ { inside = 1; print NR ": " $0; next }
    inside && /^  [a-zA-Z]/ { exit }
    inside { print NR ": " $0 }
' "$workflow")
gate_checkout_line=$(printf '%s\n' "$gate_job_block" | grep -F 'actions/checkout' | head -1 | cut -d: -f1)
gate_verdict_line=$(printf '%s\n' "$gate_job_block" | grep -F 'ci-accept-codex-verdict.sh' | head -1 | cut -d: -f1)
if [ -z "$gate_verdict_line" ]; then
    fail "the review-gate job no longer invokes scripts/ci-accept-codex-verdict.sh"
elif [ -z "$gate_checkout_line" ]; then
    fail "the review-gate job invokes a script without checking out the repository"
elif [ "$gate_checkout_line" -ge "$gate_verdict_line" ]; then
    fail "the gate checks out (line $gate_checkout_line) after running the script (line $gate_verdict_line)"
fi
if printf '%s\n' "$gate_job_block" | grep -qF 'pull_request.base.sha'; then
    fail "the gate's checkout is pinned to the base revision, where a newly added gate script cannot exist"
fi

# 4. The clean-verdict acceptance, driven end to end through a `gh` stand-in. Each case answers the
#    three endpoints the script calls - the PR's comments, commit resolution, and the PR's commit
#    list - from fixtures, so the decision is exercised with no network and no live pull request.
#
#    The comments endpoint answers with a realistic JSON array and the stand-in runs the script's own
#    `--jq` program over it. That is the point of the shape. The previous stand-in printed the
#    abbreviation and ignored `--jq`, so the program that decides *which* comments count never ran:
#    deleting `select(.user.id == $CODEX_BOT_ID)`, the `startswith("Codex Review: Didn't find any
#    major issues.")` body test and the `Reviewed commit:` capture - leaving a program that accepts
#    any hex string in any comment from any account - all three left this suite green (measured).
#    There is deliberately no second copy of that program here either: the hand-copied one this
#    replaces had drifted to a spelling in neither the workflow nor the script, and still asserted the
#    `startswith($sha)` prefix semantics the script no longer has.
#
#    The attack under test: collect a verdict on a benign head, then grind a replacement commit that
#    shares the comment's abbreviated sha. 10 hex is 40 bits and 7 hex is 28, both cheap, so the gate
#    must never accept a stale verdict for a head nobody reviewed.
verdict_script=scripts/ci-accept-codex-verdict.sh
[ -x "$verdict_script" ] || fail "$verdict_script is not executable"

reviewed=aaaaaaaaaa111111111111111111111111111111 # the benign head Codex reviewed
ground=aaaaaaaaaa222222222222222222222222222222   # same 10-hex prefix, never reviewed
other=bbbbbbbbbb333333333333333333333333333333   # an unrelated commit
abbrev=aaaaaaaaaa
other_abbrev=bbbbbbbbbb

work=$(mktemp -d "${TMPDIR:-/tmp}/gate-gh-stub.XXXXXX")
trap 'rm -rf "$work"' EXIT

cat >"$work/gh" <<'STUB'
#!/bin/sh
# Answers only what ci-accept-codex-verdict.sh asks of `gh api`, and runs the `--jq` program it was
# handed over that answer - so the script's own matcher is what decides every case below.
#   GATE_COMMENTS_JSON  file holding the issue-comments response (a JSON array); absent => []
#   GATE_RESOLUTIONS    `ref<TAB>sha` lines; a ref not listed is a 422. Takes precedence.
#   GATE_RESOLVES_TO    what any ref resolves to when GATE_RESOLUTIONS is empty ('' => 422)
#   GATE_PR_SHAS        commits of the pull request, newline separated
#
# `--paginate` is accepted and ignored: real `gh` applies the filter per page, and every fixture here
# is one page.
api_path=
jq_program=
want_jq=0
for arg in "$@"; do
    if [ "$want_jq" -eq 1 ]; then
        jq_program=$arg
        want_jq=0
        continue
    fi
    case $arg in
        --jq) want_jq=1 ;;
        --jq=*) jq_program=${arg#--jq=} ;;
        repos/*) api_path=$arg ;;
    esac
done
[ -n "$jq_program" ] || jq_program=.

# `gh --jq` prints strings raw, like `jq -r`, which is what the script's `tr`/`sort` pipeline reads.
emit() { printf '%s' "$1" | jq -r "$jq_program"; }

case $api_path in
    */pulls/*/commits)
        printf '%b\n' "${GATE_PR_SHAS:-}" |
            jq -R 'select(length > 0) | {sha: .}' | jq -s . | jq -r "$jq_program"
        ;;
    */issues/*/comments)
        if [ -r "${GATE_COMMENTS_JSON:-}" ]; then
            jq -r "$jq_program" "$GATE_COMMENTS_JSON"
        else
            emit '[]'
        fi
        ;;
    */commits/*)
        ref=${api_path##*/}
        if [ -n "${GATE_RESOLUTIONS:-}" ]; then
            resolved=$(printf '%b\n' "$GATE_RESOLUTIONS" |
                awk -F'\t' -v r="$ref" '$1 == r { print $2; exit }')
        else
            resolved=${GATE_RESOLVES_TO:-}
        fi
        if [ -z "$resolved" ]; then
            echo 'gh: No commit found for SHA (HTTP 422)' >&2
            exit 1
        fi
        emit "{\"sha\": \"$resolved\"}"
        ;;
esac
STUB
chmod +x "$work/gh"

# One comment per `id<TAB>login<TAB>body<TAB>type` line on stdin, shaped like the issue-comments
# response. `\n` inside a body becomes a real newline, so a case can craft a multi-line body on one
# line, and jq does the JSON escaping - a hand-escaped fixture is how a crafted body stops being the
# one the test meant to craft.
comments_json() {
    jq -Rs '
        split("\n") | map(select(length > 0)) | map(split("\t"))
        | map({ user: { id: (.[0] | tonumber), login: .[1], type: .[3] },
                body: (.[2] | gsub("\\\\n"; "\n")) })'
}

codex_comment() { # <body>
    printf '%s\t%s\t%s\t%s\n' 199175422 'chatgpt-codex-connector[bot]' "$1" Bot
}

# `accepts_verdict <comments fixture> <resolutions> <pr shas> <head>`; an empty fixture path means
# the pull request has no comments at all.
accepts_verdict() {
    PATH="$work:$PATH" GITHUB_REPOSITORY=aizlabs/srui \
        GATE_COMMENTS_JSON="$1" GATE_RESOLUTIONS="$2" GATE_PR_SHAS="$3" \
        bash "$verdict_script" 72 "$4" >/dev/null 2>&1
}

# The fixtures. Every body here is one the bot could really post, or one an attacker really can.
verdict_body="Codex Review: Didn't find any major issues.\n\n**Reviewed commit:** \`$abbrev\`\n"

codex_comment "$verdict_body" | comments_json >"$work/genuine.json"
printf '%s\t%s\t%s\t%s\n' 66666666 'chatgpt-codex-connector-attacker' "$verdict_body" User |
    comments_json >"$work/foreign-author.json"
codex_comment "\`\`\`\n$verdict_body\`\`\`\n" | comments_json >"$work/fenced.json"
codex_comment "## Codex Review Summary\n\nThe reviewer said:\n\n> Codex Review: Didn't find any major issues.\n\n**Reviewed commit:** \`$abbrev\`\n" |
    comments_json >"$work/quoted.json"
codex_comment "Codex Review: Didn't find any major issues.\n\n**Reviewed commit:** \`$other_abbrev\`\n\n**Reviewed commit:** \`$abbrev\`\n" |
    comments_json >"$work/two-fields.json"
codex_comment "Codex Review: Didn't find any major issues.\n\n**Reviewed commit:** \`deadbeefcafe\`\n" |
    comments_json >"$work/ref-name.json"
printf '[]\n' >"$work/none.json"

resolves_both="$abbrev\t$reviewed\n$other_abbrev\t$other"

# The genuine fixture has to be the body the bot really posts before any verdict about the crafted
# ones means anything: one that failed `startswith` for an uninteresting reason - a mangled escape,
# say - would make every rejection below pass vacuously.
jq -e --arg sha "$abbrev" '
    .[0].body
    | startswith("Codex Review: Didn'"'"'t find any major issues.")
      and test("\\n\\n\\*\\*Reviewed commit:\\*\\* `" + $sha + "`")' \
    "$work/genuine.json" >/dev/null ||
    {
        fail "the genuine verdict fixture is not the body the bot posts"
        jq -r '.[0].body' "$work/genuine.json" >&2
    }

if accepts_verdict "$work/genuine.json" "$resolves_both" "$reviewed" "$reviewed"; then
    echo "  ok: a genuine verdict for this head is accepted"
else
    fail "a genuine verdict for the current head was rejected"
fi

# The grind: head is the ground commit, while the verdict's abbreviation resolves to the reviewed one.
if accepts_verdict "$work/genuine.json" "$resolves_both" "$reviewed\n$ground" "$ground"; then
    fail "a stale verdict satisfied the gate for an unreviewed head sharing its abbreviation"
else
    echo "  ok: a verdict naming a different commit is rejected for a ground look-alike head"
fi

# The same grind after a force-push that drops the reviewed commit, so the pull request contains only
# the ground one. Nothing is ambiguous any more, and the collision guard cannot help: comparing the
# resolved sha in full is the only thing left that can reject this.
if accepts_verdict "$work/genuine.json" "$resolves_both" "$ground" "$ground"; then
    fail "a stale verdict satisfied the gate after a force-push that removed the reviewed commit"
else
    echo "  ok: rejected even when the reviewed commit is no longer in the pull request"
fi

# An abbreviation the API will not resolve (ambiguous or unknown) leaves nothing to compare.
if accepts_verdict "$work/genuine.json" "$other_abbrev\t$other" "$reviewed" "$reviewed"; then
    fail "a verdict whose abbreviation cannot be resolved was accepted"
else
    echo "  ok: an unresolvable abbreviation is refused"
fi

if accepts_verdict "$work/none.json" "$resolves_both" "$reviewed" "$reviewed"; then
    fail "the gate accepted a head with no verdict comment"
else
    echo "  ok: no verdict comment means no acceptance"
fi

# An account whose login merely begins with the bot's name, posting the bot's sentence byte for byte.
# Only the account id separates it from the real thing.
if accepts_verdict "$work/foreign-author.json" "$resolves_both" "$reviewed" "$reviewed"; then
    fail "a verdict posted by a look-alike account was accepted"
else
    echo "  ok: a perfect verdict from a non-Codex account is rejected"
fi

# Crafted bodies. The verdict sentence has to *open* the comment: anyone can quote it, and the bot's
# own "Codex Review Summary" comment tabulates an abbreviated sha of its own.
if accepts_verdict "$work/fenced.json" "$resolves_both" "$reviewed" "$reviewed"; then
    fail "a verdict inside a code fence was accepted"
else
    echo "  ok: a verdict quoted inside a code fence is not a verdict"
fi

if accepts_verdict "$work/quoted.json" "$resolves_both" "$reviewed" "$reviewed"; then
    fail "the verdict sentence quoted inside a longer comment was accepted"
else
    echo "  ok: the verdict sentence quoted inside a longer comment is not a verdict"
fi

# Two `Reviewed commit:` fields: the capture takes the first, so a body whose first field names some
# other commit does not become a verdict for this head because the head appears further down.
if accepts_verdict "$work/two-fields.json" "$resolves_both" "$reviewed" "$reviewed"; then
    fail "a comment naming two commits was accepted for the second one"
else
    echo "  ok: only the first Reviewed commit field counts"
fi

# `GET /commits/{ref}` resolves branches and tags too, and `[0-9a-fA-F]{7,40}` is a legal ref name:
# here a ref spelled like an abbreviation points at the head while abbreviating nothing.
if accepts_verdict "$work/ref-name.json" "deadbeefcafe\t$reviewed" "$reviewed" "$reviewed"; then
    fail "a verdict naming a ref that merely points at the head was accepted"
else
    echo "  ok: the resolved sha must begin with the abbreviation the comment named"
fi

if [ "$status" -eq 0 ]; then
    echo "Review gate identifies Codex by account identity: genuine verdict accepted, forged rejected."
fi
exit "$status"
