# Approximate execution timeline

Planning estimate, 12 September 2026. These are engineering judgment ranges, not measured ticket throughput or a delivery promise. Start dates are relative to kickoff at the chosen, audited revision.

## Assumptions

One experienced maintainer working full time with coding-agent assistance, reviewing and integrating changes in one implementation lane. A Mac and a disposable Linux host are available from kickoff. No automatic speedup from parallel agents is assumed. Estimates include implementation, review, deterministic tests, native/Linux verification, and modest rework. Calendar waits for hardware, approval, signing credentials, or access are additional.

The passing existing-example tests justify reuse, but they were run at an older revision than this PR's base; generic UI/range gaps are not yet fully audited at the newer revision. Re-estimate after PX-000 and PX-010-G01. All acceptance gates remain binding regardless of elapsed time.

## Initial delivery

| Stage | Scope | Approximate effort | Cumulative target |
|---|---|---|---|
| Baseline refresh | PX-000, current revision, reuse map, initial ledger, environment checks | 1–2 working days | Week 1 |
| R0 read-only monitor | PX-001–PX-008, reuse plumbing, correct identity/availability, Linux sample verification | 3–6 working days | Weeks 1–2 |
| Early data/scale gate | PX-042, PX-009/PX-010, PX-010-G01, PX-068 | 3–7 working days | Weeks 2–3 |
| Useful explorer and reconnect | PX-011–PX-026: inspector, navigation, trees/threads, last observed state, connection UX | 6–12 working days | Weeks 3–6 |
| Installable R1 | PX-026-G01 and PX-027, packaging, compatibility, privacy/accessibility, native release evidence | 3–5 working days | Roughly weeks 4–7 |
| Safe termination | PX-028–PX-031, immutable confirmation, handle identity, dedupe/uncertain outcomes, owned-worker tests | 4–8 working days | Roughly weeks 5–9 |

R1 totals about 16–32 working days, rounded to 4–7 calendar weeks with integration overhead. Safe termination adds about 1–2 weeks. These ranges assume reusable runtime capabilities work as expected. A material protocol/renderer gap can add 1–3 weeks or more; create a scoped prerequisite and revise the forecast when discovered.

The original nine R0 tickets are small acceptance slices and may reuse existing evidence/code. They are not nine equal-sized development sessions.

## After safe termination

Prioritize the core dashboard after safe termination, then choose follow-on increments. These ranges overlap; do not add every row as if they were disjoint commitments.

| Increment | Additional approximate effort | Conditions |
|---|---|---|
| Multiple isolated hosts, PX-095 | 3–7 working days | R1 and safe-termination behavior verified; session isolation may reveal generic client work |
| Core btop-inspired dashboard through PX-073-G02 | 3–5 weeks | Includes design, basic host collectors, generic graphs/configuration, sampled process history, and isolated hosts; reuses earlier work; GPU/detailed CPU enhancements excluded |
| Dashboard plus comparison, export, guided advantages through PX-097 | 4–8 weeks total after safe termination | Includes the preceding dashboard estimate; excludes parity and advanced administration |
| Advanced controls through PX-041 | 2–5 weeks if target strategies are feasible | Start with a 2–5 day PX-035-G01 investigation; blocked operations have no reliable completion date |
| Broad ordinary-user Linux coverage and packaging through PX-089 | Roughly 3–6 additional months | Substantial monitoring/diagnostic/optional-provider work; revise against the individual ledger and actual equipment |

Optional GPU/vendor, ZFS, OpenRC, and similar live verification can introduce equipment-dependent waits. A release can describe its narrower verified scope honestly; missing evidence cannot be relabeled full parity.

The core dashboard is approximately weeks 8–14 from kickoff under the same assumptions. PX-070-G01 must re-estimate after pinning the btop reference and auditing generic graph/layout needs. Existing R1 (4–7 weeks) and safe-termination (5–9 weeks) estimates are unchanged. Optional GPU/vendor and detailed per-CPU panels receive separate estimates and do not delay the four-panel release.

## Longer branches

Privileged helpers, PCP, terminal rendering, external automation, and each additional host OS need separate feasibility and sizing. Plan a 3–10 working-day discovery/prototype budget per branch before scheduling production work; some will need several weeks or months afterward.

Complete cross-platform/deployment parity is a rough 12–24+ month planning horizon for one maintainer, with low confidence and no finite promise for unsupported identity guarantees or unavailable platforms. Do not add that horizon to the short milestones as if all scopes had already been estimated.

## Re-estimation checkpoints

- After PX-000: confirm source revision, reuse, generic widget gaps, and Linux/Mac access.
- After PX-010-G01: commit to a measured process-count/rate/network support envelope; price any runtime changes.
- At R1: use observed ticket throughput and field feedback to prioritize multi-host/history versus coverage.
- After PX-035-G01: estimate only technically supported action families.
- Before each parity/OS branch: inventory its exact features, test equipment, review needs, and lifecycle maintenance cost.
