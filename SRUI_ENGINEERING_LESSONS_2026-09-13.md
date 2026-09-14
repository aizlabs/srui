**SRUI: ten engineering lessons from the implementation history**

Repository: [aizlabs/srui](https://github.com/aizlabs/srui)

Audit date: 13 September 2026

Merged snapshot: [da18fba344b123886be6dd1063c762656f6d480f](https://github.com/aizlabs/srui/commit/da18fba344b123886be6dd1063c762656f6d480f)

SRUI demonstrates exceptionally fast construction of a substantial protocol, server, and native client, followed by considerable work to make their interactions and verification trustworthy. The strongest improvements changed the system so that an entire class of mistakes became harder to express. The weakest stretches repeatedly repaired local symptoms while ownership, measurement, or acceptance criteria remained unsettled.

My central recommendation is to move more of the rigor already visible late in this history into the beginning of each risky task. Preserve the speed of implementation; improve the quality of the question, the independence of the test, and the clarity of the decision to accept the result.

**What this audit covers**

I inventoried all 427 commits reachable through local refs after fetching origin, including side branches and stash objects. Of these, 419 are reachable from the merged snapshot: 349 non-merge commits and 70 merge commits. Eight objects sit outside its ancestry, including three stash-related commits and implementation/verification work; they are not counted as delivered changes.

The analysis combines the full commit-subject chronology, programmatic inspection of commit metadata/messages and per-file change statistics, focused inspection of important patches, repository guidance, all 42 merged GitHub PRs' delivery metadata, and selected PR descriptions and review discussions. It is a historical engineering assessment, not a line-by-line review of every patch or a fresh correctness certification. Historical verification claims below are attributed to their commits/PRs; I did not rerun product tests.

Author names and co-author trailers do not measure human effort or identify who actually reasoned about a change. In particular, a response posted under the maintainer's account may have been written by an agent. GitHub review submissions can be individual replies; their counts are not counts of independent review rounds.

**The observed pace**

| Observation | Evidence | Interpretation |
|---|---|---|
| Merged history spans 28 August–12 September 2026 | Commit timestamps at the audited snapshot | Sixteen calendar dates, not sixteen measured workdays |
| First scaffold to the initial local end-to-end integration commit: about 24 hours 44 minutes | [376028b7](https://github.com/aizlabs/srui/commit/376028b7) → [822997a3](https://github.com/aizlabs/srui/commit/822997a3) | Rapid assembly of the first vertical path; not proof it was already stable |
| 107 of 349 non-merge commits occurred on the first two dates | Committer dates, using each commit's recorded offset | About 31% of the commit activity arrived during initial construction |
| 162 of 349 non-merge subjects contain correction/review-related words | Case-insensitive whole-word pattern: fix*, harden*, review, correct*, repair* | A 46.4% classification heuristic, not a defect rate or a fraction of labor wasted |
| 42 merged PRs; median opening-to-merge interval about 2.55 hours | GitHub PR creation and merge timestamps | A workflow measure that omits work completed before opening |
| Text editing: 24 non-merge branch commits; benchmarks: 49; connection manager: 22 | Commits on each PR's merged second-parent side absent from its first parent, excluding merges | Complex deliverables required substantial iteration despite short apparent PR intervals |
| SessionController.swift changed in 80 non-merge commits; session.rs in 55; EventOutbox.swift in 44 | Per-commit changed-path counts | Repeated integration pressure concentrated in lifecycle and ordering code |
| Raw textual churn: 345,373 additions and 125,018 deletions | Non-merge Git numstat, all tracked text | Unsuitable as authored code or productivity: it includes fuzz corpus, generated files, docs, baselines, moves, and rewrites |

The headline velocity is real as observed delivery cadence. Its conversion into a human-hours saving or an AI productivity multiplier is unknowable from this record. The authorship metadata contains 308 non-merge commits attributed to Alx and 41 to Cursor Agent; this is especially poor evidence for a human-versus-AI work split.

A telling example is [PR #52](https://github.com/aizlabs/srui/pull/52): it was open for roughly 38 minutes but carried 22 non-merge commits whose author timestamps span about ten hours. Opening-to-merge time would radically underdescribe even the visible commit history, let alone preparation.

**1. Define delivery by a verified user scenario, not the first implementation commit**

The initial end-to-end path arrived within about a day. The following history then addressed resync, receive-loop failure, event retry, transport lifecycle, acknowledgement races, and session-safe replay. Examples include [f12ac657](https://github.com/aizlabs/srui/commit/f12ac657), [ffae3ebf](https://github.com/aizlabs/srui/commit/ffae3ebf), and [1b6b2721](https://github.com/aizlabs/srui/commit/1b6b2721).

What went well: the project reached an executable vertical slice early, creating something concrete against which to discover integration problems. The counter, process monitor, coding-agent demo, and connection manager supplied progressively richer consumers.

What could improve: task completion needs to distinguish “the happy path exists” from “the promised behavior survives its specified interruptions.” Otherwise the backlog rewards implementation speed while stabilization becomes a stream of apparently unrelated fixes.

**Practice:** give each feature one executable acceptance scenario and a small failure matrix at ticket creation. For a remote counter, include connect, click, lose an ACK, reconnect, and prove the increment happened exactly once. Record first demonstrable behavior and acceptance completion separately. Do not demand every future subsystem in the first slice; demand honesty about its boundary.

**2. When a bug crosses subsystems, repair the contract before adding another guard**

[PR #26](https://github.com/aizlabs/srui/pull/26) introduced outbound backpressure. [PR #27](https://github.com/aizlabs/srui/pull/27) documents the resulting architectural problem: a coalesced N→M delivery could advance the store while the journal remained behind, wedging later commits with NonContiguousRevision. A pre-check contained one path; another remained.

The stronger repair introduced distinct AuthoritativeCommit, CoalescedScalarDelta, and ResyncSnapshot types and entry points. Journal admission and staging were validated before irreversible mutation; coalescing moved out of the authoritative model. See [7e54a8f5](https://github.com/aizlabs/srui/commit/7e54a8f5), [7cdfe587](https://github.com/aizlabs/srui/commit/7cdfe587), and [44b00f7e](https://github.com/aizlabs/srui/commit/44b00f7e).

This is one of the best engineering decisions in the history. It corrected the concept shared by multiple modules, rather than relying on every caller to remember an exception.

**Practice:** before changing batching, replay, caching, or coalescing, write down which forms may reach which owners, what revisions mean, and when publication becomes irreversible. Where practical, encode those distinctions in types. Repeated fixes on both sides of an interface should trigger a contract review.

**3. A passing test is useful only if the intended mistake would make it fail**

[PR #47](https://github.com/aizlabs/srui/pull/47) contains the clearest cautionary example. Some conformance suites compared generated fixtures derived from the same registry; agreement did not establish that production behavior honored the registry. [08dc813a](https://github.com/aizlabs/srui/commit/08dc813a) replaced those checks with real event validation, renderer interactions, and outbound encoding.

Then [3cdeff55](https://github.com/aizlabs/srui/commit/3cdeff55) found that expected-panic tests looped over cases. Execution stopped on the first expected panic: nine of eleven cases were unverified. Splitting cases made each defect independently observable.

The positive counterexample is [cf923ca2](https://github.com/aizlabs/srui/commit/cf923ca2): surviving collection/eviction mutants identified missing boundary assertions.

**Practice:** for high-risk behavior, require a demonstration that the regression test fails when the relevant behavior is broken. Use targeted mutation checks where the expected failure is hard to establish. Generate structural registry data once, but obtain behavioral evidence through the actual implementation. Do not create a second generated “oracle” and mistake consistency for conformance.

**4. Concurrency needs an ownership model before it needs more conditionals**

Text editing accumulated 24 non-merge commits in [PR #39](https://github.com/aizlabs/srui/pull/39). The sequence addressed draft retention, acknowledgement order, remounts, IME composition, replay, resync, and authorization. Later connection-manager work again needed continuity ownership, supersession cleanup, persistence ordering, and replacement-session recovery.

The path-change concentration reinforces the observation: SessionController.swift was touched in 80 non-merge commits. That is a hotspot signal, not proof that every change was a defect or that a large controller is inherently wrong.

What went well: fixes increasingly named ownership and generation boundaries, including [c53696e4](https://github.com/aizlabs/srui/commit/c53696e4), [2cdd5953](https://github.com/aizlabs/srui/commit/2cdd5953), and [d3926163](https://github.com/aizlabs/srui/commit/d3926163). What was costly: local race fixes kept interacting with neighboring lifecycle states.

**Practice:** specify who owns a draft, queued event, budget, transport, and session identity through disconnect, replacement, and cancellation. Put the state transitions in a compact table, including forbidden transitions and cleanup obligations. Test adversarial event order with controlled rendezvous rather than timing guesses. Keep ordering-critical decisions under one clearly defined owner.

**5. Resource limits must bound objects and identities, not only bytes**

Security hardening in [PR #44](https://github.com/aizlabs/srui/pull/44) illustrates how a plausible limit can leave another dimension unbounded. [f966ce24](https://github.com/aizlabs/srui/commit/f966ce24) reports a sub-1-MB payload materializing 5,999 queued transaction tasks. Byte backpressure did not bound the number of tasks, UUIDs, and dictionary entries.

The fix added a queue-depth bound. It also made the ingress budget shareable across replacement controllers: a session-level rate limit was otherwise reset by rebuilding a controller during reconnect. Earlier [0fefff7a](https://github.com/aizlabs/srui/commit/0fefff7a) bounded the retained dedupe-client map and other transport behavior.

This was strong follow-through, but the dimensions and lifetime of the budget should have been part of the initial design.

**Practice:** for every queue or cache, name its byte bound, item bound, identity bound, timeout, owner, and reset condition. Ask what happens when requests are tiny, identities churn, connections fail, or controller objects are replaced. Security and reliability share these questions; they should enter feature acceptance before a late hardening ticket.

**6. Real consumers and the real platform are part of the specification**

The examples exposed limits and renderer behavior that isolated tests missed. The process monitor needed chunked initial model insertion in [e485f4b3](https://github.com/aizlabs/srui/commit/e485f4b3), and safer client-specific selections and process signaling in [0113063d](https://github.com/aizlabs/srui/commit/0113063d).

The [PR #49 review record](https://github.com/aizlabs/srui/pull/49) describes a menu test that passed by calling ControlFactory directly while production LayoutRenderer routed the property elsewhere. It also records real AppKit behavior that invalidated assumptions about popup titles and duplicate labels. Several collection/text commits explicitly had to restore macOS compilation.

What went well: native examples became demanding integration tests. What could improve: production dispatch and native type checking should be exercised before a feature acquires layers of dependent work.

**Practice:** require a minimal native consumer for every new semantic capability. Add at least one test through the same dispatch path the application uses. Keep Linux-portable logic separate, as the LogicalChannelScheduling extraction did, but label Swift parsing as syntax-only. A Linux parser pass cannot certify AppKit behavior or Swift type correctness.

**7. Validate the instrument before investing in the measurement campaign**

The benchmark branch, [PR #49](https://github.com/aizlabs/srui/pull/49), carried 49 non-merge commits. Its result is impressively explicit about workload identity, presentation evidence, sample counts, process attribution, and reporting. Its long correction sequence also shows how expensive it is to build a broad harness while the meaning of a measurement is still being established.

The [instrumentation findings](https://github.com/aizlabs/srui/blob/da18fba344b123886be6dd1063c762656f6d480f/benchmarks/parse-render/INSTRUMENTATION_FINDINGS.md) record rejected xctrace assumptions, WebKit attachment perturbation, and a malloc-history export of approximately 1.9 GB before the workload. The final approach carefully distinguishes compositor-visible paint from smoke diagnostics, display timestamps from callback receipt, and net live allocation deltas from cumulative allocation calls. Cumulative allocation-event counting remained a declared follow-up in [issue #48](https://github.com/aizlabs/srui/issues/48).

The best decision here was narrowing claims to what the evidence supported and preserving the failed experiments for future maintainers.

**Practice:** start each metric with a one-page contract: quantity, scope, start/end boundaries, perturbation, and falsification experiment. Prove one sample valid before building a full dashboard or recording a baseline. Split methodology, harness construction, and baseline publication into separately reviewable deliverables. Never make a performance number look better by silently changing what it measures.

**8. Parallel agents need integration boundaries and curated outputs**

The early coverage campaign contributed 24 “swarm(Txx): agent changes” commits. It expanded coverage quickly, but those subjects reveal almost nothing about behavior or intent. Fuzz corpus also dominated the apparent churn: [827d6d1c](https://github.com/aizlabs/srui/commit/827d6d1c) touched 7,094 paths, 7,071 under the fuzz corpus. That is why a raw additions/deletions chart would badly misrepresent this project's engineering activity.

Later Process Explorer work added dedicated worktrees, explicit ticket ownership, implementation/verification separation, and durable orchestration state in [PR #51](https://github.com/aizlabs/srui/pull/51). That is a promising response. The merged record only contains its initial tickets, so it is too early to claim the new workflow has demonstrated sustained superiority.

**Practice:** parallelize work with independent inputs and clear file/interface ownership. Keep one owner for changes to shared lifecycle contracts. Require each contribution to explain behavior, evidence, and generated output. Curate fuzz seeds and keep disposable artifacts out of ordinary change review. Independent verification is useful when it can reject the implementer's assumptions, not merely repeat the implementer's commands.

**9. Human supervision has highest leverage at decisions about meaning and evidence**

The review trail shows a productive interaction between automated review and maintainer-account responses. It does not establish the human hours behind those responses.

Two examples show the desired judgment. In [PR #47](https://github.com/aizlabs/srui/pull/47), a response acknowledges that an earlier emits matrix silently widened the protocol contract while claiming the design remained unchanged. The repair restored the spec boundary and exposed genuine gaps. In [PR #49](https://github.com/aizlabs/srui/pull/49), a response explains why converting an internal metric-name invariant failure into a recoverable Result would not improve correctness; the panic contract was documented instead.

Supervision should include the ability to reject a suggested change, revise a requirement explicitly, or admit missing evidence. Resolving every comment mechanically is not an engineering objective.

**Practice:** the human owner approves product semantics, compatibility policy, important resource budgets, measurement claims, and remaining limitations. Agents can implement, probe, test, and challenge those decisions. Require a concise evidence packet for high-risk acceptance: intended behavior, strongest counterexample, test result, unresolved uncertainty, and exact commit. After repeated reviews discover variants of the same defect, pause for a model/ownership review rather than continuing an unlimited patch loop.

**10. Turn operational failures into executable safeguards, and keep those safeguards proportionate**

[f6eeab0d](https://github.com/aizlabs/srui/commit/f6eeab0d) documents an editing tool preserving modification times, causing Rust or Swift builds to run stale artifacts. [1d8766d8](https://github.com/aizlabs/srui/commit/1d8766d8) documents leaked sshd processes holding test stdout/stderr open: every test could pass while the runner hung waiting for EOF.

These are valuable lessons because they undermine the verification mechanism itself. The repository responded with explicit guidance, centralized process helpers, and a stdio check.

But accumulated safeguards can also make small tasks disproportionately slow. [PR #56](https://github.com/aizlabs/srui/pull/56) replaced unconditional repository-wide pre-push work for Process Explorer-only changes with affected-component checks, while retaining broader checks for shared or unknown changes and leaving full CI/merge protection in place.

**Practice:** every costly operational surprise should yield a reproduction, an explanation, and a focused guard where practical. Verify process cleanup and build freshness as properties of the harness. Select checks from changed dependencies and acceptance criteria, with a conservative fallback when the dependency boundary is uncertain. Measure check runtime and fault detection before adding more mandatory gates.

**How I would assess the major tasks**

These are qualitative assessments of the recorded implementation process and evidence, not numerical code-quality scores.

| Task | Assessment | Why |
|---|---|---|
| Scaffold through initial end-to-end client/server | Strong discovery velocity | A real vertical path arrived in about a day; later lifecycle fixes show it was a starting point rather than final acceptance |
| Transaction delivery forms, PR #27 | Strong architectural correction | Explicit types and staged admission addressed a multi-module consistency failure |
| Virtualized collections, PR #37 | Strong verification practice with integration gaps | Mutation testing improved boundary assertions; dropped requests and macOS compilation still required follow-up |
| Native text editing, PR #39 | Necessary capability, costly convergence | Many interacting identities and asynchronous states needed repeated correction; ownership modeling should have led the task |
| Security ingress, PR #44 | Strong eventual dimensional reasoning | Follow-up bounded queued objects and kept budgets attached to session lifetime |
| Conformance consolidation, PR #47 | Weak initial evidence, strong recovery | Self-confirming fixtures and vacuous cases were replaced by behavioral tests and honest gap accounting |
| Benchmark suite, PR #49 | Strong final claim discipline, oversized implementation/research batch | Measurement assumptions, platform behavior, product defects, and baseline publication were corrected together |
| Connection manager, PR #52 | Useful user-facing integration, substantial hidden preparation | Short PR age obscures 22 commits of continuity and lifecycle work |
| Process Explorer verification scope, PR #56 | Promising workflow improvement | Focused checks preserved broad fallbacks; long-term effect is not yet established |

The repeated review fixes deserve two interpretations simultaneously: reviewers found valuable problems, and some classes of problem were being discovered too late in task execution. Many were corrected before merge. Neither “AI produced hundreds of bugs” nor “review made everything safe” is supported by this history.

**A practical improvement program**

For the next two weeks, apply the following only to work touching state, concurrency, protocol semantics, security boundaries, or measurement. Keep simple changes simple.

1. Before implementation, record one user scenario, one ownership/transition table where relevant, the source-of-truth contract, and the most likely falsifying case.
2. Before acceptance, require one production-path test and evidence that the important negative assertion can actually fail.
3. Limit concurrent changes to the same lifecycle owner. Parallelize independent leaves; explicitly sequence shared contract changes.
4. Treat a second recurrence of the same failure class during review as a signal to revisit the model. This is a proposed team policy, not a threshold measured from the history.
5. Keep an evidence ledger for accepted limitations and disputed reviewer suggestions, with owners and closure criteria.
6. Record check selection and duration, and track repairs after acceptance separately from corrections made before acceptance.

For the next month, measure accepted scenarios, median and upper-tail time to acceptance, repeated failure classes, defects found after acceptance, review waiting time, and flaky/hung verification runs. Human supervision time requires explicit recording; Git timestamps cannot supply it.

The project already contains the ingredients of a stronger process. The improvement is to make its best late discoveries—the typed contracts, adversarial tests, honest metrics, explicit ownership, and proportionate checks—the default starting conditions for the next feature.

**Evidence and reproduction**

Primary repository documents: [implementation plan](https://github.com/aizlabs/srui/blob/da18fba344b123886be6dd1063c762656f6d480f/SRUI_Implementation_Plan.md), [engineering guidance](https://github.com/aizlabs/srui/blob/da18fba344b123886be6dd1063c762656f6d480f/CLAUDE.md), [benchmark contract/operations](https://github.com/aizlabs/srui/blob/da18fba344b123886be6dd1063c762656f6d480f/benchmarks/README.md), and the linked commits/PRs above. The T0–T38 plan was committed on 7 September; its current presence does not prove exactly when its contents were first written or used.

Core census commands:

```sh
git fetch origin
git rev-parse origin/main
git rev-parse --is-shallow-repository
git rev-list --count --all
git rev-list --count origin/main
git rev-list --count --no-merges origin/main
git log --all --format='%H%x09%P%x09%aI%x09%cI%x09%an%x09%s'
git log origin/main --no-merges --numstat --format='COMMIT%x09%H'
gh pr list --state merged --limit 100 \
  --json number,title,createdAt,mergedAt,additions,deletions,changedFiles,author
```

For an ordinary two-parent PR merge M, the branch contribution count used here is:
`git rev-list --count --no-merges M^1..M^2`.
It counts branch ancestry newly reachable through that merge, not distinct engineering tasks or review cycles. GitHub's merged-PR list and Git's merge-commit count are different populations; some merged history is represented by different commits, and Git also contains non-PR merges.

Verification performed for this report: refreshed Git refs; confirmed the repository is not shallow; reconciled all-ref/merged/non-merge populations; measured changed-path and churn statistics; fetched merged-PR metadata; inspected selected patches and review evidence. Only this report was added in a dedicated documentation worktree; no product files were changed and no product tests were run.
