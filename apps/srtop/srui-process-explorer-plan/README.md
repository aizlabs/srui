# SRUI Process Explorer — agent implementation pack

Updated 12 September 2026 • Edition 1.2

## Start here

Read [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md), [BASELINE.md](BASELINE.md), and [EXECUTION_TIMELINE.md](EXECUTION_TIMELINE.md). Use [TASK_INDEX.md](TASK_INDEX.md) or `task-index.json.execution_order` to choose a ticket. IDs are stable and no longer express execution order. Give one standalone ticket to an implementation session with the authoritative repository design.

The product lives under `apps/srtop`; preserve the runnable `examples/process-monitor` example and reuse verified components. PX-000 records the revision-specific baseline. The PX-001 shell now has implementation test evidence in [its completion record](docs/process-explorer/completions/PX-001.md), including real localhost SSH and retained native handles. Independent verification is still required; ticket-index statuses and broader release gates are not advanced by this implementation evidence.

PX-002 now has deterministic fake-source implementation evidence in [its completion record](docs/process-explorer/completions/PX-002.md): three model-backed rows over native localhost SSH, distinct duplicate names and explicit missing values. Its individual ledger remains `in_progress` until independent verification; broader release gates and task-index statuses are unchanged.

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
- `validate_feature_ledger.py`: schema, duplicate-ID, upstream-reference, owner, status, and evidence checks for the PX-000 ledger.
- `STANDING_AGENT_CONTRACT.md`: shared implementation rules.
- `SOURCES.md`: design and upstream reference provenance.
- `validate_plan.py`: ticket, dependency, execution-order, shared-contract, and feature-ownership consistency checks; excludes general Markdown link/fence linting.
- `VALIDATION_REPORT.md`: planning validation and its limits.

The htop baseline remains 3.5.3; no fresh upstream release check was performed for edition 1.1. Rebaseline explicitly for newer releases. The adjacent `../IMPLEMENTATION_PLAN.md` points to this canonical planning pack.

The dashboard takes inspiration from btop++; htop remains the process-management coverage baseline. R1/safe termination stay unchanged. Selected workflow comparisons require a pinned btop version and evidence; this plan makes no full btop-parity or blanket superiority commitment.
