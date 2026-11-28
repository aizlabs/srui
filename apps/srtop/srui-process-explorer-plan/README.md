# SRUI Process Explorer — agent implementation pack

Updated 12 September 2026 • Edition 1.2

## Start here

Read [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md), [BASELINE.md](BASELINE.md), and [EXECUTION_TIMELINE.md](EXECUTION_TIMELINE.md). Use [TASK_INDEX.md](TASK_INDEX.md) or `task-index.json.execution_order` to choose a ticket. IDs are stable and no longer express execution order. Give one standalone ticket to an implementation session with the authoritative repository design.

The product lives under `apps/srtop`; preserve the runnable `examples/process-monitor` example and reuse verified components. PX-000 records the revision-specific baseline. The PX-001 shell now has implementation test evidence in [its completion record](docs/process-explorer/completions/PX-001.md), including real localhost SSH and retained native handles. Independent verification is still required; ticket-index statuses and broader release gates are not advanced by this implementation evidence.

PX-002 now has deterministic fake-source implementation evidence in [its completion record](docs/process-explorer/completions/PX-002.md): three model-backed rows over native localhost SSH, distinct duplicate names and explicit missing values. Its individual ledger remains `in_progress` until independent verification; broader release gates and task-index statuses are unchanged.

PX-003 now has live Linux implementation evidence in [its completion record](docs/process-explorer/completions/PX-003.md): one real `/proc` snapshot on an Ubuntu-kernel host that locates a test-owned sleeping worker, `ProcessKey` instance identity including a boot-tick creation token, incomplete scans kept distinct from authoritative empty results, and sanitized hostile process names. Its individual ledger remains `in_progress` until independent verification; broader release gates and task-index statuses are unchanged.

PX-026-G01, the installable-R1 package gate, has implementation evidence in [its completion record](docs/process-explorer/completions/PX-026-G01.md) and release notes in [R1.md](docs/process-explorer/releases/R1.md). It covers a build-from-source package, live compatibility, host-key, authentication, reconnect and replacement checks against the devbox, a repeatable privacy audit, and the PX-010-G01 support envelope. PX-026-G02, the server's reasoned handshake refusal, is merged in, and every live check passes. The gate stays open until the user returns two results: the clean-account walkthrough and the VoiceOver checklist (`apps/srtop/r1/`). Its ledger marks both `blocked: awaiting user evidence`.

PX-031 completes the implementation of the safe single-target slice (`delivery_milestones.safe_termination`; the release map's "Confirmed SIGTERM and observed outcome", gate: none): PX-028's authorization, PX-029's process handles, PX-030's confirmed SIGTERM and PX-031's observed outcome, which reports a sent request separately from what srtop's own scans then show (exited after the request, still running, or outcome unknown, never escalated), settles every result through the runtime's event result cache, and abandons every intent on a restart. Its [completion record](docs/process-explorer/completions/PX-031.md) has the evidence; HTOP-R1-001 is `in_progress` until independent verification. The plan defines no separate release record for this slice, and the R1 user evidence above is still outstanding.

PX-095 adds multiple isolated host windows to the generic connection manager: several sessions run at once, each in its own session context and attempt generation, and every window names its host, account and session incarnation in client-owned chrome. Two hosts serving identical PIDs, names and item identities share no selection, event sequence, cached identity or confirmation, and replacing one session leaves the other running. Its [completion record](docs/process-explorer/completions/PX-095.md) has the evidence; `SRUI-PX095-001`..`005` are `in_progress` until independent verification. It was implemented with the R1 user evidence still outstanding. No side-by-side metric comparison view was built.

PX-070-G01 specifies the later btop-inspired dashboard before any of it is built: btop++ `v1.4.8` (commit `cf1da590`, Apache-2.0) is the pinned reference, htop 3.5.3 stays the process-management baseline, and [the specification](docs/process-explorer/design/dashboard.md) gives every core panel (CPU, memory/swap, disk, network) a collector owner, renderer owner, unit and freshness policy, finite budget and accessible fallback, and defines the evaluation tasks and pass/fail criteria PX-073-G02 runs. The missing standard `Split` container becomes PX-070-G02, a new prerequisite of PX-073-G01. Neither R1 nor safe termination depends on any dashboard ticket. Its [completion record](docs/process-explorer/completions/PX-070-G01.md) has the evidence; `SRUI-PX070G01-001`..`005` are `in_progress` until independent verification. No product code changed.

PX-070-G02 adds the generic standard `Split` container that dashboard needs. With the user's recorded approval (2026-11-28) the registry gains one property, `split_axis`, and one enum, `SplitAxis` (`side_by_side` by default, `stacked`). It is admitted only on a `Split` and refused at decode and apply when its value is unknown; a `Split` holds at most 8 panes. The SDK gains a `Split` builder. The AppKit client renders it as `NSSplitView`. Each child is a pane, sized from its own layout hints. A collapsed child collapses its pane. Divider positions stay on the client: the user drags a divider or adjusts it through accessibility, nothing is sent, and the positions survive content commits and remounts. srtop does not use it yet (PX-073-G01 will). Its [completion record](docs/process-explorer/completions/PX-070-G02.md) has the evidence; `SRUI-PX070G02-001`..`004` are `in_progress` until independent verification.

PX-071 specifies the optional metrics profile the dashboard's graphs will use, before any of it is built. [The specification](docs/process-explorer/design/metrics-profile.md) names it `io.github.aizlabs.srui.metrics/1`, a project-owned name rather than `org.srui`, and the server assigns its namespace through ordinary negotiation. It defines numeric gauges and graphs of up to 18 sampled series, each series with its own identity, unit, timestamps and explicit gaps, all expressed with existing SRUI models and operations. Each graph falls back to one bounded table of recent samples for clients without the profile. Eight documentation fixtures cover the schema review. The profile needs no protocol or registry change, and no graph is enabled. Its [completion record](docs/process-explorer/completions/PX-071.md) has the evidence, including one open finding for the dashboard's 4 KiB/s traffic bound; `SRUI-PX071-001`..`005` are `in_progress` until independent verification.

## Delivery priorities

1. PX-000–PX-008: real read-only monitor.
2. PX-042, PX-009/PX-010, PX-010-G01: typed fields and early scale/progress/budget gate.
3. PX-068 and remaining read-only navigation/connection tickets.
4. PX-026-G01 and PX-027: installable R1 with compatibility, privacy, accessibility, and measured support envelope.
5. PX-028–PX-031: safe single-target termination with truthful outcomes.
6. Deliver the later btop-inspired dashboard through PX-070-G01, PX-073-G01, and PX-073-G02: CPU/memory/disk/network graphs, the searchable table, sampled-history inspector, and isolated host windows. GPU and detailed per-CPU panels are optional additions.
7. Extend with comparison/export/guided workflows. Advanced actions, optional collectors, and Linux parity have their own dependencies and gates.

PX-035-G01 resolves advanced-action feasibility before enabling those operations. Linux coverage and its packaged release remain PX-088/PX-089. Privileged helpers, PCP, other OSes, terminal-only use, and external automation need separate prototypes, estimates, and live environments.

## Files

- `IMPLEMENTATION_PLAN.md`: constraints, releases, and all specifications.
- `BASELINE.md`: revision-specific source/test evidence and unresolved gaps.
- `EXECUTION_TIMELINE.md`: approximate effort, assumptions, and re-estimation points.
- `TASK_INDEX.md`: priority queue and dependency catalogue.
- `task-index.json`: structured specification, dependency graph, and execution order.
- `tickets/`: 140 original IDs plus six suffixed tickets, each self-contained.
- `feature-ledger.seed.json`: 48 planning families with zero verified parity claims.
- `feature-ledger.px000.json`: ten individually scoped htop 3.5.3 entries for R0/R1 and safe termination; all remain planned until evidence is attached.
- `feature-ledger.px001.json`: individually scoped empty-shell implementation evidence against the SRUI design; no htop parity claim.
- `feature-ledger.px002.json`: individually scoped deterministic fake-source implementation evidence; no live collection or htop parity claim.
- `feature-ledger.px003.json`: individually scoped live Linux one-shot snapshot and process-instance identity evidence; no refresh loop, action or htop parity claim.
- `validate_feature_ledger.py`: schema, duplicate-ID, upstream-reference, owner, status, and evidence checks for the PX-000 ledger.
- `STANDING_AGENT_CONTRACT.md`: shared implementation rules.
- `SOURCES.md`: design and upstream reference provenance.
- `validate_plan.py`: ticket, dependency, execution-order, shared-contract, and feature-ownership consistency checks; excludes general Markdown link/fence linting.
- `VALIDATION_REPORT.md`: planning validation and its limits.

The htop baseline remains 3.5.3; no fresh upstream release check was performed for edition 1.1. Rebaseline explicitly for newer releases. The adjacent `../IMPLEMENTATION_PLAN.md` points to this canonical planning pack.

The dashboard takes inspiration from btop++; htop remains the process-management coverage baseline. R1/safe termination stay unchanged. Selected workflow comparisons require a pinned btop version and evidence; this plan makes no full btop-parity or blanket superiority commitment.
