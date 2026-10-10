---
name: address-pr-review-comments
description: Work a pull request's open review comments to closure — read every unresolved thread, judge each finding on the code rather than accepting it, fix what is real, push, reply with the evidence, and resolve the thread. Use when asked to "fix the comments on GitHub", address review feedback, or get a PR's review gate green.
---

# Address PR review comments (SRUI)

Work the pull request named in the invocation (`$ARGUMENTS`); with no argument, use the
PR for the current branch (`gh pr view --json number`). State the PR number and head
commit before touching anything, so every reply is anchored to a known tree.

This skill is the mirror of `adversarial-pr-review`: that one produces findings, this
one closes them. A thread is closed by a fix plus evidence, never by a reply alone.

## Before anything else: work in the right place

Check `git status --short --branch`, `git branch --show-current`, and `git worktree list`.
Never edit on `main`. Fix on the PR's own branch, in the worktree that already holds it
if one exists; create one from the PR branch otherwise. `git fetch origin` first — the
branch may have moved since you last looked, and a fix committed onto a stale base is a
force-push waiting to happen.

## 1. Collect every open thread

Review comments are not PR comments. Fetch both, and fetch thread resolution state,
which the REST API does not expose:

```bash
gh api repos/{owner}/{repo}/pulls/{pr}/comments \
  --jq '.[] | {id, path, line, user: .user.login, in_reply_to_id, body}'

gh api graphql -f query='query{repository(owner:"OWNER",name:"REPO"){
  pullRequest(number:PR){reviewThreads(first:50){nodes{
    id isResolved isOutdated
    comments(first:10){nodes{databaseId author{login} body}}}}}}}' \
  --jq '.data.repository.pullRequest.reviewThreads.nodes[]
        | select(.isResolved==false) | {id, first: .comments.nodes[0].databaseId}'
```

Work only unresolved threads. Note each thread's node ID (`PRRT_…`) now — you need it to
resolve, and it is not the comment's numeric ID.

## 2. Judge each finding before fixing it

A review comment is a claim about the code, not an instruction. For each one:

- Read the cited code at the current head yourself. The comment may quote a line that a
  later commit already changed.
- Decide: **real**, **already fixed**, **wrong**, or **out of scope for this PR**.
- For a real finding, state the concrete failure — inputs or state leading to a wrong
  observable outcome. If you cannot state one, you have not understood the finding yet;
  re-read the code rather than applying a speculative patch.
- An automated reviewer is wrong often enough to be worth checking, and right often
  enough that "the bot is wrong" needs the same burden of proof as any other claim.
  Disagreeing is a legitimate outcome — say why, in the thread, and leave the thread
  for a human to resolve rather than resolving it yourself.

A finding that is real but outside the PR's scope becomes a follow-up issue or ticket,
recorded in the reply with its ID. Do not quietly widen the PR.

## 3. Fix, with a test that proves it

Fix the cause, not the symptom the comment happened to name. Prefer a change that makes
the defect unrepresentable — a required trait method, a narrowed type, an enum that
forces the caller to handle the case — over one that patches the single reported call
site and leaves the next one exposed.

Every fix needs a test that **fails before it and passes after**. Prove that:

1. Write or extend the test.
2. Revert the fix (or reintroduce the defect), run the test, confirm it fails, and keep
   the exit status.
3. Restore the fix and confirm green.

A fix whose test passes in both states proves nothing and is not a closed thread.

**`touch` every file you edited before running any test.** The editing tools preserve
mtimes, and both cargo and SwiftPM key rebuilds off mtime: an unchanged test count after
an edit means you are reading a stale binary, not a passing suite.

Run the narrow profile for what you changed (see `CLAUDE.md`), not the whole repository,
unless the change reaches further than the file you touched.

## 4. Update the PR

Commit with a message naming what the fix does, not "address review comments". Push to
the PR branch — never force-push a branch someone else may have pulled unless you say so
in the thread. Update the PR body when the fix changed behavior described there.

If the repository records ticket evidence (`docs/**/completions/*.md`), add a short
**Review follow-up** section: the finding, the fix, the tests, the mutation check. The
completion note must not still describe the pre-review behavior.

## 5. Reply, then resolve

Close the loop in the same turn as the push — an open thread blocks the review gate, and
a fix nobody is told about reads as an ignored finding.

Reply in the thread (not as a new top-level comment) with: the commit SHA, what changed
and why that addresses the finding, the test that now covers it, and the mutation result.

```bash
gh api repos/{owner}/{repo}/pulls/{pr}/comments/{comment_id}/replies -f body="..."

gh api graphql -f query='mutation($id:ID!){resolveReviewThread(input:{threadId:$id}){
  thread{isResolved}}}' -f id="PRRT_…"
```

Resolve only threads you actually closed with a fix. Leave open: anything you disagreed
with, anything deferred to a follow-up, anything a human must judge.

## 6. Re-request review and confirm the gate

This repository's `Codex Review Resolved` check keys on the **head SHA** and fails closed
after 15 minutes:

```
No Codex review of <sha> after 15 minutes.
```

Codex reviews automatically only when a PR opens or leaves draft — **not** on a later
push. So after pushing a fix: comment `@codex review`, then re-run the failed check
(`gh run rerun <run-id> --failed`). Never reach for `skip_review_gate`; a gate failing
because nobody reviewed the new code is evidence, not an obstacle.

Then confirm, rather than assuming: checks green, every intended thread resolved, and
`mergeStateStatus` no longer `UNSTABLE`. `gh pr view <pr> --json mergeStateStatus,statusCheckRollup`.

## Report

Per thread: the finding, your verdict (fixed / already fixed / disputed / deferred), the
commit, the test and its mutation result, and whether the thread is resolved. Then the
gate state, and anything left open with the reason. Do not merge; do not report a thread
closed that you left open.
