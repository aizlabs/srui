#!/usr/bin/env bash
# Has Codex posted a clean-verdict comment for *exactly* this head?
#
# Usage: scripts/ci-accept-codex-verdict.sh <pr-number> <head-sha>
#   exit 0 - a verdict for this head exists; the caller may proceed
#   exit 1 - no such verdict; the caller must fail closed
# Needs `gh` and $GITHUB_REPOSITORY.
#
# Codex reports "no findings" as an issue comment and *not* as a review object - measured on PR #65,
# where the only evidence for two clean heads was a comment, while review objects existed solely for
# the heads that had findings. So this path cannot be dropped in favour of the exact `commit_id` on a
# review; it has to be made safe instead.
#
# Why a prefix comparison is not safe, which is what this script exists to avoid. The comment
# abbreviates the sha (`**Reviewed commit:** `3e7eb47a22``), and the gate used to accept a verdict
# when `$HEAD_SHA` merely *started with* that abbreviation. An author could then collect a clean
# verdict on a benign head and grind a replacement commit sharing the abbreviation: 10 hex is 40
# bits, which a GPU exhausts in minutes, and 7 hex - the old lower bound - is 28 bits, which a laptop
# exhausts. The stale comment would satisfy the gate for code nobody reviewed.
#
# Instead the abbreviation is resolved server-side and compared in full. Measured against this
# repository's API: a 10-hex prefix resolves to its unique 40-hex sha, and anything ambiguous or
# unknown is refused with HTTP 422 - so resolution fails closed on its own.
set -uo pipefail

# `chatgpt-codex-connector[bot]`, by immutable account id: a login prefix would let any account
# registered with that prefix forge a verdict on a public repository.
CODEX_BOT_ID=199175422

pr=${1:?usage: ci-accept-codex-verdict.sh <pr-number> <head-sha>}
head=$(printf '%s' "${2:?usage: ci-accept-codex-verdict.sh <pr-number> <head-sha>}" | tr 'A-F' 'a-f')
repo=${GITHUB_REPOSITORY:?GITHUB_REPOSITORY must be set}

# Abbreviations captured out of the `**Reviewed commit:**` field of a bot comment whose body *starts*
# with the clean-verdict sentence. Captured from that field rather than searched for anywhere in the
# body, so a sha quoted in prose is not a verdict; and the body test excludes the bot's own "Codex
# Review Summary" comment, which tabulates an abbreviated sha too.
abbrevs=$(gh api "repos/$repo/issues/$pr/comments" --paginate \
    --jq "[ .[]
            | select(.user.id == $CODEX_BOT_ID)
            | (.body // \"\")
            | select(startswith(\"Codex Review: Didn't find any major issues.\"))
            | capture(\"Reviewed commit:\\\\*\\\\*\\\\s*\`(?<sha>[0-9a-fA-F]{7,40})\`\").sha
          ] | .[]" 2>/dev/null | tr 'A-F' 'a-f' | sort -u)

[ -n "$abbrevs" ] || exit 1

# Every commit of this pull request, for the ambiguity guard below.
pr_shas=$(gh api "repos/$repo/pulls/$pr/commits" --paginate --jq '.[].sha' 2>/dev/null |
    tr 'A-F' 'a-f')

for abbrev in $abbrevs; do
    full=$(gh api "repos/$repo/commits/$abbrev" --jq '.sha' 2>/dev/null | tr 'A-F' 'a-f')
    [ -n "$full" ] || continue       # unknown or ambiguous: 422, and nothing to compare
    [ "$full" = "$head" ] || continue # a verdict for some other commit, including an ancestor

    # Belt and braces, in case the API ever resolves an ambiguous abbreviation to an arbitrary
    # commit rather than refusing it: if any other commit of this pull request shares the
    # abbreviation, the comment does not identify the head uniquely.
    collisions=$(printf '%s\n' "$pr_shas" | grep -c "^$abbrev" || true)
    if [ "${collisions:-0}" -gt 1 ]; then
        echo "Refusing a verdict whose abbreviation ${abbrev} matches ${collisions} commits of this PR." >&2
        continue
    fi

    echo "Codex reviewed ${head} and reported no findings (verdict names ${abbrev}, resolved in full)."
    exit 0
done

exit 1
