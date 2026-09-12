# SRUI Process Explorer — implementation plan

**Edition:** 1.2 • **Updated:** 12 September 2026
**Starting point:** SRUI design v0.6; real process-monitor example and partial source/test baseline in [BASELINE.md](BASELINE.md)
**Initial product:** Rust application on a Linux host, generic Swift/AppKit client over existing SSH SRUI
**Roadmap:** 140 original IDs plus 6 suffixed tickets; deliver R1 and safe termination before broad parity
**Pinned comparison:** htop 3.5.3, the release marked Latest by upstream when this plan was prepared [H1]

## 1. Goal and limits of this plan

Evolve verified pieces of the existing process-monitor example into a useful Linux-host/macOS-client explorer under apps/srtop. Preserve the example; the product starts read-only. Initial tickets describe acceptance slices, not a requirement to rewrite working plumbing. Deliver installable R1, then safe single-target termination. Broader htop scopes are later commitments.

**PX-000–PX-008** produce R0. Before further feature expansion, run PX-042, PX-009/PX-010 and PX-010-G01, then PX-068 for basic columns. The installable R1 gate is **PX-027**, including PX-026-G01 packaging/privacy/native verification. **PX-031** adds confirmed single-process termination independently of advanced actions. PX-095 and PX-090–PX-094 can follow without waiting for Linux parity. PX-088 remains the later coverage audit.

Edition 1.1 incorporated a partial checkout/source audit and a passing process-monitor test command; see BASELINE.md for exact revision and limits. It does not certify all runtime capabilities or live Linux/native behavior. PX-000 refreshes evidence at the implementation revision and records missing reusable capabilities. The authoritative design is unchanged.

“Supersede everything in htop” needs several honest milestones. Ordinary-user Linux functionality is one scope; optional builds/hardware, privileged administration, PCP, other host OSes, and terminal-only deployment are additional scopes. A native presentation may replace a terminal keystroke or color with a functionally equivalent accessible control. It may not replace missing data/actions with a decorative placeholder and call that parity. Full functional/deployment coverage is claimable only after all declared scopes pass. A better user experience is demonstrated separately, not proved by counting features.

## 2. How to hand work to coding agents

Use one fresh coding-agent session per ticket. Attach the authoritative SRUI v0.6 design, give the agent the repository, and paste one file from `tickets/`; every file repeats the standing execution contract and its own build/verification scope. `TASK_INDEX.md` is the human queue and `task-index.json` is the machine-readable queue.

Execute the explicit execution_order in task-index.json, honoring depends_on. Ticket numbers are stable identifiers, not execution order. Branch K, L, M, N, and O work starts only after its named prerequisites; different OS branches do not depend on one another. Branches are optional for the first release, **not optional for an eventual claim that includes their feature/deployment scope**. Optional hardware collectors must build and fail gracefully on ordinary machines, but missing live hardware evidence is recorded as unverified.

At completion, record code paths, tests, command outputs, manual/native evidence, and unresolved limitations. A passing unit test against a fake source does not by itself verify an OS API or native renderer. Commit a bounded change only after required verification; never silently relax the gate because a runner is unavailable.

If a ticket exposes a missing reusable widget/protocol capability, or a platform family is larger than one coherent change, append suffixed tickets such as `PX-088-G01` with explicit dependencies. Do not renumber this edition. Collector, UI, and live-verification subchanges may be separated. Parent gates stay open until the new prerequisites pass. The parity/platform expansion tickets intentionally provide this mechanism rather than claiming a fixed list can anticipate every optional build.

**Suggested module responsibilities, not mandatory new crates:** process source; snapshot/identity model; metric catalogue; view/query projection; action policy and executor; diagnostic jobs; settings; sample history; SDK view composition. Start as small modules within the existing app; split crates only when it improves boundaries. Shared client changes belong in generic rendering/presentation/connection modules. The app must not acquire process-specific Swift code.

### Delivery order and additional decisions

The initial path and optional branches are enumerated in [TASK_INDEX.md](TASK_INDEX.md); estimates and assumptions are in [EXECUTION_TIMELINE.md](EXECUTION_TIMELINE.md). Basic column configuration and packaging must not wait for GPU/ZFS/service-manager coverage. Source collection is shared where appropriate, but query state, selection, outboxes, and action intent need explicit client/session ownership; PX-000 must establish that boundary before product expansion.

PX-010-G01 proves progress under continuous updates, not only rejection of stale ranges. PX-026-G01 gates privacy, accessibility, version compatibility, and clean installation before R1. PX-035-G01 resolves each advanced action's targeting strategy before enabling it. Unresolved strategies remain disabled and block corresponding parity claims, not PX-031.

Terminal, privileged-helper, PCP, external-automation, and additional-OS branches require their own feasibility prototype, environment matrix, bounded child tickets, and estimate. No branch is considered small merely because it has few parent tickets. The original 140 IDs remain unchanged; new prerequisites use suffixes.

### Later dashboard direction: btop inspiration

Keep htop 3.5.3 as the process-management baseline. Use [btop++](https://github.com/aristocratos/btop) as inspiration for a later native dashboard, not a full-parity commitment. Pin a reference version and selected workflows in PX-070-G01; this amendment makes no freshly verified upstream feature/version claim.

Combine CPU, memory/swap, disk, and network graphs with the searchable table and sampled process-history inspector. Reuse PX-054–PX-056 collectors, PX-069/PX-070 configuration, PX-071–PX-073 generic metrics, and PX-090 history. PX-054/PX-055/PX-056 now depend directly on the catalogue/basic summary instead of unrelated advanced metrics. GPU and detailed per-CPU panels arrive with verified collectors and do not block the core dashboard.

PX-070-G01 specifies the workflow/evaluation, PX-073-G01 composes it, and PX-073-G02 gates **R-Dashboard**, including isolated host windows from PX-095. This follows safe termination and does not block R1. PX-097 combines it with comparison/export/guided workflows. Panels expose sample intervals, units, independent timestamps, gaps, and stale/disconnected states. Layout remains client-local; process policy remains server-side.

Compare defined workflows and measured overhead against the pinned btop build before claiming specific advantages. Host disk/network activity does not establish process attribution; independently sampled panels are not an atomic snapshot.

## 3. Non-negotiable implementation rules

### Authority and UI

Process enumeration, metric interpretation, filtering policy, identity, and actions stay server-side. Use one collection model and reusable native views, not one view node per process. Query/range replies carry sufficient generation/revision context to reject stale data. Ordinary typing, caret, IME, selection highlights, cached scrolling, and local layout do not wait for a round trip. Server-dependent search/inspection results still take time. No downloaded client code and no frame protocol. [D1 §§4–8, 12, 22]

Keep presentation-only preferences local. Server-side named application views may store semantic column/filter/meter choices, but not local window geometry, credentials, or render objects. Only an explicitly approved generic client behavior may derive a local age/countdown display; no app-specific logic is smuggled into the renderer. [D1 §§6.3, 22.1]

### Identity and actions

A process record is not identified by PID alone. Use host/source identity, boot and namespace context, PID, and a high-resolution native creation token. Distinguish a logical ItemId from an OS task identity and never give a replacement task the old intent. A precise display timestamp is not necessarily a sufficiently precise creation token. [D1 §6.2; K1]

Confirmations bind immutable target sets, operation, parameters, session, and expiry to a fresh action node/intent. Do not execute “whatever is selected now.” Revalidate at execution without treating every unrelated telemetry revision as a reason to reject. Kernel pidfd signaling addresses a different problem from PID-only priority/affinity calls; those need their own reviewed strategy. Merely opening a handle or prechecking PID/start time does not make every subsequent PID-based API safe. [K1–K5]

A delivered signal is a request, not proof of exit. A bulk operation is not atomic across OS processes. Report partial results and uncertain outcomes. Surviving-session deduplication protects settled retries; an OS effect and an in-memory result cache are not a universal atomic transaction across handler abort or daemon crash. Never claim crash-proof exactly-once process effects. New incarnations abandon old pending intent. [S1; D1 §18.2 and Appendix B]

### Continuity, security and limits

Use T23/T24 instead of adding app retries. A remembered revision is useful only with a corresponding committed replica. A saved client identity is reusable only with the necessary event allocation/frontier/outbox state; otherwise use the documented fresh-identity and snapshot path. A superseded reconnect attempt is inert. [D1 §§17–18; D2 T23–T24, T38]

Keep the daemon and bridge unprivileged. Advanced admin helpers, external local IPC, and any new local-resource capability need separate review and are not implied by T32/T35. All tools use fixed argument-vector interfaces and bounded output; no shell interpolation. Historical/paused/disconnected views cannot initiate process-changing actions. Secrets are not collected or persisted merely because an API exposes them. [D1 §§25–27; D2 T32, T35]

Use finite queues/caches and honest unavailable states. Coalesced revision spans contain only permitted scalar SET_PROPERTY operations; MODEL_UPDATE and structural traffic cannot be silently merged using that rule. Reduce source sampling/projection or follow detach/resync policy instead. [D1 §§12.1, 20.4, 26]

## 4. Proposed defaults and test discipline

These are starting configuration choices, **not measured performance claims**. Keep them configurable and bounded; the runtime's stricter negotiated limits always win.

| Concern | Initial choice |
|---|---|
| Ordinary sampling | 1 second; later validated interval from 0.1 to 10 seconds |
| Expensive metrics | Selected/enabled fields only; slower independent collection tier |
| Views | One host, one account, one current view, required-tier widgets |
| Actions | Read-only first; explicit enable policy, immutable confirmation, short finite intent TTL |
| Model load | Entire small process set initially; explicit range virtualization before large-model claims |
| Stress fixtures | 10, 1,000 and 100,000 synthetic rows; no artificial creation of 100,000 real processes |
| Retained history | Off initially; bounded selected-process samples, initially at most 15 minutes |
| Buffers | Initial measured caps at PX-010-G01; each later feature adds caps immediately; PX-085 consolidates/stresses them |
| Network tests | 0/100/300/600 ms RTT; dropped links; bounded bandwidth; newest-resume wins |
| OS-side test safety | Disposable owned workers, hard time/resource bounds, temp files, cleanup handles |
| Optional tools/hardware | Disabled or unavailable cleanly; fixtures plus a named live test before verified status |

Use fake clocks and recorded counters for exact assertions. Live metrics require documented sampling-window tolerances; two separately sampled tools need not show identical instantaneous values. Record API/kernel/library versions and which fields were enabled. A static fake source is the zero-app-traffic test, not a changing live machine. A benchmark comparing htop must match sampling/fields/workload and distinguish payload bytes from SSH overhead. Do not assume a semantic GUI necessarily uses less bandwidth than an efficient terminal application. [D1 §31; H1]

## 5. Release map

| Release/gate | Ends at | User-visible result | Explicitly not claimed |
|---|---|---|---|
| R0 | PX-008 | A real read-only CPU/RSS process list | Controls, advanced diagnostics, parity |
| R1 | PX-027 + suffixed gates | Installable searchable/inspectable explorer, columns, reconnect, measured support envelope | Full htop coverage or external agent IPC |
| Safe single-target slice | PX-031 | Confirmed SIGTERM and observed outcome | All process administration |
| R2 | PX-041 | Advanced controls after PX-035-G01 resolves operation-specific feasibility | Root/admin equivalence, all optional facilities |
| R3 | PX-088–PX-089 | Versioned Linux feature report and distributable release | Untested optional, PCP, other OS, or terminal-only scopes |
| R-Dashboard | PX-073-G02 | Native CPU/memory/disk/network dashboard, sampled inspector, isolated host windows | Full btop parity, required GPU support, unmeasured superiority |
| R4 (independent of R3) | PX-097 | Evidence-backed history/export/multi-host advantages | Unclosed platform/admin/deployment scopes |
| Admin / PCP / OS / TUI | Branch gates | Expanded parity for each named scope | Any other still-open scope |
| Automation | PX-139 | Reviewed trusted-local action automation | Unrestricted agents or local-resource access |

Before using an unqualified full-coverage claim, the release report must name the pinned htop version and all included build, OS, hardware and privilege scopes. Every corresponding individual ledger entry must be verified or have a justified native presentation equivalent. Upstream-unsupported features may be marked not applicable with evidence. **Missing implementation, missing test equipment and intentional policy restrictions do not become verified feature parity.**

## 6. Phase index

### A. Start almost from zero

**PX-000–PX-008 · 9 tickets · mainline.** Audit/reuse existing plumbing → verify real read-only CPU/RSS monitor.

### B. Make the read-only monitor useful

**PX-009–PX-021 plus PX-010-G01 · 14 tickets · mainline.** Sort, filter, search, inspect, follow, trees, threads, tags, last-observed record.

### C. Prove the SRUI advantages

**PX-022–PX-027 plus PX-026-G01 · 7 tickets · mainline.** Connection UX, continuity, latency demo, in-process automation.

### D. Add safe process controls

**PX-028–PX-041 plus PX-035-G01 · 15 tickets · staged controls.** Policy and identity first; then confirmations, signals, bulk operations, scheduling controls.

### E. Expand monitoring depth

**PX-042–PX-059 · 18 tickets · monitoring branch; PX-042 runs after R0.** Typed fields, memory, I/O, namespaces, CPU detail, pressure and host metrics.

### F. Cover optional Linux facilities

**PX-060–PX-067 · 8 tickets · independent conditional providers; PX-067 integrates them.** GPU, compressed memory, ZFS, service-manager and security-state meters.

### G. Reach customization and visualization parity

**PX-068–PX-075 plus three dashboard tickets · 11 tickets · presentation/dashboard branch; only PX-068 is required for R1.** Columns, saved views, meter layouts, bounded metric graphics, preferences and startup/import.

### H. Add advanced inspection

**PX-076–PX-083 · 8 tickets · mainline with optional tools.** Bounded diagnostics, descriptors, locks, environment, maps, traces and backtraces.

### I. Prove scale, safety, and Linux coverage

**PX-084–PX-089 · 6 tickets · later scale/coverage audits, extending early R1 gates.** Virtualization, budgets, fault tests, privacy, feature-ledger closure, packaging.

### J. Go beyond the baseline

**PX-090–PX-097 · 8 tickets · expansion.** Sampled history, comparisons, threshold observations, export and isolated multi-host use.

### K. Administrative parity without a root daemon

**PX-098–PX-100 · 3 tickets · review-gated branch.** Reviewed privileged helper; operation-by-operation implementation.

### L. PCP and declarative dynamic metrics

**PX-101–PX-103 · 3 tickets · optional-build parity branch.** Separate PCP backend and dynamic-view parity scope.

### M. Monitored-host platform parity

**PX-104–PX-132 · 29 tickets · parallel platform branches.** Portable contract plus macOS, FreeBSD, NetBSD, OpenBSD, DragonFly, Solaris and illumos.

### N. Terminal-only deployment

**PX-133–PX-136 · 4 tickets · deployment parity branch.** Native terminal SRUI client; does not require the macOS GUI.

### O. Permission-gated external automation

**PX-137–PX-139 · 3 tickets · review-gated expansion.** Local consent model, read-only IPC, then scoped actions.

## 7. Feature-coverage map

This is a family-level planning map. `feature-ledger.seed.json` deliberately contains no verified entries. The baseline/audit tickets expand families into individual actions, fields, meters, configuration options and build variants, with upstream references and test evidence. A percentage computed over this family map is not a legitimate feature-parity score.

| ID | Feature family | Scope | Owner tickets |
|---|---|---|---|
| B01 | Basic process list and live incremental sampling | core | PX-003, PX-004, PX-008 |
| B02 | Process-instance identity and stale target handling | core | PX-003, PX-029, PX-086 |
| B03 | CPU/RSS summary and field conventions | core | PX-005, PX-006, PX-007 |
| B04 | Typed sorting and sort reversal | core | PX-009, PX-068 |
| B05 | Incremental filter and search navigation | core | PX-010, PX-011 |
| B06 | User/PID filters | core | PX-012, PX-075 |
| B07 | Command line, executable and merged presentation | core | PX-014, PX-050 |
| B08 | Pause, refresh and configurable interval | core | PX-015, PX-075 |
| B09 | Keyboard, mouse, scrolling and help workflows | core | PX-016, PX-074, PX-075 |
| B10 | Follow and stable table/tree navigation | core | PX-017, PX-018, PX-074 |
| B11 | Process tree and collapse/expand | core | PX-018 |
| B12 | User threads, kernel threads and counts | core | PX-019, PX-043 |
| B13 | Tagging, clear tags, descendants and bulk targets | core | PX-020, PX-035 |
| B14 | Readonly enforcement and signal controls | core | PX-028, PX-030, PX-032, PX-033, PX-034 |
| B15 | Nice, affinity, I/O priority and scheduler controls | conditional/privilege | PX-036, PX-037, PX-038, PX-039 |
| B16 | Autogroup fields and control | conditional/privilege | PX-040 |
| B17 | Identity, scheduling, time, fault and context fields | core | PX-043, PX-044 |
| B18 | Memory field families including PSS/EPSS | core/conditional | PX-045, PX-046 |
| B19 | Per-process I/O accounting and rates | core/conditional | PX-047 |
| B20 | Cgroups, namespaces and container visibility | conditional | PX-048 |
| B21 | OOM, privilege state and replaced executable/library warnings | conditional | PX-049, PX-050 |
| B22 | Delay accounting | optional-build/privilege | PX-051, PX-098, PX-100 |
| B23 | Per-CPU classes, topology, SMT and hotplug | core/conditional | PX-052 |
| B24 | Frequency and temperature | hardware | PX-053 |
| B25 | Host memory, swap, huge pages | core/conditional | PX-054 |
| B26 | Disk and network meters | core/conditional | PX-055, PX-056 |
| B27 | Pressure, descriptors, task counts, host/clock/uptime variants | core/conditional | PX-057, PX-058 |
| B28 | Battery and power source | hardware | PX-059 |
| B29 | Host and process GPU fields/meters | hardware | PX-060, PX-061 |
| B30 | zram, zswap and ZFS ARC variants | optional-build | PX-062, PX-063, PX-064 |
| B31 | systemd, OpenRC and SELinux meters | optional-build | PX-065, PX-066, PX-067 |
| B32 | Selectable/reordered columns and named screens | core | PX-068, PX-069 |
| B33 | Meter placement, hide/show and text/bar/graph/indicator modes | core | PX-070, PX-071, PX-072, PX-073 |
| B34 | Appearance, highlights, labels and layout preferences | core/native-adaptation | PX-074 |
| B35 | Startup flags, environment/config behavior and import | core/native-adaptation | PX-075 |
| B36 | Open files and descriptor information | tool/permission | PX-076, PX-077 |
| B37 | File locks and environment inspection | permission | PX-078, PX-079 |
| B38 | System-call tracing and bounded output | tool/permission | PX-081, PX-082 |
| B39 | Backtrace support | optional-build/permission | PX-083 |
| B40 | PCP metric instances and declarative dynamic views | separate-PCP-scope | PX-101, PX-102, PX-103 |
| B41 | Additional monitored-host operating systems | platform-scope | PX-104, PX-108, PX-112, PX-116, PX-120, PX-124, PX-128, PX-132 |
| B42 | No-GUI/terminal-only deployment and flags | deployment-scope | PX-133, PX-134, PX-135, PX-136 |
| B43 | Administrative workflows requiring additional privileges | privilege-scope | PX-098, PX-099, PX-100 |
| X01 | Semantic interaction and transparent reconnect | SRUI-advantage | PX-022, PX-024, PX-025, PX-026, PX-086 |
| X02 | Last observed state, sampled history and comparisons | beyond-baseline | PX-021, PX-090, PX-091, PX-092 |
| X03 | Threshold observations, export and multiple hosts | beyond-baseline | PX-093, PX-094, PX-095 |
| X05 | Btop-inspired native dashboard and process-history inspection | beyond-baseline/no-btop-parity | PX-070-G01, PX-073-G01, PX-073-G02 |
| X04 | Trusted external semantic automation | beyond-baseline/review | PX-137, PX-138, PX-139 |

## 8. Standing agent contract

The complete individual ticket files repeat this contract. In this master document it is shown once to avoid repetition. Read it together with each ticket.

```text
You are implementing ONE bounded ticket for SRUI Process Explorer, an application built on the existing SRUI runtime. Read this entire ticket before changing code.

AUTHORITATIVE CONTEXT
- SRUI design v0.6 is authoritative and read-only. Read BASELINE.md and PX-000 evidence for the exact checkout: T0–T35 are audit scope, not a blanket completion claim. Reuse the real T21 example where verified. Check T36–T38 capabilities rather than assuming presence or absence.
- Read the ticket's cited design sections, current app code, prerequisite completion notes, and feature ledger. Reuse T21 where safe. Existing T21 process-changing demo code is not automatically suitable for production.
- Initial stack: Rust server/app, generic Swift/AppKit client, existing Protobuf and non-PTY SSH binding. Suggested module names in the plan are roles, not assertions about files in the repository.

WORKING RULES
- Before changing any repository file, verify the branch and worktree. Never edit, generate, format, stage, or commit files in a checkout on main. Use a dedicated task worktree on a new non-main branch based on origin/main. Main changes only through pull-request merges; preserve other worktrees and unrelated work.
- Follow task-index.json execution_order and depends_on, not numeric ticket order. Read BASELINE.md and EXECUTION_TIMELINE.md for scope; time estimates never waive acceptance gates.
- Implement only this ticket. Preserve user work and unrelated examples; no speculative framework rewrite, global formatting, silent dependency upgrade, or following-ticket implementation.
- Keep collection, filtering policy, process identity, authorization and action execution server-side. Standard UI uses the SDK and generic renderer. Never add a process-name/PID-specific client code path.
- Reuse atomic transactions, event settlement, range models, resource limits, and continuity-aware reconnect. No own frame stream/retry stack. MODEL_UPDATE is not eligible for the scalar SET_PROPERTY coalescing rule.
- A numeric PID, row index, or display name is not process-instance identity. Actions bind to immutable server-validated targets, not mutable selection. No shell interpolation. No root sessiond/bridge, no unreviewed privilege helper, and no remote-triggered local file/clipboard/URL access.
- Local typing/scroll feedback stays local; remote results may arrive later. T35 automation is in-process only unless a separately reviewed IPC ticket is being executed.
- Respect configured memory/time/message bounds. Unavailable, denied, warming-up, stale and failed values must not silently become zero. Use deterministic fake sources for correctness and only bounded disposable owned workers for live tests.
- A missing generic protocol/widget facility is a separate narrowly scoped prerequisite with conformance tests. Do not extend standard semantics ad hoc, weaken tests, modify the read-only design, or claim unsupported infrastructure already exists.
- If the task exceeds one coherent change, write suffixed prerequisite/continuation tickets with exact dependencies and acceptance tests before expanding it. Existing IDs are append-only. Review-gated branches require recorded external approval, not agent self-approval.

COMPLETION CONTRACT
Run the repository commands recorded by PX-000, plus the ticket's verification. Add deterministic positive and negative tests; add wire/UI integration tests where the feature crosses those boundaries. Verify relevant SRUI conformance still passes. Record actual commands, results, native/manual evidence, and any unavailable environment in docs/process-explorer/completions/<ticket-id>.md. Update the feature ledger and README where behavior changes. A skipped test or absent OS/hardware is not a pass. On a real blocker, produce a precise blocker report and preserve completed work; do not silently waive the gate. Commit the bounded change only after required checks pass, using a message beginning with the ticket ID. Report changed files, tests, limitations and remaining blockers; do not proceed to another ticket.
```

## 9. Sequential ticket specifications

Specifications are listed by stable ID for lookup. Execute task-index.json execution_order, not this numerical listing.

### Phase A — Start almost from zero

#### PX-000 — Establish the app baseline and a versioned htop feature ledger

**Phase:** A · **Scope size:** S · **Status:** planned · **Gate:** none
**Dependencies:** Revision-specific baseline audit
**Read:** D1 §§4, 7.3, 12.1, 18, 22.9, 29; D2 T21, T28, T33–T35, T38; sources H1–H8

##### Build

Start from BASELINE.md, recording checkout SHA, dirty state, toolchains, and available Linux/macOS runners. The repository contains a real examples/process-monitor app; do not rebuild working plumbing or infer all T0–T35 are complete. Reconcile against the intended implementation revision before estimating gaps. Preserve examples/process-monitor and develop the product under apps/srtop; choose crate/package layout after checking workspace membership. Reuse or extract proven generic seams without changing the example's behavior.
Audit main.rs, monitor.rs, domain.rs, source/terminator code, SDK builders, model_range.rs, and generic collection/rendering capabilities. Map initial tickets to reusable code and missing acceptance evidence. Replace whole-second creation identity before production controls; represent unavailable metrics explicitly; verify typed sort events, query/range progress, selection, tree navigation, dialogs, and reconnect. Record missing generic semantics as bounded prerequisite tickets.
Create the htop 3.5.3 ledger with pinned tag/commit, build/OS/library scope, upstream references, owner, status, and evidence. Prioritize individual entries needed for R0/R1 and safe termination; retain later families as planned inventory owned by PX-088 and branch gates. An all-platform inventory is not a prerequisite for the first monitor.
Record concrete build/test/conformance commands and results. Run available app tests and identify native SSH/Linux checks still required. Fake-source success does not prove Linux collection, native rendering, or safe OS actions.

##### Out of scope

No product features, infrastructure rewrite, dependency upgrades, or copying upstream implementation code.

##### Verification / acceptance criteria

The baseline records the exact revision, app entry point, dependency pins, reusable components, product location, and named gaps. Run available baseline tests and record failed/unavailable checks explicitly. Initial acceptance can reuse fresh evidence but cannot skip missing behavior. Ledger validation rejects duplicate IDs, missing upstream references, and verified entries without evidence; deferred families retain owners and planned status.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-000.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-001 — Display an empty Process Explorer window over existing SRUI

**Phase:** A · **Scope size:** S · **Status:** planned · **Gate:** none
**Dependencies:** PX-000
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21

##### Build

Register the new app through the existing server runtime and SDK. Render only Surface > Column > heading, a read-only status label, and an empty Table with PID and Name columns. Use a fixed development SSH target or the existing launcher; keep connection UX out of this ticket.
Add a deterministic smoke fixture and an app-specific test entry point. Preserve the same unmodified generic macOS client used by other SRUI examples.
Use the example's runtime/SDK integration as the reference, with the product under apps/srtop. Do not expose its unconfirmed Kill Selected workflow in this read-only release.

##### Out of scope

No process-changing actions, advanced diagnostics, custom graphs, new transport, or changes to unrelated examples.

##### Verification / acceptance criteria

A real SSH launch displays the title and zero-row table. No Terminal node or process-changing action exists. Changing a title in a fixture updates the existing native handle rather than reconstructing the window.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-001.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-002 — Populate the table with a deterministic fake process source

**Phase:** A · **Scope size:** S · **Status:** planned · **Gate:** none
**Dependencies:** PX-001
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21

##### Build

Introduce a minimal ProcessSource boundary and a FakeProcessSource returning three fixed process records. Add typed snapshot time, source identity, and explicit missing-value representation; do not build a universal monitoring framework.
Assign opaque session-scoped item IDs and render records through a collection model, not child view nodes. Separate the process record from its UI projection so real collection can replace the fake source next.

##### Out of scope

No process-changing actions, advanced diagnostics, custom graphs, new transport, or changes to unrelated examples.

##### Verification / acceptance criteria

The fixture displays exactly three rows in deterministic order. Two records with the same display name remain distinct. Missing data is not formatted as numeric zero. No live OS enumeration is invoked in fake mode.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-002.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-003 — Read one real Linux process snapshot with instance identity

**Phase:** A · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-002
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources K1, S1

##### Build

Implement one-shot enumeration of PID and process name using the already pinned collector where suitable. Keep the source injectable. Introduce ProcessKey: source/host identity, boot identity, PID-namespace identity where relevant, PID, and the highest-resolution available process creation token. Do not use rounded start-time seconds as the sole discriminator.
Resolve each ProcessKey to an opaque ItemId without reusing it within the semantic session. Represent incomplete enumeration separately from an authoritative empty result. Individual inaccessible records must not fail the entire snapshot. No process controls.

##### Out of scope

No process-changing actions, advanced diagnostics, custom graphs, new transport, or changes to unrelated examples.

##### Verification / acceptance criteria

Read a real Linux snapshot and locate a test-owned sleeping worker. In fixtures, reused PID with a different creation token receives a new ItemId. Verify process names containing control characters, invalid UTF-8, whitespace, and parentheses cannot corrupt parsing or render executable content.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-003.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-004 — Refresh the process list with incremental transactions

**Phase:** A · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-003
**Read:** D1 §§8, 12, 20.4; D2 T21, T25, T28

##### Build

Poll at a configurable, initially fixed one-second interval. Diff completed snapshots and batch inserts, deletes, and changed fields into the existing SDK transaction path. Never rebuild the semantic node tree per tick. Update model item_count and ordering according to the existing model contract.
A failed/incomplete scan must not turn every previously visible process into a deletion. Only serialize UI-visible changes; do not include an always-changing hidden field merely to force traffic.

##### Out of scope

No process-changing actions, advanced diagnostics, custom graphs, new transport, or changes to unrelated examples.

##### Verification / acceptance criteria

A fake sequence exercises one insertion, one deletion, and one rename; unchanged rows retain ItemIds and native controls. A repeated identical snapshot causes no app UI transaction. A failed scan retains last-known data with an error; recovery converges. Real creation/exit of a test worker appears within two sampling intervals.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-004.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-005 — Add resident memory with accurate units and unavailable states

**Phase:** A · **Scope size:** S · **Status:** planned · **Gate:** none
**Dependencies:** PX-004
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources S1, K1

##### Build

Add an RSS column in bytes internally and IEC units for display. State the source and interpretation in a metric definition. Sorting is not part of this ticket. Avoid adding every available memory field.
Use the availability states defined in the plan. Prevent loss of precision from storing byte counts in floating-point display strings.

##### Out of scope

No process-changing actions, advanced diagnostics, custom graphs, new transport, or changes to unrelated examples.

##### Verification / acceptance criteria

Unit tests cover zero, missing, very large values, and boundaries between KiB/MiB/GiB. A test-owned bounded allocator changes its RSS within a documented tolerance; do not demand exact byte equality from live accounting.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-005.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-006 — Add sampled per-process CPU usage

**Phase:** A · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-005
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources S1

##### Build

Retain counters and monotonic sampling intervals to compute process CPU usage. Choose a default convention where one fully used logical CPU is 100%, and label it; a multithreaded process may exceed 100%. Keep normalized-host percentage as a later display option.
Treat the first measurement, counter resets, process replacement, and zero/negative elapsed intervals as warm-up/unavailable rather than spikes. Reuse a persistent collector instance if its API requires it.

##### Out of scope

No process-changing actions, advanced diagnostics, custom graphs, new transport, or changes to unrelated examples.

##### Verification / acceptance criteria

Deterministic counters yield expected single-CPU and multicore percentages independent of wall-clock jumps. First sample is visibly warming up. CPU burn tests use a bounded worker and tolerance, not exact scheduler-dependent numbers.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-006.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-007 — Add the first system summary and data-freshness indicator

**Phase:** A · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-006
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources S1, K1

##### Build

Add overall CPU, memory used/total, swap used/total, uptime, load averages when available, and process count using Text and Progress. All percentages name their denominator and remain distinct from per-process CPU convention.
Display the last successful sample time and source. A local age display may derive elapsed time from a standard timestamp only through an already available generic renderer behavior; otherwise show last-received timestamp without adding process-specific client logic. Distinguish collector errors from transport loss.

##### Out of scope

No process-changing actions, advanced diagnostics, custom graphs, new transport, or changes to unrelated examples.

##### Verification / acceptance criteria

Fixture totals and percentages are consistent, including a host with no swap. The UI remains usable after a collector failure. Process count reflects the declared source/filter scope. No raw pixel instructions or custom graph dependency appears.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-007.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-008 — Release gate: the minimal read-only monitor

**Phase:** A · **Scope size:** S · **Status:** planned · **Gate:** R0
**Dependencies:** PX-007
**Read:** D1 §§4, 12.2, 31; D2 T21, T33–T34

##### Build

Provide a repeatable launch script and a short README: prerequisites, fake mode, real Linux host, known limitations, and how to stop the app. Capture a small operation/byte trace and a screenshot only as visual evidence, not semantic conformance.
Run an integration scenario from empty window through worker creation, sampling, and exit. Tag a read-only milestone only when the test gate passes.

##### Out of scope

No process-changing actions, advanced diagnostics, custom graphs, new transport, or changes to unrelated examples.

##### Verification / acceptance criteria

The same generic client runs the counter and the explorer. All A tickets pass. A frozen fake source produces zero app UI mutations after synchronization, excluding control traffic. Verify no code path can signal or reconfigure a process. Report commands and test results in a durable release record.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-008.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

### Phase B — Make the read-only monitor useful

#### PX-009 — Sort by a selected column without losing process identity

**Phase:** B · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-008
**Read:** D1 §§6.2, 7.6, 8; D2 T28

##### Build

Implement server-authoritative sorting by PID, name, CPU, or RSS with ascending/descending order, typed comparisons, missing-last behavior, and a deterministic ProcessKey tie-breaker. Use a standard semantic control; a simple button row is acceptable before a generic sort-header event exists.
Update the existing collection ordering through documented model operations. Retain ItemIds when order changes; never delete and recreate the logical processes simply to sort.

##### Out of scope

No process-changing actions, local application-policy engine, external automation IPC, or unrequested advanced metrics.

##### Verification / acceptance criteria

Fixtures cover ties, missing values, numeric versus lexical ordering, and changing CPU rankings. Selection remains on the same item after reordering. Stale query results cannot overwrite a newer sort request.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-009.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-010 — Add a responsive command filter

**Phase:** B · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-009
**Read:** D1 §§7.6, 8, 18.2–18.3, 22.6; D2 T24, T29

##### Build

Add a native TextInput that filters by process name and available command text on the server. Start with bounded case-insensitive fixed-string matching; support separate OR terms only through a documented, bounded grammar. Reuse T29 for editing and coalesce before event allocation, never by discarding allocated unacknowledged events.
Tag query changes with monotonic generations at the app layer and show pending versus committed results. Do not claim that server-filtered results arrive without network delay.

##### Out of scope

No process-changing actions, local application-policy engine, external automation IPC, or unrequested advanced metrics.

##### Verification / acceptance criteria

Typing, caret, and IME composition remain immediate under injected RTT. Older query results cannot replace the newest result set. Empty, long, Unicode, and no-match queries work; no regex backtracking or shell execution is possible.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-010.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-010-G01 — Gate early live collection scale, progress, and resource budgets

**Phase:** B · **Scope size:** M · **Status:** planned · **Gate:** R1-prerequisite
**Dependencies:** PX-010, PX-042
**Read:** D1 §§8, 12, 20.4, 22.7, 26; D2 T21, T28

##### Build

Build an app-to-runtime/native-client harness before further feature expansion. Exercise 1,000 and 10,000 synthetic rows with CPU sort, filtering, insert/delete churn, stable selection, and overlapping delayed range requests. Probe 100,000 rows to locate limits without claiming support. Test 0/100/300/600 ms RTT, documented constrained bandwidth, and faster sampling where configurable.
Verify model_range.rs behavior when authoritative revisions change during provider work. Demonstrate bounded progress under continuing telemetry, using query/source generation and consistent snapshots. If generic semantics need changes, append reviewed prerequisites and keep this gate open.
Set finite caps for collector working sets, identity/tombstone retention, pending ranges, client caches, and outgoing traffic. Never drop committed MODEL_UPDATE/structural operations or apply scalar coalescing to them. Measure source throttling/detach behavior, hardware, fields, row counts, sampling, payload/transport bytes, memory, and input-to-result latency.

##### Out of scope

No 100,000-row production claim, full collector coverage, or replacement transport.

##### Verification / acceptance criteria

Record pass/fail thresholds before measuring the supported R1 envelope: local input latency, range progress deadline, memory/queue caps, and recovery time. Within that envelope, correct rows arrive before the deadline under continuous churn, selection stays bound to ItemIds, and queues remain bounded. Outside it, behavior is explicit and bounded. Require native macOS evidence and deterministic tests. Append blockers instead of reducing thresholds after failure.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-010-G01.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-011 — Add find-next/find-previous and jump to PID

**Phase:** B · **Scope size:** S · **Status:** planned · **Gate:** none
**Dependencies:** PX-010-G01
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21

##### Build

Keep search distinct from filtering: search locates matches without hiding nonmatching rows. Provide next/previous navigation and a numeric PID jump that resolves to the currently observed process instance.
Specify behavior when matches change between samples or a PID is no longer visible. Reuse the query-generation rules; do not search only cached rows while presenting results as a whole-model search.

##### Out of scope

No process-changing actions, local application-policy engine, external automation IPC, or unrequested advanced metrics.

##### Verification / acceptance criteria

Three identical names cycle predictably forward/backward without changing row count. No match has a visible explanation. A PID that was reused selects the new live record only as an explicit new search, never retargets an old selection automatically.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-011.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-012 — Filter by owner and an explicit PID set

**Phase:** B · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-011
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21

##### Build

Add current-user/all-visible-user filtering plus selecting a specific numeric UID or resolved name. Add an optional PID-set filter for parity workflows. UID is authoritative; account-name resolution is bounded, cached, and never blocks sampling indefinitely.
Define how user, PID-set, text filter, and search combine. All-visible means visible to the daemon's OS account, not a promise to bypass host permissions.

##### Out of scope

No process-changing actions, local application-policy engine, external automation IPC, or unrequested advanced metrics.

##### Verification / acceptance criteria

Fixtures include an unresolved UID, duplicate display names, empty results, and a PID-set containing absent PIDs. Changing filters does not create new process identities. A delayed user-name lookup cannot stall the stream.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-012.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-013 — Add a selected-process inspector

**Phase:** B · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-012
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21

##### Build

On semantic selection, populate a compact detail region with name, PID, owner, state, parent, start time, and current CPU/RSS. Resolve the item on the server; treat selection input as untrusted. Bind asynchronous details to both ProcessKey and selection generation.
Keep native selection highlight local; details can load asynchronously. Distinguish no selection, loading, permission denied, and vanished process.

##### Out of scope

No process-changing actions, local application-policy engine, external automation IPC, or unrequested advanced metrics.

##### Verification / acceptance criteria

Select A then B with A's details artificially delayed; A never overwrites B. A row moving under sorting keeps the inspector attached to its identity. An unknown ItemId is rejected without a collector call for an arbitrary PID.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-013.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-014 — Show full command line and executable context

**Phase:** B · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-013
**Read:** D1 §§7, 22.6, 26–27; D2 T29, T32; sources S1, K1

##### Build

Add a selectable wrapped command-line view and executable/path information where available. Preserve the distinction between argv entries and a shell command; any pretty-printed command is display-only. Bound lengths, make truncation explicit, and scrub control characters for display without silently altering identity data.
Collect sensitive full arguments lazily on selection. Do not persist or log them by default. Remote paths are text, never automatic local file actions.

##### Out of scope

No process-changing actions, local application-policy engine, external automation IPC, or unrequested advanced metrics.

##### Verification / acceptance criteria

Arguments containing quotes, newlines, escape bytes, and shell metacharacters render as inert text. Clipboard selection uses normal native text behavior. Source permission failure and truncation are explicit. Logs do not contain secret-bearing fixture arguments.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-014.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-015 — Add sampling interval and a clearly labeled paused view

**Phase:** B · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-014
**Read:** D1 §§12, 18, 20.4; D2 T25

##### Build

Expose a bounded refresh interval with validation and a Pause view/Resume view action. Pause the app's published sample view at a defined server revision rather than interrupting Core transaction application or the transport. State whether collection continues; default to freezing app publication while maintaining session/control traffic.
Disable process-changing actions on frozen historical data. Resuming publishes a current consistent view. Manual refresh performs one bounded sample, not a new display frame protocol.

##### Out of scope

No process-changing actions, local application-policy engine, external automation IPC, or unrequested advanced metrics.

##### Verification / acceptance criteria

Pause preserves the shown rows while real workers change. Connection/control state still updates. Resume converges without replaying every missed sample. Invalid intervals are rejected and every timer is cancelled on app/session shutdown.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-015.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-016 — Make the basic workflow keyboard accessible

**Phase:** B · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-015
**Read:** D1 §§7.6, 22.5–22.9; D2 T16, T33, T35

##### Build

Add navigation and discoverable shortcuts for search, filter, refresh, pause, inspect, and sort using an existing generic action/shortcut mechanism. Native table movement, page navigation, horizontal scrolling, focus, and text selection remain client-local.
If the generic shortcut contract is missing, submit one bounded platform-neutral prerequisite with fixtures instead of a process-monitor switch statement in Swift. Do not capture letter shortcuts while a text editor or IME owns input.

##### Out of scope

No process-changing actions, local application-policy engine, external automation IPC, or unrequested advanced metrics.

##### Verification / acceptance criteria

Complete find/select/inspect/filter/reset without a mouse. Key behavior respects focus, IME, disabled controls, and OS-reserved shortcuts. Check accessibility labels, enabled state, and reading order using the actual AppKit client.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-016.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-017 — Add follow mode and stable viewport behavior

**Phase:** B · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-016
**Read:** D1 §§6.2–6.3, 8, 22.7; D2 T28

##### Build

Make Follow selected process an explicit semantic setting; keep ordinary selection preservation independent from viewport-following. Define follow cancellation, sticky follow, and pan-without-changing-selection behavior.
Use a generic collection presentation hook if needed, with no process-specific renderer branch. Handle filtering-out, disappearance, and cross-view navigation honestly.

##### Out of scope

No process-changing actions, local application-policy engine, external automation IPC, or unrequested advanced metrics.

##### Verification / acceptance criteria

CPU resorting keeps the followed identity visible without creating rows. Manual pan behavior matches the chosen mode. A disappeared process never transfers follow to another process with the same PID. Non-follow mode does not jump the user's viewport on every sample.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-017.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-018 — Add parent/child process tree navigation

**Phase:** B · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-017
**Read:** D1 §§6.2, 8, 22.7; D2 T28

##### Build

Project the same process identities into a Tree model. Resolve parents from the current sampled generation and validate creation ordering; an uncertain or inaccessible parent becomes an explicit orphan/root, not a guessed relationship. Provide collapse/expand and sibling sorting.
Preserve selection when switching table/tree. Keep hierarchy generation off the client and avoid one semantic node per row.

##### Out of scope

No process-changing actions, local application-policy engine, external automation IPC, or unrequested advanced metrics.

##### Verification / acceptance criteria

Fixtures cover missing parents, reparenting, PID reuse, malformed cycles, and deep trees under configured limits. Tree disclosure is local and missing ranges load asynchronously. Sorting siblings does not flatten the hierarchy.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-018.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-019 — Expose user threads and kernel-thread visibility

**Phase:** B · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-018
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources H3, S1

##### Build

Add thread-aware source records and an explicit process/thread distinction. A thread identity includes its process incarnation and an OS thread creation token where available. Add independent user-thread/kernel-thread visibility controls and thread counts.
Document whether rows show per-thread or aggregate process counters. Never sum the parent aggregate and its child thread totals as separate host usage.

##### Out of scope

No process-changing actions, local application-policy engine, external automation IPC, or unrequested advanced metrics.

##### Verification / acceptance criteria

A bounded multithreaded worker yields expected parent and thread rows. Process totals are not double counted. Thread exit/reuse gets new identity. Hosts without reliable thread metadata show an explicit capability limitation.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-019.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-020 — Add stable multi-selection and descendant tagging

**Phase:** B · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-019
**Read:** D1 §§6.2, 7.6, 8; D2 T28

##### Build

Implement explicit tags independent of the current keyboard-highlighted row. Support tag/untag, clear all, and tag current sampled descendants. Store tags by ProcessKey; freeze descendant membership at the time of the request.
Show the number of tagged items hidden by filters. No bulk side effects yet. Do not confuse a sampled descendant set with an OS process group or a promise about future children.

##### Out of scope

No process-changing actions, local application-policy engine, external automation IPC, or unrequested advanced metrics.

##### Verification / acceptance criteria

Tags survive sorting and filtering and are removed or marked historical on exit. Newly created descendants are not silently added. PID reuse never inherits a tag. Clearing tags does not affect current selection.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-020.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-021 — Keep a last-observed record when a process disappears

**Phase:** B · **Scope size:** S · **Status:** planned · **Gate:** none
**Dependencies:** PX-020
**Read:** D1 §§6.2, 20.4, 26; D2 T21

##### Build

Retain a bounded tombstone for the selected or explicitly watched process: instance identity, last sample, last CPU/RSS, and last-observed time. Mark historical state unmistakably and disable all process actions.
Say no longer observed when sampling cannot establish exit; only say exited when positively established. Do not invent exit code, causality, or unsampled lifetime history. Bound retention by count, age, and bytes.

##### Out of scope

No process-changing actions, local application-policy engine, external automation IPC, or unrequested advanced metrics.

##### Verification / acceptance criteria

An exiting worker leaves an inspectable tombstone. An inaccessible scan does not falsely record every process as exited. A PID replacement has a distinct live item. Retention expires deterministically with a fake clock.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-021.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

### Phase C — Prove the SRUI advantages

#### PX-022 — Expose disconnect, catch-up, and replacement correctly

**Phase:** C · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-021
**Read:** D1 §§17–18.3; D2 T22–T24, T29

##### Build

Wire the app's displayed state to existing session continuity outcomes without adding a second reconnect implementation. A detached window labels data stale, allows safe cached inspection, and prevents new process actions. Same-session replay preserves compatible process identities; replacement clears pending intent and invalidates old selections/tombstones unless explicitly retained as historical records.
Make transport reconnect status trusted client chrome, not a remote claim that the socket is alive. Full-resync text handling follows T29 exactly.

##### Out of scope

No new retry protocol, secret/key storage, simultaneous fleet administration, or external automation listener.

##### Verification / acceptance criteria

Exercise replay within retention, SAME_SESSION snapshot outside retention, and REPLACED after daemon restart. Only newest resume generation affects the window. Pending edits follow the specified resume/resync behavior. No stale result is displayed as newly sampled data.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-022.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-023 — Add a minimal generic host connection form

**Phase:** C · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-022
**Read:** D1 §§19, 22.1, 25; D2 T19–T20, T38

##### Build

Implement the first bounded slice of T38: host, user, optional port, and Connect in app-level client chrome. Reuse SSHTransport, known_hosts, and existing credential handling. Do not add connection forms to the remote app or allow it to select arbitrary local destinations.
Keep host-key failures blocking and diagnostics understandable. Support one connection window initially; do not build a fleet manager.

##### Out of scope

No new retry protocol, secret/key storage, simultaneous fleet administration, or external automation listener.

##### Verification / acceptance criteria

Fresh known-host connection works. Malformed endpoint input is rejected without shell interpolation. Changed/unknown host key follows the existing explicit trust policy and never silently connects. Counter and explorer can use the same launcher.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-023.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-024 — Save endpoints and safely reattach after client relaunch

**Phase:** C · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-023
**Read:** D1 §§6.3, 17–18.2; D2 T23–T24, T38

##### Build

Persist endpoint labels locally and reconnect via existing session APIs. Store only continuity bookkeeping that can actually be restored consistently. A remembered revision without a matching committed replica must not be used as though the replica exists: use the documented snapshot/attach recovery path. Likewise do not reuse a client event sequence identity without its required outbox/frontier state; choose a fresh client identity when appropriate.
Treat saved connection deletion as local forgetting. Surface replaced sessions and update stale session IDs. Do not implement partial durable client recovery disguised as seamless resume.

##### Out of scope

No new retry protocol, secret/key storage, simultaneous fleet administration, or external automation listener.

##### Verification / acceptance criteria

Quit/relaunch after updates and verify the restored UI contains complete current state, not an empty replica receiving only later deltas. Lost local outbox state cannot generate sequence collisions. Double-connect respects generation supersession. Forgetting an entry leaves the remote session alive.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-024.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-025 — Add bounded demonstration workloads and latency scenarios

**Phase:** C · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-024
**Read:** D1 §31; D2 T28–T29, T34

##### Build

Provide an opt-in demo worker binary with bounded CPU, memory, thread, and short-lived-process modes. Identify workers as test-owned; keep cleanup handles and hard resource/time limits. Never use production processes as test targets.
Compose repeatable 0/100/300/600 ms RTT, transport interruption, and collector failure scenarios. Reuse T34 instrumentation. Distinguish local interaction latency from remote result latency.

##### Out of scope

No new retry protocol, secret/key storage, simultaneous fleet administration, or external automation listener.

##### Verification / acceptance criteria

Workers terminate/clean up after success, failure, and harness interruption. Typing/caret/scrolling of cached ranges gain no RTT wait. Server-dependent search results are delayed as expected. Wire bytes for the same fake state stream are independent of renderer cadence.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-025.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-026 — Demonstrate semantic inspection without external IPC

**Phase:** C · **Scope size:** S · **Status:** planned · **Gate:** none
**Dependencies:** PX-025
**Read:** D1 §22.9; D2 T24, T35

##### Build

Use T35's in-process API to locate controls by semantic identity/role/label and invoke a harmless action through EventOutbox. Include a disabled-control test and a selection-plus-inspection scenario.
Add a developer-only inspection view or test output using semantic IDs, values, and advertised actions. Do not expose NSView types or a new XPC/TCP listener.

##### Out of scope

No new retry protocol, secret/key storage, simultaneous fleet administration, or external automation listener.

##### Verification / acceptance criteria

The automation-generated action follows the same wire/authorization path as a real click. A disabled action fails identically. Tests do not use screen coordinates or process-specific client shortcuts. No new external automation endpoint is reachable.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-026.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-026-G01 — Gate installable R1 with compatibility, privacy, and native verification

**Phase:** C · **Scope size:** M · **Status:** planned · **Gate:** R1-prerequisite
**Dependencies:** PX-026, PX-068, PX-010-G01
**Read:** D1 §§18, 20, 25–27, 29, 32; D2 T23–T24, T32–T34

##### Build

Produce reproducible Linux app and macOS client build/install instructions or artifacts using existing SSH and same-user sockets. Pin tested OS/kernel/client/server versions, dependency locks, signing requirements, and prerequisites. Check compatible/incompatible protocol profiles, unknown host keys, authentication failure, reconnect, and server replacement.
Package with process-changing actions unavailable. Audit command-line/path collection, logs, saved endpoints, and diagnostic bundles for disclosure. Keep credentials and unnecessary sensitive payloads out. Verify keyboard navigation and accessibility in the native client.
Document user-owned installation, update/rollback, and uninstall without removing unrelated data. Include a clean-account walkthrough and bounded demo workers. Publish PX-010-G01 measurements as the support envelope.

##### Out of scope

No full htop parity, privileged installer/helper, external automation, or advanced diagnostics.

##### Verification / acceptance criteria

On named Linux/macOS environments, install/connect/use/update-or-rollback/uninstall succeeds with recorded commands and native evidence. Unknown host keys are not silently accepted; incompatible versions fail clearly. Artifacts/settings contain no secrets. R1 notes state scope and tested performance envelope. Missing required Linux/native evidence blocks R1.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-026-G01.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-027 — Release gate: the first polished SRUI showcase

**Phase:** C · **Scope size:** S · **Status:** planned · **Gate:** R1
**Dependencies:** PX-026, PX-026-G01, PX-068
**Read:** D1 §§31–32; D2 T33–T35; sources H1

##### Build

Bundle the read-only monitor, navigation, inspector, connection form, and reproducible demonstration. Publish measured traffic and latency results with workload, hardware, build, and sampling settings; separate semantic payload bytes from encrypted transport overhead.
Compare against htop only using matched data and sampling workloads. Do not claim automatic bandwidth superiority over an ncurses application. Document all remaining functionality and environmental gaps.
R1 is an installable Linux-host/macOS-client release. Require PX-010-G01 scale/budget evidence and PX-026-G01 installation, compatibility, privacy, and accessibility evidence. Publish the tested process-count/refresh/RTT/bandwidth envelope.

##### Out of scope

No new retry protocol, secret/key storage, simultaneous fleet administration, or external automation listener.

##### Verification / acceptance criteria

Run the end-to-end demo from a clean account with a configured server. Verify reconnect, keyboard use, accessibility, bounded workers, and zero app mutations for an unchanged fake source. Release notes say read-only showcase, not htop parity.
Both suffixed gates must pass. High-churn delayed range requests must make bounded progress, not merely avoid wrong-row data.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-027.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

### Phase D — Add safe process controls

#### PX-028 — Define action authorization and a dry-run action contract

**Phase:** D · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-027
**Read:** D1 §§7.7, 18.2, 27; D2 T24, T32

##### Build

Add a server-only ProcessActions interface and an ActionIntent record containing the exact ProcessKeys, operation, parameters, authenticated account/context, creation time, expiry, and immutable intent ID. Keep read-only mode enforced on the server, not merely disabled buttons.
Define typed outcomes: refused, unsupported, requested, observed complete, failed, and outcome unknown. Validate intent-specific freshness; ordinary telemetry revision changes alone must not invalidate a still-valid confirmation. Implement only a dry-run executor initially.

##### Out of scope

No automatic remediation, arbitrary shell commands, root session daemon, privileged helper, or changes to other action families.

##### Verification / acceptance criteria

Unknown, historical, unowned/disallowed, and replaced-incarnation targets cannot reach the executor. Forged client parameters cannot widen the target set. Tests cover disabled controls and stale intents through real event dispatch, not only a helper function.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-028.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-029 — Resolve stable Linux process handles before enabling actions

**Phase:** D · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-028
**Read:** D1 §§6.2, 27; D2 T32; sources K1–K3

##### Build

Implement an identity-checked, bounded-lifetime target handle for signaling. On supported Linux use pidfd-based signaling; verify that the acquired handle represents the sampled ProcessKey, not a PID replacement encountered between enumeration and acquisition. Close handles on cancellation, expiry, and settlement.
Document the minimum supported kernel/API path and behavior when unavailable. A held pidfd does not magically make every separate PID-based syscall race-free; non-signal operations need their own reviewed targeting strategy. Keep unsupported actions disabled rather than silently weakening identity guarantees.

##### Out of scope

No automatic remediation, arbitrary shell commands, root session daemon, privileged helper, or changes to other action families.

##### Verification / acceptance criteria

Inject exit/PID reuse before handle acquisition and after confirmation preparation; no replacement receives a signal. Test permission denial, unsupported API, descriptor exhaustion, and descriptor cleanup. Live tests use only handles for disposable owned workers.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-029.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-030 — Add confirmed graceful termination of one process

**Phase:** D · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-029
**Read:** D1 §§7.7, 18.2, 27; D2 T24, T32; sources K2

##### Build

Add Terminate… which prepares an immutable intent and an inline confirmation panel built from existing widgets. Create a fresh confirmation action node/handler bound to that intent; do not reuse a mutable generic confirm handler whose target changes with selection. Confirm names the exact process and sends SIGTERM through ProcessActions.
Cancel, expiry, navigation, readonly mode, and identity replacement invalidate the intent. Refuse PID 1 in the relevant namespace, the app/daemon/transport support chain, and any configured protected targets. Do not depend on this denylist as the sole authorization boundary.

##### Out of scope

No automatic remediation, arbitrary shell commands, root session daemon, privileged helper, or changes to other action families.

##### Verification / acceptance criteria

Prepare termination for A, select B, and confirm: only the explicitly confirmed valid target A can be acted on, or the old intent is rejected. Cancel and expired intent have no effects. A test-owned worker handles SIGTERM and exits. Repeat delivery of the same event cannot send a new independent action.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-030.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-031 — Distinguish action acknowledgement from observed process outcome

**Phase:** D · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-030
**Read:** D1 §18.2 and Appendix B; D2 T24; sources S1

##### Build

Publish the OS request result and later observed state separately: signal requested is not equivalent to process exited. Track pending observation with a deadline; after timeout display still running or outcome unknown rather than silently escalating.
Integrate the existing event result cache without introducing a second transport ACK protocol. Treat a handler's domain failure as a reported action result; use protocol rejection only for its defined validation conditions. Record intent and settlement without logging sensitive arguments.

##### Out of scope

No automatic remediation, arbitrary shell commands, root session daemon, privileged helper, or changes to other action families.

##### Verification / acceptance criteria

A worker that ignores SIGTERM shows a successful request but remains running. A worker exiting naturally is not falsely attributed to the action. Break transport after effect but before ACK: surviving-session replay uses the settled result without re-dispatch. Restart the daemon: old pending intent is abandoned, not retried against a replacement.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-031.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-032 — Add explicit Stop and Continue actions

**Phase:** D · **Scope size:** S · **Status:** planned · **Gate:** none
**Dependencies:** PX-031
**Read:** D1 §§18.2, 27; D2 T24, T32

##### Build

Add separate confirmed process suspension and continuation operations through the same intent/handle pipeline. Label these distinctly from Pause view. Reflect observed stopped/running state asynchronously.
Protect the daemon, bridge, app, and their support processes. Provide bounded test cleanup that resumes or terminates only test-owned stopped workers. Do not freeze arbitrary process groups as a shortcut.

##### Out of scope

No automatic remediation, arbitrary shell commands, root session daemon, privileged helper, or changes to other action families.

##### Verification / acceptance criteria

A disposable worker stops making progress and later resumes. Pause view never sends a stop signal. A lost connection cannot strand test cleanup indefinitely. Permission failures and exited processes produce typed results.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-032.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-033 — Add explicit force termination without automatic escalation

**Phase:** D · **Scope size:** S · **Status:** planned · **Gate:** none
**Dependencies:** PX-032
**Read:** D1 §§7.7, 18.2, 27; D2 T24, T32

##### Build

Expose Force terminate… as a separate destructive intent using SIGKILL for supported targets. Explain that it is not graceful and cannot be undone. Never schedule it automatically when graceful termination is slow; require a new explicit user confirmation.
Reuse exact-target binding, protection policy, identity verification, and observation reporting. Do not add an Undo button for irreversible actions.

##### Out of scope

No automatic remediation, arbitrary shell commands, root session daemon, privileged helper, or changes to other action families.

##### Verification / acceptance criteria

Only an explicitly confirmed disposable worker is force-terminated. SIGTERM timeout never triggers SIGKILL. Replayed settled confirmation does not execute again. Historical/expired/protected targets fail closed.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-033.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-034 — Add an advanced signal chooser

**Phase:** D · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-033
**Read:** D1 §§6.4, 7.7, 27; D2 T24, T32; sources H4

##### Build

Expose supported named signals from the remote OS through a typed, server-validated selection model. Describe process versus thread scope and keep risky signals behind advanced mode. Do not assume macOS client signal numbers match the Linux host.
Validate against the advertised operation set and OS permissions. Signal 0, where offered, is a separate probe action, not a misleading termination choice. Do not automatically include real-time signal payload features not in the baseline.

##### Out of scope

No automatic remediation, arbitrary shell commands, root session daemon, privileged helper, or changes to other action families.

##### Verification / acceptance criteria

Invalid numbers/names cannot reach the OS. Fixtures use different remote signal mappings to prove client neutrality. Cancellation and stale chooser state produce no effect. Every advertised signal has a documented test or an explicit restricted test-environment status.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-034.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-035 — Apply actions to a frozen tagged target set

**Phase:** D · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-034
**Read:** D1 §§7.7, 12, 18.2, 27; D2 T24, T32

##### Build

Extend action preparation to a bounded explicit ProcessKey list from tags or sampled descendants. Show visible/hidden target counts and the full confirmable target set. Revalidate each target at execution; newly appearing children are not included.
Execute with bounded concurrency and individual results. A semantic UI transaction is not an OS transaction: partial success is possible and must be shown. Never silently retry succeeded targets or promise rollback.

##### Out of scope

No automatic remediation, arbitrary shell commands, root session daemon, privileged helper, or changes to other action families.

##### Verification / acceptance criteria

A batch with permitted, protected, disappeared, and denied targets reports each outcome. PID reuse and newly spawned descendants do not expand the batch. Disconnect/duplicate delivery cannot repeat completed members. Cancellation semantics distinguish unstarted from already executed members.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-035.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-035-G01 — Resolve identity-safe targeting feasibility for advanced controls

**Phase:** D · **Scope size:** M · **Status:** planned · **Gate:** Advanced-controls-prerequisite
**Dependencies:** PX-029, PX-031
**Read:** D1 §§6.2, 18.2, 27; D2 T32; sources K1–K5

##### Build

For nice, affinity, I/O priority, scheduler policy, and autogroup, document kernel APIs, target scope, identity tokens/handles, permission requirements, race windows, cancellation, and minimum versions. Prototype against disposable owned workers. A held pidfd or PID/start-time precheck is not universal protection for later PID-based calls.
Obtain recorded technical review per strategy. Disable blocked operations and leave parity open rather than weakening identity guarantees. Give feasible implementations bounded follow-on scopes; unresolved research is not a routine medium-size implementation ticket.

##### Out of scope

No enabled process controls, privilege helper, or claim that fixtures alone prove races impossible.

##### Verification / acceptance criteria

Provide an operation-by-operation supported/blocked matrix with API references, adversarial exit/PID-reuse fixtures, live owned-worker evidence where feasible, reviewer decisions, and limitations. PX-031 ships independently. PX-036–PX-040 enable only reviewed operations; required missing operations keep PX-041/full parity open.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-035-G01.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-036 — Read and change nice values with operation-specific safety

**Phase:** D · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-035, PX-035-G01
**Read:** D1 §27; D2 T32; sources K5

##### Build

Add current nice value, validated target nice value, and a confirmed change action. Verify the exact platform syscall semantics and permission model at implementation time. Use a reviewed identity-safe targeting strategy for this PID-based operation; do not describe pidfd signaling protection as protection for setpriority.
Support permitted changes and display denied changes without requesting root for sessiond. Any residual race or restricted functionality is an explicit blocked parity item, not a hidden fallback.
Apply the operation-specific decision from PX-035-G01. A blocked strategy leaves the operation disabled and parity open; pidfd possession or a precheck alone is not proof.

##### Out of scope

No automatic remediation, arbitrary shell commands, root session daemon, privileged helper, or changes to other action families.

##### Verification / acceptance criteria

Fixtures cover out-of-range values, permission denial, thread/process scope, identity churn, and partial bulk failure. A disposable worker can have its priority decreased and observed. No test assumes an ordinary user can raise priority or restore a prior setting.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-036.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-037 — Inspect and edit CPU affinity

**Phase:** D · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-036
**Read:** D1 §§6.5, 27; D2 T32; sources K4

##### Build

Display remote logical CPU identifiers and permitted/online masks; allow a confirmed nonempty affinity mask where supported. Clearly state single-thread, selected-thread, or whole-process semantics and apply them consistently.
Use an operation-specific targeting strategy and account for hotplug and cpuset restrictions. Read back the effective mask; do not claim the requested mask was accepted unchanged. Do not add client hardware assumptions.
Apply the operation-specific decision from PX-035-G01. A blocked strategy leaves the operation disabled and parity open; pidfd possession or a precheck alone is not proof.

##### Out of scope

No automatic remediation, arbitrary shell commands, root session daemon, privileged helper, or changes to other action families.

##### Verification / acceptance criteria

Test sparse CPU IDs, more than 64 CPUs, empty masks, offline CPUs, cpuset restrictions, permission failures, and thread exit. A test-owned worker's effective affinity is independently verified. Invalid or stale target identity is rejected.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-037.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-038 — Inspect and edit I/O scheduling priority

**Phase:** D · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-037
**Read:** D1 §§18.2, 27; D2 T24, T32; sources H4

##### Build

Add I/O scheduling class/priority fields and an advanced confirmed edit workflow. Implement remote platform semantics behind ProcessActions; distinguish unsupported kernel/scheduler behavior from denied permissions or a successful request with no observable workload effect.
Do not infer disk throughput improvements from changing priority. Keep operation-specific identity safety and bulk results consistent with prior controls.
Apply the operation-specific decision from PX-035-G01. A blocked strategy leaves the operation disabled and parity open; pidfd possession or a precheck alone is not proof.

##### Out of scope

No automatic remediation, arbitrary shell commands, root session daemon, privileged helper, or changes to other action families.

##### Verification / acceptance criteria

Validate every supported class and priority boundary with fixtures. Unsupported classes are disabled and explained. A harmless owned target exercises the available syscall path; no disk saturation test is required. Replayed events preserve the original result.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-038.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-039 — Inspect and edit scheduler policy safely

**Phase:** D · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-038
**Read:** D1 §27; D2 T32; sources H7

##### Build

Add scheduler policy, applicable priority range, and reset-on-fork where the remote OS supports them. Require an explicit advanced workflow. Default real-time changes to denied unless an administrator-authorized environment and reviewed policy allow them.
Implement one policy action with strict enum/range validation and operation-specific target identity checks. Do not run an unbounded real-time worker or auto-adjust scheduling based on CPU use.
Apply the operation-specific decision from PX-035-G01. A blocked strategy leaves the operation disabled and parity open; pidfd possession or a precheck alone is not proof.

##### Out of scope

No automatic remediation, arbitrary shell commands, root session daemon, privileged helper, or changes to other action families.

##### Verification / acceptance criteria

Fixture tests cover every advertised policy, invalid priority combinations, denied real-time requests, and reset-on-fork preservation. Live elevated tests are opt-in in disposable environments with watchdog cleanup. Ordinary daemon privilege remains unchanged.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-039.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-040 — Expose autogroup identity and priority changes

**Phase:** D · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-039
**Read:** D1 §§7.7, 27; D2 T32; sources H3

##### Build

Add supported autogroup metadata and its separate priority action. Explain that the target is a scheduling group, not simply one process, and show the affected group before confirmation. Resolve group identity/freshness from the server rather than an arbitrary client path.
Do not confuse this feature with Unix process-group signaling or descendant tags. Unsupported platforms display the capability as unavailable.
Apply the operation-specific decision from PX-035-G01. A blocked strategy leaves the operation disabled and parity open; pidfd possession or a precheck alone is not proof.

##### Out of scope

No automatic remediation, arbitrary shell commands, root session daemon, privileged helper, or changes to other action families.

##### Verification / acceptance criteria

Test two processes sharing a group, vanished/replaced group metadata, range validation, and denied permissions. The action cannot write a client-supplied path. Confirmation displays group scope and the result is read back where possible.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-040.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-041 — Release gate: an everyday process explorer with safe controls

**Phase:** D · **Scope size:** M · **Status:** planned · **Gate:** R2
**Dependencies:** PX-040
**Read:** D1 §§18.2, 26–27; D2 T24, T32–T34

##### Build

Run all action-policy, stale-target, confirmation, duplicate-delivery, permission, and cleanup tests. Publish an action-support table for the target Linux kernels and privileges. Keep incomplete advanced actions disabled and identified as gaps; do not hide them behind a general claim of parity.
Verify the default experience still begins read-only or with a clearly explicit enable-actions policy, and that dangerous operations are never available on stale/historical views.

##### Out of scope

No automatic remediation, arbitrary shell commands, root session daemon, privileged helper, or changes to other action families.

##### Verification / acceptance criteria

The safe single-target workflow passes from real click to observed worker outcome. Same-session retry tests do not repeat a settled side effect. Deliberate daemon failure is not represented as guaranteed exactly-once execution. No root sessiond, arbitrary shell runner, or privilege escalation was introduced.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-041.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

### Phase E — Expand monitoring depth

#### PX-042 — Introduce a typed metric catalogue and collection-cost policy

**Phase:** E · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-008
**Read:** D1 §§6.5, 8, 20.4, 26; D2 T25, T28

##### Build

Promote existing fields into a server-side catalogue: stable key, units, raw type, source, aggregation semantics, default formatter/sort, capability, sensitivity, collection cost, and supported refresh tier. Preserve original field behavior.
Represent present, warming-up, permission-denied, unsupported, stale, and failed distinctly. Schedule cheap list fields every sample and expensive selected/visible fields only on demand. Retain bounded source timestamps so delayed expensive results cannot masquerade as a fresh atomic OS snapshot.
Execute immediately after R0. Catalogue existing CPU/RSS/name/PID fields first; later collectors extend this catalogue.

##### Out of scope

No host configuration changes, automatic privilege grants, all-metric polling, or implementation of unrelated metric families.

##### Verification / acceptance criteria

Catalogue rejects duplicate keys and incompatible units. Hidden expensive fields are not collected. Slow source work cannot block the ordinary sample/control loop. Newer field results are not overwritten by older completions.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-042.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-043 — Add process identity, timing, and scheduling columns

**Phase:** E · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-042
**Read:** D1 §§6.5, 8; D2 T28; sources H3, K1

##### Build

Add parent/group/session/terminal identities, real/effective user metadata where available, start/elapsed time, accumulated user/system/child CPU time, thread count, last CPU, state, and nice/priority via the metric catalogue. This is one cheap process-metadata collector family, not every advanced metric.
Document counter units and process/thread meanings. Keep host wall time for display separate from monotonic durations. Fields not produced reliably by the chosen API need bounded OS readers, not fabricated values.

##### Out of scope

No host configuration changes, automatic privilege grants, all-metric polling, or implementation of unrelated metric families.

##### Verification / acceptance criteria

Recorded snapshots cover units, long runtimes, clock jumps, missing terminal, UID resolution failure, and thread/process distinctions. Verify every new field's formatter and typed sort. Add source references and test IDs to the parity ledger.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-043.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-044 — Add faults and context-switch accounting

**Phase:** E · **Scope size:** S · **Status:** planned · **Gate:** none
**Dependencies:** PX-043
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources H3, K1

##### Build

Add minor/major faults, waited-child fault counts where meaningful, and voluntary/involuntary context switches. Expose counters and carefully labeled rates only where supported. Rate calculation resets on ProcessKey change or counter discontinuity.
Do not interpret these numbers as causal diagnosis. Use separate unknown and zero values and retain the measurement interval.

##### Out of scope

No host configuration changes, automatic privilege grants, all-metric polling, or implementation of unrelated metric families.

##### Verification / acceptance criteria

Counter deltas and child totals match fixtures. Reset, wrap/overflow boundaries, first sample, process replacement, and missing kernel fields do not create spikes. Disabled columns produce no unnecessary collection work.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-044.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-045 — Add detailed low-cost memory columns

**Phase:** E · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-044
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources H3, K1

##### Build

Add virtual, resident, shared/private estimate, code/data/stack/library fields, and swap where reliably available. Distinguish a cheap RSS-minus-shared estimate from precise private memory; do not label them as equivalent.
Use the catalogue to explain unsupported historical fields rather than filling them with zeros. Verify host page size at runtime; never hardcode 4 KiB.

##### Out of scope

No host configuration changes, automatic privilege grants, all-metric polling, or implementation of unrelated metric families.

##### Verification / acceptance criteria

Fixtures cover multiple page sizes, large processes, unavailable components, and estimates. Impossible negative derived values are handled explicitly. Every field has provenance, units, and sort tests.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-045.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-046 — Add on-demand proportional memory accounting

**Phase:** E · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-045
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources H3, K1

##### Build

Read proportional/private mapping information using a bounded on-demand source, preferring aggregate interfaces where supported. Expose PSS, proportional swap, effective PSS, and precise private memory as distinct definitions.
Default expensive collection to the selected process or explicitly enabled columns with an adaptive slow tier. Mark sample age and cancellation; do not scan every mapping of every process each tick.

##### Out of scope

No host configuration changes, automatic privilege grants, all-metric polling, or implementation of unrelated metric families.

##### Verification / acceptance criteria

Synthetic shared mappings yield expected accounting formulas. Permission denial, huge mapping lists, process exit, and slow reads cannot block UI/control. The expensive collector is cancelled/bounded and hidden fields do not trigger it.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-046.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-047 — Add per-process I/O counters and rates

**Phase:** E · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-046
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources H3, K1

##### Build

Add logical character counts, syscall counts, storage read/write counters, cancelled writes, and their supported rates. Keep logical I/O distinct from physical/storage accounting; do not relabel either as per-process network bandwidth.
Compute rates from monotonic intervals and process identity. Integrate with sortable columns and a small I/O-focused inspector region.

##### Out of scope

No host configuration changes, automatic privilege grants, all-metric polling, or implementation of unrelated metric families.

##### Verification / acceptance criteria

Fixtures separate logical and storage I/O and verify combined-rate definitions. First sample, permission loss, process replacement, and counter reset are explicit. A bounded temp-file worker exercises the live collector without stressing the host.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-047.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-048 — Expose cgroups, namespaces, and container visibility

**Phase:** E · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-047
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources H3, K1

##### Build

Add full and compact cgroup paths, available namespace identity, and a documented containerized-process classification. Provide a hide/show-containerized filter without relying solely on a command-name substring.
Keep host and container resource denominators distinct. Explain unknown classification and hierarchy/version differences. Do not open a Docker socket, enter namespaces, or control containers in this ticket.

##### Out of scope

No host configuration changes, automatic privilege grants, all-metric polling, or implementation of unrelated metric families.

##### Verification / acceptance criteria

Fixtures cover cgroup v1/v2, nested namespaces, escaped names, inaccessible metadata, and a monitor itself running inside a container. Unknown is not silently classified as host. Full path remains available when compact display is used.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-048.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-049 — Expose OOM, elevated privileges, and security context

**Phase:** E · **Scope size:** S · **Status:** planned · **Gate:** none
**Dependencies:** PX-048
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources H3, K1

##### Build

Add OOM-related values and available privilege/security metadata as read-only diagnostic fields. Display elevated state with semantic warning text/roles, not color alone. Keep unavailable/denied distinct from a benign value.
Do not implement changes to OOM policy, capabilities, or security labels. Avoid presenting incomplete metadata as a security verdict.

##### Out of scope

No host configuration changes, automatic privilege grants, all-metric polling, or implementation of unrelated metric families.

##### Verification / acceptance criteria

Test absent OOM values, differing real/effective identities, capability metadata, and restricted reads. Accessibility exposes warning meaning. No action path modifies the inspected policy.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-049.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-050 — Show command variants and replaced executable/library warnings

**Phase:** E · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-049
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources H3, H5, K1

##### Build

Expose separate comm, executable, and argv-derived display options plus a readable merged presentation. Detect available evidence that an executable or mapped library was deleted/replaced, preserving provenance and uncertainty.
Use bounded on-demand map inspection; distinguish executable warning from library warning. Do not follow remote paths into local files or automatically suggest a destructive restart action.

##### Out of scope

No host configuration changes, automatic privilege grants, all-metric polling, or implementation of unrelated metric families.

##### Verification / acceptance criteria

Fixtures cover missing exe links, long argv, deleted executables, replaced libraries, and conflicting names. Warning state is available as text for accessibility and sorting/filtering. No repeated full-map scan occurs while the feature is off.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-050.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-051 — Add optional delay-accounting metrics

**Phase:** E · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-050
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources H3, H4

##### Build

Implement an optional provider for CPU, block-I/O, and swap delay accounting supported by the host and build. Discover kernel/library/permission availability and report the exact reason for missing data. Keep it off by default until configured.
Use bounded requests and deltas; never change host sysctls, load kernel modules, or elevate sessiond automatically. Integrate the eventual admin helper only through its approved contract.

##### Out of scope

No host configuration changes, automatic privilege grants, all-metric polling, or implementation of unrelated metric families.

##### Verification / acceptance criteria

Fixtures cover unsupported kernel, disabled collection, permission denial, and counter reset. Timeouts cannot stall normal sampling. Opt-in VM tests validate supported metrics; lack of a test host is recorded as unverified, not passed.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-051.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-052 — Add per-CPU usage classes and topology

**Phase:** E · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-051
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources H4, K1

##### Build

Add per-logical-CPU measurements, aggregate class breakdown, and package/core/SMT labels where discoverable. Correctly handle overlapping guest counters when computing totals; record the formula. Represent offline/missing CPUs explicitly and accept sparse IDs.
Initially render the data through small tables and native progress indicators. Local screen refresh remains independent of sample cadence.

##### Out of scope

No host configuration changes, automatic privilege grants, all-metric polling, or implementation of unrelated metric families.

##### Verification / acceptance criteria

Fixtures cover guest/steal accounting, hotplug, noncontiguous IDs, mixed topology, and more than 256 CPUs. No double counting produces impossible aggregate percentages. CPU disappearance releases associated sample state.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-052.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-053 — Add CPU frequency and temperature readings

**Phase:** E · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-052
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources H4

##### Build

Implement optional frequency/temperature collection with clear distinctions between per-core, package, instantaneous, and estimated values. Discover sensor labels safely; do not assume every logical CPU has a corresponding sensor.
Use a slow tier and explicit unsupported/read-failure states. No fan control, tuning, or automatic hardware configuration.

##### Out of scope

No host configuration changes, automatic privilege grants, all-metric polling, or implementation of unrelated metric families.

##### Verification / acceptance criteria

Fixtures cover absent sensors, unusual labels, changing CPU sets, invalid values, and temperature units. A slow/missing optional library cannot degrade ordinary collection. Availability and source labels are visible.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-053.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-054 — Add detailed host memory, swap, and huge-page statistics

**Phase:** E · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-042, PX-007
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources H4, K1

##### Build

Add used/available/cache/buffer/shared categories, swap details, and supported huge-page totals. Define overlap and accounting rules so displayed segments do not imply invalid sums. Keep host versus container limits explicit.
Present a readable breakdown with standard controls. A machine with no swap or huge-page allocation is a valid state, not a collection failure.

##### Out of scope

No host configuration changes, automatic privilege grants, all-metric polling, or implementation of unrelated metric families.

##### Verification / acceptance criteria

Recorded hosts with distinct memory layouts satisfy documented invariants. Zero totals cannot divide by zero. Large counters, multiple page sizes, and missing optional fields are tested. Labels distinguish available from free memory.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-054.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-055 — Add host and per-device disk activity

**Phase:** E · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-042, PX-007
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources H4

##### Build

Collect bounded host/per-device storage counters and rates with stable device identity where available. Expose transfer and activity fields supported by the source; identify aggregate scope and avoid double counting stacked devices.
Use a virtualized table for large device sets. Device removal or counter reset starts a new rate baseline.

##### Out of scope

No host configuration changes, automatic privilege grants, all-metric polling, or implementation of unrelated metric families.

##### Verification / acceptance criteria

Fixtures include partitions/stacked devices, hotplug, zero interval, and reset counters. Aggregate rules are documented. Bounded temporary-file activity confirms live sampling; no benchmark writes raw devices.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-055.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-056 — Add host and per-interface network activity

**Phase:** E · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-042, PX-007
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources H4

##### Build

Collect receive/transmit bytes and packet rates with interface identity and scope. Clarify loopback and virtual-interface aggregation. Do not call these per-process network measurements.
Support interface disappearance/recreation and display unknown rates during warm-up. Keep all capture passive; no packet capture or network probing.

##### Out of scope

No host configuration changes, automatic privilege grants, all-metric polling, or implementation of unrelated metric families.

##### Verification / acceptance criteria

Fixtures cover renamed/recreated interfaces, loopback, counters decreasing, and packet/byte unit distinction. Multiple interfaces do not silently double count in the chosen aggregate. A local bounded transfer verifies sampling.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-056.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-057 — Add pressure-stall indicators

**Phase:** E · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-056
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources K6

##### Build

Expose available CPU, memory, and I/O pressure statistics with their actual averaging windows and some/full meanings. Separate host from cgroup scope. These are observed stall metrics, not generic utilization percentages.
Use text and progress controls with short explanations. Absence of kernel support is visibly different from zero pressure; do not enable pressure monitoring by mutating host configuration.

##### Out of scope

No host configuration changes, automatic privilege grants, all-metric polling, or implementation of unrelated metric families.

##### Verification / acceptance criteria

Parse valid, partial, malformed, and future-field fixtures. Preserve windows and units. No unbounded pressure-generating workload is used in tests. Unknown scope cannot be presented as host-wide data.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-057.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-058 — Add host identity, task, descriptor, clock, and uptime meter variants

**Phase:** E · **Scope size:** S · **Status:** planned · **Gate:** none
**Dependencies:** PX-057
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources H4

##### Build

Fill remaining inexpensive host overview families: architecture/kernel identity, task/thread state counts, descriptor usage/limit where meaningful, remote date/time, and uptime formats. Audit the upstream meter registry for inexpensive missing variants and map each to an explicit subcase in this ticket.
Use semantic text and units. Do not use remote time as a source for local expiry security or monotonic rate calculation.

##### Out of scope

No host configuration changes, automatic privilege grants, all-metric polling, or implementation of unrelated metric families.

##### Verification / acceptance criteria

Catalogue and parity ledger cover each implemented variant. Missing descriptor limits and unknown task states are not replaced with invented values. Clock jumps do not affect sample intervals or action expiry.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-058.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-059 — Add optional battery and power-source meters

**Phase:** E · **Scope size:** S · **Status:** planned · **Gate:** none
**Dependencies:** PX-058
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources H4

##### Build

Expose supported battery charge, AC state, and source identity with bounded safe parsing. Handle multiple batteries with a documented aggregate rather than averaging percentages blindly.
Use ordinary metrics controls. Do not add local-client battery information to a remote-host panel, and do not control power settings.

##### Out of scope

No host configuration changes, automatic privilege grants, all-metric polling, or implementation of unrelated metric families.

##### Verification / acceptance criteria

Fixtures cover desktop/no-battery, multiple batteries, malformed power-supply data, removal, and unknown capacity. Optional collection does not fail the whole host sample.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-059.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

### Phase F — Cover optional Linux facilities

#### PX-060 — Add optional host GPU monitoring

**Phase:** F · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-042
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources H4

##### Build

Implement a read-only GPU provider for a specifically named, documented OS/vendor interface that is relevant to the pinned baseline. Expose supported device activity and memory metrics with source/availability. Split additional vendor backends into separate generated tickets instead of claiming one implementation covers all GPUs.
Bound discovery and sampling; hardware absence is normal. No driver installation, GPU stress test, or vendor command constructed from remote text.

##### Out of scope

No driver/service/kernel configuration, automatic installs, privileged helper, or implied support for untested providers/hardware.

##### Verification / acceptance criteria

Fixtures cover absent devices, multiple devices, driver errors, and differing engine/memory definitions. Live validation identifies the exact device/driver used. The parity ledger records which vendor/interface combinations remain unverified or missing.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-060.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-061 — Add supported per-process GPU accounting

**Phase:** F · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-060
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources H3, H4

##### Build

Extend the GPU provider with process-linked accounting only where the source supports reliable attribution. Join on a verified process instance and device identity; shared contexts and shared allocations require explicit aggregation rules.
Expose unavailable attribution as unavailable, not zero. Do not synthesize per-process usage by distributing host GPU load across visible processes.

##### Out of scope

No driver/service/kernel configuration, automatic installs, privileged helper, or implied support for untested providers/hardware.

##### Verification / acceptance criteria

Recorded data tests PID replacement, shared allocations, multiple engines/devices, and missing attribution. Summaries explain when per-process values cannot be summed. Disabled GPU columns impose no discovery/collection overhead.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-061.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-062 — Add zram meters

**Phase:** F · **Scope size:** S · **Status:** planned · **Gate:** none
**Dependencies:** PX-042
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources H4

##### Build

Add an optional zram collector and catalogue entries for supported capacity, compressed size, and memory consumption. Preserve numerator/denominator meanings and multiple-device scope.
Never create/configure a zram device. Handle source ABI variations through versioned fixture readers and explicit missing values.

##### Out of scope

No driver/service/kernel configuration, automatic installs, privileged helper, or implied support for untested providers/hardware.

##### Verification / acceptance criteria

Fixtures cover no zram, several devices, zero denominator, truncated statistics, and removal. Ratios use named units and do not confuse logical uncompressed data with physical memory usage.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-062.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-063 — Add zswap meters

**Phase:** F · **Scope size:** S · **Status:** planned · **Gate:** none
**Dependencies:** PX-042
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources H4

##### Build

Add an independent optional zswap provider for supported stored-page and pool-usage statistics. Do not equate zswap with zram, and do not combine their memory usage without a documented accounting rule.
Handle disabled/unsupported/permission-denied separately. No host configuration changes.

##### Out of scope

No driver/service/kernel configuration, automatic installs, privileged helper, or implied support for untested providers/hardware.

##### Verification / acceptance criteria

Fixtures test disabled zswap, enabled-but-empty pools, missing counters, and boundary values. This feature can fail without suppressing ordinary memory/swap measurements.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-063.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-064 — Add ZFS ARC and compressed-ARC meters

**Phase:** F · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-042
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources H4

##### Build

Implement read-only ARC metric collection for one explicitly supported host interface, including compressed/uncompressed interpretations when available. Add catalogue entries and standard-widget presentation.
Use optional discovery and a slow collection tier. Additional OS ABI support belongs in its platform adapter ticket; no filesystem administration or pool changes.

##### Out of scope

No driver/service/kernel configuration, automatic installs, privileged helper, or implied support for untested providers/hardware.

##### Verification / acceptance criteria

Fixtures cover no ZFS, missing metrics, changing capacities, and compressed ARC semantics. Live evidence identifies the actual ZFS/platform version; otherwise record hardware/software verification as pending.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-064.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-065 — Add read-only systemd status and service-count meters

**Phase:** F · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-042
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources H4

##### Build

Read systemd status and service counts through a fixed, bounded official interface or fixed executable/argument vector. Distinguish missing bus/library/tool from failed service state.
Collect at a slow rate and expose supported counts/state only. This ticket is not a service manager: no start/stop/restart, privileged bus access, or parsing shell commands supplied by the client.

##### Out of scope

No driver/service/kernel configuration, automatic installs, privileged helper, or implied support for untested providers/hardware.

##### Verification / acceptance criteria

Fixtures cover available, degraded, missing, timeout, and partial counts. Slow service queries cannot stall the process list. No service-changing operation is reachable through advertised actions.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-065.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-066 — Add read-only OpenRC status meters

**Phase:** F · **Scope size:** S · **Status:** planned · **Gate:** none
**Dependencies:** PX-042
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources H4

##### Build

Implement the corresponding OpenRC status/runlevel/count family through a documented fixed interface. Keep it independent of systemd detection; the absence of one system does not imply the presence of the other.
Parse bounded output defensively and publish availability. Do not add service controls.

##### Out of scope

No driver/service/kernel configuration, automatic installs, privileged helper, or implied support for untested providers/hardware.

##### Verification / acceptance criteria

Tests cover a supported OpenRC fixture, missing executable/interface, unexpected output, timeout, and changing counts. The wrong service-manager provider is never guessed into use.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-066.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-067 — Add SELinux state and complete optional-meter registration

**Phase:** F · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-042, PX-060, PX-061, PX-062, PX-063, PX-064, PX-065, PX-066
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources H4

##### Build

Add read-only SELinux status using a documented host interface; unknown, disabled, enforcing, and permissive must remain distinct where supported. Do not change policy.
Register all F-phase providers independently. Add a settings page that explains why each optional provider is available, absent, disabled, denied, or not implemented. Reconcile optional meter names against the pinned platform registry and append small gap tickets for anything not represented.

##### Out of scope

No driver/service/kernel configuration, automatic installs, privileged helper, or implied support for untested providers/hardware.

##### Verification / acceptance criteria

No optional provider failure prevents core startup. Every advertised optional metric has units, state, bounded collection, and a fixture. A missing provider is recorded as a parity gap rather than misrepresented as unsupported hardware.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-067.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

### Phase G — Reach customization and visualization parity

#### PX-068 — Let users choose and reorder process columns

**Phase:** G · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-042, PX-009
**Read:** D1 §§7.3, 8; D2 T28

##### Build

Build a column chooser from the catalogue: show/hide, order, and restore defaults. Use a Table/List plus movement buttons if optional drag controls or Select are absent. Sort keys stay typed and stable even when the displayed column is hidden.
Changing visible columns updates collection projection and cost scheduling. Avoid loading every expensive metric merely because it is listed in the chooser.
For R1, implement the chooser for existing cheap columns. Exercise cost tiers through fake providers; each later collector must verify its live enable/disable behavior.

##### Out of scope

No downloaded client logic, CSS/HTML engine, arbitrary immediate-mode drawing, or modifications to upstream htop settings.

##### Verification / acceptance criteria

Add/remove/reorder every cheap field without losing selection or identity. Disabled/unsupported fields have explanations. Enabling an expensive field requests its documented sampling tier; disabling it stops collection. Empty/invalid column sets recover safely.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-068.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-069 — Add named saved views and a versioned settings store

**Phase:** G · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-068
**Read:** D1 §§6.3, 7.3, 26–27; D2 T32

##### Build

Create named views containing columns, sort, filters, thread/tree mode, and selected meter definitions. Use server-side per-user application settings for authoritative view choices, scoped to app/schema/source compatibility. Keep endpoint credentials, local window geometry, and renderer appearance out of this file.
Support create/rename/delete/restore defaults and a bounded, atomic settings format with migration. A button-based view switcher is acceptable without Tabs. Never store action confirmations or pending process intents as preferences.

##### Out of scope

No downloaded client logic, CSS/HTML engine, arbitrary immediate-mode drawing, or modifications to upstream htop settings.

##### Verification / acceptance criteria

Restart the app and recover compatible settings. Truncated, oversized, unknown-version, and partially written files fail safely without losing a valid backup. Renaming/deleting a view cannot change process identity or accidentally execute an action.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-069.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-070 — Configure meter layout and non-graph display modes

**Phase:** G · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-069
**Read:** D1 §§7.4, 10–12; D2 T16, T28

##### Build

Allow selecting meter families, ordering them into a bounded number of semantic groups, hiding the header, and choosing supported text/bar presentation. Define semantically meaningful layout choices, not transmitted pixels or theme colors.
Keep device/CPU expansion bounded and use virtualized lists for large sets. Implement numeric/text fallbacks for every displayed value. Historical graphs are a later negotiated extension, not resource images refreshed every tick.
Support a later btop-inspired native dashboard: bounded CPU, memory/swap, disk, and network panels alongside the process table. Visibility/order are saved semantic preferences; pixel placement, window dimensions, and theme remain client-local.

##### Out of scope

No downloaded client logic, CSS/HTML engine, arbitrary immediate-mode drawing, or modifications to upstream htop settings.

##### Verification / acceptance criteria

Switching layout updates structure only when layout actually changes, not per sample. A high-core-count fixture remains bounded. Native resize/theme changes do not require resending server layout geometry.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-070.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-070-G01 — Specify the btop-inspired native dashboard and evaluation scope

**Phase:** G · **Scope size:** M · **Status:** planned · **Gate:** Dashboard-design
**Dependencies:** PX-031, PX-070
**Read:** D1 §§6–8, 12, 18, 22, 25–26, 31–32; PX-070–PX-073, PX-090; sources BT1

##### Build

Pin a btop++ reference tag/commit and document selected workflows: at-a-glance host activity, process discovery, and sampled-trend inspection. Keep htop 3.5.3 as the process-management baseline. This is not full btop parity or permission to copy upstream code/assets.
Specify CPU aggregate, memory/swap, disk, and network panels beside a searchable process table. Include units, sample intervals, independent timestamps, warming-up/stale/denied states, and compact/expanded layouts. Selecting a process opens the existing inspector and its explicitly sampled history.
Define bounded panel configuration, local resizing, keyboard focus, accessibility alternatives, host/account labels, and disconnected/paused presentation. Detailed per-CPU, temperature, and GPU panels arrive later with verified collectors and do not block the core dashboard.
Define objective tasks and pass/fail criteria before implementation: locate a busy process, inspect a trend, inspect disk/network activity, recover after disconnect. Compare matched sampling, fields, workloads, and environments; do not infer superiority from screenshots.

##### Out of scope

No btop parity commitment, copied code/assets, process-specific renderer logic, or R1 scope expansion.

##### Verification / acceptance criteria

Review generic widget/profile support. Every required panel has a collector/renderer owner, unit/freshness policy, finite budget, and accessible fallback. Record a reference version and evaluation protocol. Missing generic capabilities become prerequisite tickets. R1/safe termination do not depend on this dashboard.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-070-G01.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-071 — Specify a small retained metrics-visualization profile

**Phase:** G · **Scope size:** S · **Status:** planned · **Gate:** none
**Dependencies:** PX-070
**Read:** D1 §§6.4, 11, 15, 26; D2 T20, T31; T36 is NOT assumed

##### Build

Write a separately versioned optional profile for numeric gauges and bounded time series, with units, source timestamps, series identity, append/replace/evict semantics, axis intent, limits, and accessibility summaries. Use a project-owned canonical namespace, assigned through the normal negotiation mechanism; do not assume org.srui ownership.
Provide Standard Widget fallbacks and golden fixtures. No arbitrary paths, shaders, drawing scripts, or downloaded expressions. Explain why this is a domain-specific extension rather than implementing T36 VectorScene or stuffing graphs into Image resources.
Support independent panel timestamps, explicit gaps/staleness, units/denominators, bounded graph windows, and stable host/device identity. Independently sampled panels must not imply an atomic host snapshot.

##### Out of scope

No downloaded client logic, CSS/HTML engine, arbitrary immediate-mode drawing, or modifications to upstream htop settings.

##### Verification / acceptance criteria

A schema review checks units, negative values, missing samples/gaps, window bounds, profile-version mismatch, and unsupported-client fallback. Both sides can implement the meaning without AppKit types. No production graph is enabled in this specification-only ticket.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-071.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-072 — Publish retained metric-series data from the server

**Phase:** G · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-071
**Read:** D1 §§11, 12, 15, 18; D2 T20, T23, T31

##### Build

Implement the approved metrics profile on the Rust SDK side and a bounded series store. Publish sampled data, not draw frames; history starts when sampling starts and preserves gaps. Send the Standard Widget fallback when the client does not negotiate the extension.
Use source timestamps and monotonic sample sequencing. Evictions and resyncs have explicit semantics; do not hide a time-series buffer inside a giant scalar value.

##### Out of scope

No downloaded client logic, CSS/HTML engine, arbitrary immediate-mode drawing, or modifications to upstream htop settings.

##### Verification / acceptance criteria

Golden fixtures, repeated-sample suppression, bounded retention, counter reset, and reconnect snapshot tests pass. Unsupported clients receive a valid fallback subtree. Wire bytes depend on sampled changes, not a requested animation cadence.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-072.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-073 — Render native retained gauges and history graphs

**Phase:** G · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-072
**Read:** D1 §§11, 22, 26, 31; D2 T16, T31–T34

##### Build

Implement the negotiated profile in a separate generic client module. Render text, bars, segmented gauges, graph windows, and compact numeric/indicator modes needed by the baseline, adapting appearance locally. Preserve semantic labels and a nonvisual value/table alternative.
Implement local resizing, axes, tooltips/crosshair where appropriate, reduced motion, and gaps without synchronous network measurement. Do not put process-specific policy in the renderer. A fixed trusted renderer is permitted; server-supplied drawing code is not.
Use the same generic renderer for dashboard and inspector. Preserve local resizing/input feedback while remote drill-down results arrive asynchronously. Missing samples appear as gaps, not zero measurements or interpolated evidence.

##### Out of scope

No downloaded client logic, CSS/HTML engine, arbitrary immediate-mode drawing, or modifications to upstream htop settings.

##### Verification / acceptance criteria

The same series renders independently at different window sizes and display cadences with identical protocol traffic. Profile limits reject oversized input. VoiceOver or the accessibility API can obtain units, current values, series names, and unavailable state. Unsupported-profile fallback still works.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-073.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-073-G01 — Compose the native host dashboard and process-history drill-down

**Phase:** G · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-070-G01, PX-073, PX-054, PX-055, PX-056, PX-090
**Read:** D1 §§6–8, 12, 18, 22, 25–26, 31–32; PX-070–PX-073, PX-090; sources BT1

##### Build

Compose CPU, memory/swap, disk, and network panels with the existing table/inspector through SDK semantics and the generic metrics renderer. Reuse collectors, PX-072 bounded host series, and PX-090 selected-process history; do not create a second sampling/history subsystem.
Show units, sample intervals, independent timestamps, gaps, and current values. Use a modest default layout with configurable panel visibility/order and bounded device expansion. Preserve table selection/filter/viewport while panels update. Process drill-down shows only samples actually collected for that ProcessKey, with a clear recording start; exited tasks stay non-actionable.
Keep data and intent scoped to host/source/incarnation. Paused, historical, disconnected, or invalid-intent contexts cannot authorize process changes. Keep numeric/Standard Widget fallbacks usable without graphs. Host disk/network activity does not identify the responsible process.
GPU, temperature, and detailed per-CPU panels are optional later integrations through PX-052/PX-053/PX-060/PX-061 with their own live evidence.

##### Out of scope

No inferred per-process network attribution, always-on audit history, mandatory GPU support, or duplicate transport/sampling stack.

##### Verification / acceptance criteria

Fake clocks/counters verify gaps, resets, PID/device reuse, freshness, selection continuity, and finite retention. Demonstrate all four panels and sampled process-history drill-down on Linux with the native Mac client. Resizing/render cadence must not change sample traffic. Missing optional hardware cannot suppress the core dashboard.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-073-G01.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-073-G02 — Release gate for the measured native dashboard

**Phase:** G · **Scope size:** M · **Status:** planned · **Gate:** R-Dashboard
**Dependencies:** PX-073-G01, PX-095, PX-010-G01, PX-026-G01
**Read:** D1 §§6–8, 12, 18, 22, 25–26, 31–32; PX-070–PX-073, PX-090; sources BT1

##### Build

Package a later dashboard release using the existing install/update path. Demonstrate the four core panels, searchable process table, sampled-history inspector, configurable panels, and isolated host windows. Show host/account/incarnation and freshness.
Run PX-070-G01 evaluation tasks against the pinned btop version with matched fields, sampling, hardware, workload, and transport. Record collection overhead, memory/queue caps, payload versus transport bytes, local input/remote-result latency, and task-completion evidence. State specific observations rather than blanket superiority.
Exercise 0/100/300/600 ms RTT, constrained bandwidth, disconnect/resume, server replacement, and two-host isolation. Recheck keyboard/VoiceOver, compact windows, numeric fallback, and clean installation. Document optional GPU/per-CPU availability separately.

##### Out of scope

No full btop parity, unqualified better-than claims, fleet administration, or dashboard requirement for R1.

##### Verification / acceptance criteria

Publish reproducible measurements/native evidence for the support envelope. Graph/table updates stay bounded and identity-correct; stale/historical data cannot authorize actions; host sessions do not leak selection/history/intent. Required core/native evidence must pass. Optional panels are verified or explicitly unavailable. Passing does not certify full htop or btop parity.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-073-G02.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-074 — Add display preferences and remaining navigation modes

**Phase:** G · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-073
**Read:** D1 §§6.3, 7.3–7.5, 22.5; D2 T16, T28; sources H6

##### Build

Expose command-path/merged-name choices, change highlighting, task/CPU labels, sort/follow behavior, and stable-tree presentation equivalents from the baseline ledger. Implement a local generic presentation preference layer for density, number formatting, column widths, typography, contrast, and monochrome-equivalent appearance.
Keep server semantic roles separate from renderer colors. Provide a documented keyboard command map and help overlay; implement common actions via the same event path. Split any missing generic native widget mapping into its own reusable prerequisite.

##### Out of scope

No downloaded client logic, CSS/HTML engine, arbitrary immediate-mode drawing, or modifications to upstream htop settings.

##### Verification / acceptance criteria

Each preference has a persistence/migration test or explicit session-only designation. New/vanished highlighting includes text or accessibility meaning and bounded expiry. Local theme/column-resize changes do not mutate authoritative process data or generate a frame stream.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-074.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-075 — Add validated startup options and configuration import

**Phase:** G · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-074
**Read:** D1 §§19, 26–27; D2 T32; sources H2, H6

##### Build

Provide a documented app/launcher command-line interface for delay, initial filter, PID set, owner, sort, tree mode, read-only, help/version, and equivalent header/input preferences. Map htop terminal-specific flags to documented native equivalents or explicit not-applicable behavior; do not silently accept and ignore them.
Add an explicit, read-only import of compatible htoprc settings into a preview, then the new settings format. A local import uses a user-selected file in client chrome; a remote import reads only the authenticated user's approved path. Never overwrite htop's file or execute config values.

##### Out of scope

No downloaded client logic, CSS/HTML engine, arbitrary immediate-mode drawing, or modifications to upstream htop settings.

##### Verification / acceptance criteria

Malformed arguments, conflicting options, huge files, unknown fields, and future versions have clear failures. Import is idempotent and preserves the original file byte-for-byte. No credentials or process intents are imported. Every CLI/config ledger row has a mapping or an open gap.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-075.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

### Phase H — Add advanced inspection

#### PX-076 — Add a bounded server-side diagnostic job runner

**Phase:** H · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-075, PX-031
**Read:** D1 §§7.7, 19.2, 26–27; D2 T27, T32

##### Build

Create an internal runner for allowlisted diagnostic adapters using fixed executable paths/argv schemas, captured stdout/stderr, timeout, output-byte limit, cancellation, environment minimization, and cleanup. Client events select an advertised diagnostic action, not arbitrary commands or executable paths.
Bind every job to a validated ProcessKey/intent and capture source/permission context. Treat external-tool PID targeting as a separate identity-safety question; a precheck alone is not a proof against later PID reuse. Intrusive tools require explicit preparation/confirmation.

##### Out of scope

No general remote shell-execution API, debugger/memory mutation, automatic privilege escalation, or unbounded diagnostic output.

##### Verification / acceptance criteria

Fake tools cover hang, flood, malformed output, launch failure, cancellation, and child cleanup. Client command/path injection is impossible. A finished job cannot replace a newer inspector selection. Jobs cannot starve UI/control or survive beyond configured policy unnoticed.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-076.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-077 — Inspect open descriptors and file information

**Phase:** H · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-076
**Read:** D1 §§8, 26–27; D2 T28, T32; sources H5, K1

##### Build

Implement an on-demand descriptor table using a bounded OS reader. Add an optional lsof adapter with a machine-readable format or safely bounded display output to cover the richer baseline workflow. Keep paths, types, descriptor IDs, and source freshness explicit.
Use asynchronous virtualization for large results. No file contents are read, no remote files are mapped onto client paths, and no local file-open action is implied by a displayed path.

##### Out of scope

No general remote shell-execution API, debugger/memory mutation, automatic privilege escalation, or unbounded diagnostic output.

##### Verification / acceptance criteria

Test pipes, sockets, deleted files, inaccessible descriptors, descriptor reuse, and process exit during collection. A large fixture remains bounded and searchable. Optional lsof absence yields an explanation, not a broken inspector or an invented empty result.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-077.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-078 — Inspect active file locks

**Phase:** H · **Scope size:** S · **Status:** planned · **Gate:** none
**Dependencies:** PX-077
**Read:** D1 §§8, 27; D2 T28, T32; sources H5

##### Build

Add a read-only process lock view through the diagnostic source abstraction. Preserve lock type, range, owner scope, and unknown process association. Do not assume every lock is cleanly attributable to one PID.
Support refresh, search, and clear loading/denied states. Do not acquire or release production locks.

##### Out of scope

No general remote shell-execution API, debugger/memory mutation, automatic privilege escalation, or unbounded diagnostic output.

##### Verification / acceptance criteria

Fixtures cover whole-file/range locks, blocked entries, special ownership cases, inaccessible data, and process disappearance. Tests acquire only bounded locks on temporary files owned by the harness and always release them.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-078.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-079 — Add opt-in environment inspection with privacy controls

**Phase:** H · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-078
**Read:** D1 §§26–27; D2 T32; sources H5, K1

##### Build

Add an explicit environment-inspection action for the selected authorized process. Fetch lazily, bound bytes, and redact likely secret values by default with an intentional reveal workflow. Label heuristic redaction as incomplete protection; do not claim every secret is detectable.
Do not copy environment values into logs, persistent history, telemetry, or general exports. Apply the diagnostic runner/source cancellation and permission rules.

##### Out of scope

No general remote shell-execution API, debugger/memory mutation, automatic privilege escalation, or unbounded diagnostic output.

##### Verification / acceptance criteria

Fixtures contain tokens, multiline values, unusual names, invalid bytes, huge environments, and permission denial. Sensitive raw values are absent from logs/default history. Reveal is explicit and scoped; changing selection cancels stale disclosure.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-079.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-080 — Inspect memory mappings and related details

**Phase:** H · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-079
**Read:** D1 §§8, 26–27; D2 T28, T32; sources K1

##### Build

Add an on-demand memory-map table with address ranges, permissions, backing description, and selected accounting fields. Keep it observational; no memory reads/writes, process injection, or policy modification.
Treat mapping information as a sampled view with bounded reads and explicit freshness. Support virtualized navigation, search, and selected-map details without collecting every expensive field on every tick.

##### Out of scope

No general remote shell-execution API, debugger/memory mutation, automatic privilege escalation, or unbounded diagnostic output.

##### Verification / acceptance criteria

Fixtures cover huge maps, anonymous mappings, deleted libraries, unreadable maps, concurrent changes, and invalid entries. A malformed source cannot exceed UI limits or crash the renderer. No address is interpreted as executable client logic.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-080.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-081 — Launch and stop a process system-call trace

**Phase:** H · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-080
**Read:** D1 §§18.2, 27; D2 T24, T32; sources H5

##### Build

Add an explicitly confirmed strace-style diagnostic with a supported fixed argument policy, exact target validation, permissions, timeout, and bounded retention. Document attachment side effects and operation-specific targeting limitations. Use the diagnostic job runner; no arbitrary shell command field.
Implement start/status/stop lifecycle first. On disconnect, choose a documented bounded continue-or-stop policy and ensure later reattach cannot silently create a second tracing job.

##### Out of scope

No general remote shell-execution API, debugger/memory mutation, automatic privilege escalation, or unbounded diagnostic output.

##### Verification / acceptance criteria

An opt-in test-owned worker can be traced and detached. Denied attachment, absent tool, exited target, cancellation, and transport loss clean up reliably. A duplicate event cannot spawn a second job. Never kill or leave a production target stopped as cleanup.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-081.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-082 — Render bounded trace output and add an optional terminal island

**Phase:** H · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-081
**Read:** D1 §§19.2, 21, 26; D2 T25, T27, T30–T32

##### Build

Render trace results as a bounded read-only model or RichText stream with source timestamps, search, pause display, truncation/gap markers, and backpressure. Keep arbitrary VT sequences inert in semantic text.
Optionally expose an explicitly opened Terminal node for a registered diagnostic/shell workflow using T30, clearly isolated from the semantic inspector. A Terminal fallback is not proof that the corresponding semantic feature is implemented.

##### Out of scope

No general remote shell-execution API, debugger/memory mutation, automatic privilege escalation, or unbounded diagnostic output.

##### Verification / acceptance criteria

High-volume trace output stays within memory/queue limits and does not delay input ACKs. Output expiry/gaps are visible. Terminal buffer exhaustion affects only the terminal island. With terminal capability absent, the semantic process UI remains fully usable.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-082.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-083 — Collect and display opt-in process/thread backtraces

**Phase:** H · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-082
**Read:** D1 §§8, 27; D2 T28, T32; sources H5

##### Build

Implement one supported server-side backtrace adapter with bounded frames/threads, explicit permissions, a fixed executable/library interface, and cleanup if attachment stops a target. Separate collecting the stack from presenting frames; display unresolved symbols honestly.
If the adapter requires an intrusive attach or cannot meet the target-identity policy, keep it gated and record the exact parity gap. Add a read-only frame/thread view using standard collections; do not add a general debugger or memory editor.

##### Out of scope

No general remote shell-execution API, debugger/memory mutation, automatic privilege escalation, or unbounded diagnostic output.

##### Verification / acceptance criteria

A debug-symbol-enabled disposable worker gives a usable stack. Test missing symbols, optimized frames, denied attachment, target exit, timeout, and release of any stopped target. Partial results are labeled and stale jobs cannot overwrite another process inspector.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-083.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

### Phase I — Prove scale, safety, and Linux coverage

#### PX-084 — Make live sorted and filtered collections range-safe

**Phase:** I · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-010-G01, PX-021
**Read:** D1 §§8, 12, 22.7; D2 T28, T33–T34

##### Build

Exercise T28 against a changing process/thread model with large synthetic counts. Bind asynchronous range results to source/query generation and a consistent model revision; stale responses must not populate the wrong ordering. Apply authoritative transforms on the server, with explicit cached-window placeholders.
Reuse existing range operations. If required query/range revision semantics are missing, add a small reviewed protocol/profile prerequisite rather than inventing out-of-band row-index rules. Keep pinned selection details independent of viewport cache eviction.
Extend the early harness to the full 100,000-row target and tree/thread behavior; this is not the first validation of live sorting and range progress.

##### Out of scope

No unreviewed new protocol semantics, weakened tests to obtain parity, silent feature exclusions, or unrelated product features.

##### Verification / acceptance criteria

A 100,000-record fake model supports scrolling, sorting, filtering, churn, and delayed ranges without wrong-row data. Exactly one native table/outline adapter exists per view, with bounded reusable row views. Uncached scrolling is local and does not masquerade as instantaneous data delivery.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-084.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-085 — Enforce application memory, history, collection, and traffic budgets

**Phase:** I · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-084, PX-076, PX-072, PX-059, PX-067
**Read:** D1 §§12.1, 20.4, 26, 31; D2 T25, T27–T28, T34

##### Build

Define configurable finite budgets for sample working sets, process identity history, tombstones, diagnostic jobs/output, metrics series, query caches, client ranges, and concurrent external tools. Store live identities only as long as needed; numeric IDs remain non-reused without retaining every dead record forever.
Measure active-field collection cost. Respect v0.6 scalar-only SET_PROPERTY coalescing: MODEL_UPDATE traffic cannot be merged using that rule. Throttle/batch at the app source or use the specified detach/resync path, never drop committed structural/model operations.
Consolidate caps already required by each feature and PX-010-G01. Earlier collectors, diagnostics, and history cannot defer finite budgets until this audit.

##### Out of scope

No unreviewed new protocol semantics, weakened tests to obtain parity, silent feature exclusions, or unrelated product features.

##### Verification / acceptance criteria

Long-running churn fixtures plateau in memory. Slow clients, many threads, expensive columns, and diagnostic floods remain bounded. Final state converges under throttling. Identical sample inputs produce identical semantic traffic at multiple renderer cadences.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-085.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-086 — Run application-specific reconnect and stale-intent fault tests

**Phase:** I · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-085, PX-041, PX-024, PX-083
**Read:** D1 §§18–18.3 and Appendix B; D2 T23–T24, T33–T34

##### Build

Create a named explorer fault suite spanning mid-transaction, pending selection/query, query-range response, pending confirmation, effect-before-ACK, partial bulk completion, terminal output, and same/replaced incarnation. Reuse Core fault hooks rather than building a second retry stack.
Include the handler-abort boundary: an OS effect and in-memory settlement are not universally atomic. Avoid fallible work after effects where practical, record uncertain outcomes, and never claim crash-proof exactly-once effects. A new event ID must never be allocated automatically merely to repeat uncertain old intent.

##### Out of scope

No unreviewed new protocol semantics, weakened tests to obtain parity, silent feature exclusions, or unrelated product features.

##### Verification / acceptance criteria

Same-session settled replay cannot redispatch. Out-of-order settlement frontiers and stale ACKs behave correctly. Replaced sessions abandon pending events and confirmations. Expired intent is terminally refused. The test report distinguishes proven retry safety from crash/OS uncertainty.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-086.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-087 — Audit sensitive data, parsing, and accessibility end to end

**Phase:** I · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-086, PX-083
**Read:** D1 §§22.8–22.9, 26–27; D2 T32–T35

##### Build

Audit command lines, environment, paths, identifiers, traces, maps, snapshots, settings, and diagnostic stderr for overcollection and unwanted persistence. Apply output/string/frame/rate bounds and sanitize display without converting remote text into code or local paths.
Test keyboard-only operation, accessibility roles/actions, contrast/non-color meanings, and disabled controls. Security hardening applies to new providers and optional profiles as well as Core. Do not expose cross-process automation yet.

##### Out of scope

No unreviewed new protocol semantics, weakened tests to obtain parity, silent feature exclusions, or unrelated product features.

##### Verification / acceptance criteria

Malicious fixtures cannot execute scripts, open local files/URLs, access clipboard contents, or bypass action policy. Hidden secret fields are not collected/exported. Every active view supports semantic inspection with no AppKit identity leakage.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-087.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-088 — Close the pinned Linux htop feature ledger

**Phase:** I · **Scope size:** S · **Status:** planned · **Gate:** R3
**Dependencies:** PX-087, PX-041, PX-059, PX-067, PX-075
**Read:** D1 §§31–32; D2 T33–T34; sources H1–H8

##### Build

Re-enumerate htop 3.5.3 actions, available fields, meter registrations, display settings, startup options, and optional compile-time features for the named Linux build. Compare with the machine-readable ledger, not memory or screenshots. Account for entries present in source but omitted from the manual.
For every unresolved implementable gap, create a separately numbered small ticket (suffixes preserve existing IDs), implement it, and rerun the relevant tests before marking closure. Break a large gap into collector/UI/verification work. This gate is not permission to write all missing features in one unreviewable commit.
Distinguish functional equivalence, native presentation adaptation, permission/environment unavailable, not implemented, and verified. Missing code is never excused as absent hardware. Keep PCP, additional OS backends, privileged administration, and terminal-only deployment as separate explicitly tracked scopes.

##### Out of scope

No unreviewed new protocol semantics, weakened tests to obtain parity, silent feature exclusions, or unrelated product features.

##### Verification / acceptance criteria

Every upstream inventory entry has a mapping and evidence/status. Ordinary-user Linux features are verified on an identified reference system; optional hardware/build paths need fixtures plus named live evidence before being called verified. The release claim lists exclusions and cannot say full parity while any required feature is missing.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-088.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-089 — Package and document the verified Linux/macOS release

**Phase:** I · **Scope size:** M · **Status:** planned · **Gate:** R3-release
**Dependencies:** PX-088
**Read:** D1 §§19, 25–27, 31–32; D2 T19, T32–T34

##### Build

Provide reproducible Rust server/app builds and macOS client distribution instructions using the existing SSH subsystem model. Document the user-owned daemon/socket, minimum supported OS/kernel/API versions, optional providers, deployment rollback, dependency licences/notices, and signing requirements where applicable.
Publish measured resource use, traffic, interaction latency, accessibility results, parity report, and a concise migration guide for htop users. Support a clean uninstall without deleting unrelated config or remote user data.
Extend R1 packaging from PX-026-G01; initial distribution must not wait for this parity release.

##### Out of scope

No unreviewed new protocol semantics, weakened tests to obtain parity, silent feature exclusions, or unrelated product features.

##### Verification / acceptance criteria

Install/connect/use/uninstall succeeds in clean test environments. Unknown host keys are never silently accepted. The feature report is generated from the ledger, build provenance is recorded, and release artifacts contain no secrets or unsafe default privileges.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-089.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

### Phase J — Go beyond the baseline

#### PX-090 — Retain a short, explicitly sampled process history

**Phase:** J · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-027, PX-072, PX-021, PX-010-G01
**Read:** D1 §§17–18, 20.4, 26

##### Build

Add an opt-in bounded server-side history store for selected/watched processes and a limited host summary. Use count, byte, and age limits; choose an initial 15-minute maximum with a configurable lower cap. Store observed samples and gaps, not inferred continuous behavior or complete process-audit events.
Keep sensitive argv/environment/trace payloads out of this store by default. Persist nothing across daemon restart in this first implementation. Do not confuse application sample history with the SRUI transaction replay journal.

##### Out of scope

No autonomous process termination, fleet orchestration, LLM dependency, cloud telemetry, or implicit local-resource permissions.

##### Verification / acceptance criteria

Fake-clock retention, process exit/PID reuse, detach continuation, sample gaps, and memory budgets pass. A new daemon starts a clearly new recording. No secret fixture value enters history or logs.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-090.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-091 — Inspect historical samples without changing live authority

**Phase:** J · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-090
**Read:** D1 §§6.3, 11, 12, 18

##### Build

Add an explicit Live/History view and time selection using standard controls plus the optional metrics profile when available. Show sampled values, timestamp, source, and gaps. Historical records are a separate read-only projection; do not rewind the live SemanticStore revision.
Disable all process controls in History. Keep the live source collecting under its policy; returning to Live presents current data and does not replay old actions.

##### Out of scope

No autonomous process termination, fleet orchestration, LLM dependency, cloud telemetry, or implicit local-resource permissions.

##### Verification / acceptance criteria

Selecting past samples never regresses the live protocol revision or re-enables historical targets. PID reuse is visually distinct. Clients without the metrics profile can use a table fallback. Concurrent reconnect produces correct live/history source labels.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-091.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-092 — Compare two recorded samples

**Phase:** J · **Scope size:** S · **Status:** planned · **Gate:** none
**Dependencies:** PX-091
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21

##### Build

Add a semantic comparison of two chosen samples: new/no-longer-observed identities, CPU/RSS changes, and changed state. Align by ProcessKey, not PID or name. Distinguish actual absence from incomplete sample coverage.
Present an ordinary Table with added/removed/changed categories and transparent formulas. Do not claim causality or identify a memory leak from two observations.

##### Out of scope

No autonomous process termination, fleet orchestration, LLM dependency, cloud telemetry, or implicit local-resource permissions.

##### Verification / acceptance criteria

Fixtures include PID reuse, missing samples, unequal intervals, and repeated names. Deltas preserve units, unavailable values, and observation timestamps. Comparison cannot trigger remote process actions.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-092.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-093 — Add user-defined threshold observations without automatic remediation

**Phase:** J · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-092
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21

##### Build

Add a bounded set of explicit CPU/RSS/pressure threshold rules with duration, hysteresis, cooldown, and source scope. Evaluate on the server's observed samples; record unknown state across gaps. Surface alerts in an in-app list.
Do not kill processes, launch commands, send webhooks, or request local notifications. Those are separate capabilities/products. Limits apply to rule count and evaluation cost.

##### Out of scope

No autonomous process termination, fleet orchestration, LLM dependency, cloud telemetry, or implicit local-resource permissions.

##### Verification / acceptance criteria

Fake samples prove duration/hysteresis/cooldown semantics and no firing during unknown gaps. Alerts for old process instances do not attach to PID replacements. No rule can contain executable text or invoke ProcessActions.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-093.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-094 — Export an explicitly selected diagnostic snapshot

**Phase:** J · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-093
**Read:** D1 §§6.3, 8, 26–27; D2 T32, T35

##### Build

Add a trusted local client command that saves explicitly selected, already materialized semantic data through a user-chosen Save destination. Label export scope, source, capture time, missing ranges, schema version, and units; offer CSV and structured JSON. Requesting full data must use bounded normal range retrieval and explicit scope, not pretend the cache is complete.
Redact sensitive fields by default and neutralize spreadsheet-formula prefixes in CSV. This is a local user-initiated operation; the server cannot choose a local path or trigger writes. No generic remote-file capability is implied.

##### Out of scope

No autonomous process termination, fleet orchestration, LLM dependency, cloud telemetry, or implicit local-resource permissions.

##### Verification / acceptance criteria

Export requires explicit user action and destination; cancellation writes nothing. Partial-cache exports state that they are partial. A hostile filename/path/cell cannot escape the chosen destination, execute a formula, or silently reveal environment/argv secrets. Round-trip structured data preserves units and identity scope.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-094.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-095 — Add multiple isolated host windows

**Phase:** J · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-027, PX-031
**Read:** D1 §§6.2–6.3, 17–18; D2 T23–T24, T38

##### Build

Extend the generic connection manager to support several concurrent sessions with unmistakable host/account/incarnation labels. Keep outboxes, source IDs, collection caches, confirmations, and reconnect generations isolated per session.
Do not add cross-host bulk actions or a fleet database. A side-by-side view may compare the same basic host metrics, with per-host timestamps and incompatible denominators explicitly labeled.

##### Out of scope

No autonomous process termination, fleet orchestration, LLM dependency, cloud telemetry, or implicit local-resource permissions.

##### Verification / acceptance criteria

Two hosts with identical PIDs and names cannot share selection, actions, or cached identity. Replacing one session leaves the other intact. A confirmation remains visibly bound to one host/account and cannot be moved onto another session.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-095.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-096 — Add evidence-based explanations and a guided demo

**Phase:** J · **Scope size:** S · **Status:** planned · **Gate:** none
**Dependencies:** PX-095, PX-094
**Read:** D1 §§4, 22.9, 29; D2 T35

##### Build

Provide concise built-in metric definitions and deterministic observation summaries, such as sustained high sampled CPU or increasing sampled RSS. Every summary includes the data window, formula, and unknown conditions. Use server-owned logic and structured text, not downloaded client code.
Create an opt-in guided demo over disposable workers and local semantic inspection. Do not add an LLM dependency or imply diagnosis/causality that the samples cannot establish.

##### Out of scope

No autonomous process termination, fleet orchestration, LLM dependency, cloud telemetry, or implicit local-resource permissions.

##### Verification / acceptance criteria

Each explanation links to its recorded observations and disappears/becomes uncertain when data is insufficient. Demo workers remain bounded and fully cleaned up. All instructional actions still use normal authorization and confirmation.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-096.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-097 — Release gate: demonstrate concrete advantages beyond parity

**Phase:** J · **Scope size:** M · **Status:** planned · **Gate:** R4
**Dependencies:** PX-096, PX-073-G02
**Read:** D1 §§31–32; D2 T33–T35

##### Build

Publish matched-workload results for navigation under latency, reconnect recovery, semantic automation, sampled history, and export. Separate claims of functional coverage from usability/performance results; record the htop version and test environment.
Show a complete scripted journey: connect, find, inspect, confirm an owned-worker action, interrupt/reconnect, inspect last-observed history, and export a redacted snapshot. Preserve a modest default screen; advanced features remain progressively disclosed.
Include the dashboard journey from PX-073-G02. Separate htop coverage from btop-inspired workflow comparisons; appearance alone establishes neither parity nor superiority.

##### Out of scope

No autonomous process termination, fleet orchestration, LLM dependency, cloud telemetry, or implicit local-resource permissions.

##### Verification / acceptance criteria

All claimed advantages have reproducible evidence. No full-htop/environment-parity claim is made until the relevant branch ledgers also close. Release notes distinguish tested improvements from subjective product preference and remaining limitations.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-097.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

### Phase K — Administrative parity without a root daemon

#### PX-098 — Design an optional narrowly privileged remote helper

**Phase:** K · **Scope size:** S · **Status:** planned · **Gate:** SECURITY-REVIEW
**Dependencies:** PX-028, PX-029, PX-087, PX-088
**Read:** D1 §§25–27; D2 T32

##### Build

Produce a security design only for capabilities that an unprivileged daemon cannot supply but the declared administrative parity tier requires. Preserve the invariant that sessiond and ssh-bridge refuse effective UID 0. A separate administrator-installed helper is not implicitly approved by T32.
Specify peer credentials, account binding, explicit operation allowlists, target identity, least privileges, resource limits, audit fields, replay/expiry, descriptor passing, cancellation, and refusal of arbitrary paths/commands. Decide safe semantics separately for pidfd-capable and PID-only operations. No generic sudo shell or client-provided executable.
Mark this branch blocked until an explicit human security review approves the contract; the coding agent must not self-approve a new privilege boundary.
Before implementation, produce a bounded feasibility prototype, supported API/platform matrix, review/test-environment requirements, and separate effort estimate. Split follow-on work into suffixed tickets. This branch is outside the R1 and safe-termination commitment.

##### Out of scope

No unapproved privilege boundary, generic sudo/shell broker, client-chosen executable/path, or self-approval of the security review.

##### Verification / acceptance criteria

Threat model covers confused-deputy access, socket substitution, namespace identity, stale intent, forged credentials, replay, and PID reuse. Each intended privileged feature maps to a narrowly defined operation and a test plan. No privileged binary, service unit, or privilege grant is installed.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-098.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-099 — Implement the reviewed helper boundary without process-changing operations

**Phase:** K · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-098
**Read:** D1 §§25–27; approved admin contract

##### Build

Only after review approval, implement the minimal server/helper transport, kernel peer-credential checks, message/FD validation, explicit caller policy, version negotiation, and audit sink. Start with a harmless capability query; default deny.
Keep packaging/install opt-in and administrator-controlled. Do not run the helper from a downloaded client command or give the ordinary daemon new ambient authority.

##### Out of scope

No unapproved privilege boundary, generic sudo/shell broker, client-chosen executable/path, or self-approval of the security review.

##### Verification / acceptance criteria

Cross-user, invalid-FD, oversized, unknown-operation, expired, and replayed messages fail closed. Runtime paths and peer credentials are independently checked. A privileged integration suite runs only in a disposable VM; ordinary tests use an unprivileged harness.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-099.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-100 — Add privileged operations one reviewed operation at a time

**Phase:** K · **Scope size:** M · **Status:** planned · **Gate:** ADMIN-per-operation
**Dependencies:** PX-099
**Read:** D1 §§18.2, 27; approved admin contract

##### Build

Start with exactly one approved operation through the helper, with read-only/deny behavior for everything else. Connect its result to the existing immutable action intent. Preserve target-handle identity and exactly the approved privilege scope.
Instantiate this ticket separately for each additional operation in the reviewed allowlist; use suffixes and separate commits/tests. A monolithic implement-all-admin-actions change is prohibited. PID-only operations lacking an approved safe targeting design remain explicit blockers to administrative parity.

##### Out of scope

No unapproved privilege boundary, generic sudo/shell broker, client-chosen executable/path, or self-approval of the security review.

##### Verification / acceptance criteria

Each operation's instance has normal, denial, stale-target, duplicate-intent, partial-result, and cleanup tests. Prove the helper cannot execute arbitrary code or widen a caller's target set. The administrative ledger cannot close while any required operation is missing or unsafe.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-100.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

### Phase L — PCP and declarative dynamic metrics

#### PX-101 — Add a read-only PCP metric catalogue adapter

**Phase:** L · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-042, PX-088
**Read:** D1 §§6.5, 8, 11, 27; sources H2

##### Build

Audit the separately versioned pcp-htop/PCP configuration contract before implementation and record a pinned reference build. Add an optional server-side adapter for metric names, units, types, instance domains, and availability through official PCP APIs. Use administrator/user-configured allowed sources, not remote UI supplied executable code or arbitrary credential-bearing URLs.
Keep PCP an optional feature and ordinary Linux monitoring independent. Arbitrary metric instances are not ProcessKeys and never automatically gain process actions.
Before implementation, produce a bounded feasibility prototype, supported API/platform matrix, review/test-environment requirements, and separate effort estimate. Split follow-on work into suffixed tickets. This branch is outside the R1 and safe-termination commitment.

##### Out of scope

No compulsory PCP dependency, arbitrary client code, implicit process controls on metrics, or unbounded expression/query execution.

##### Verification / acceptance criteria

A fake PCP source exercises metadata, missing metrics, instance churn, timeouts, and unsupported types. No process controls are advertised for generic metric rows. Missing PCP dependencies do not break the base build. Record exact primary API references used.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-101.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-102 — Fetch and display bounded PCP metric instances

**Phase:** L · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-101, PX-072, PX-073
**Read:** D1 §§8, 11, 20.4, 26

##### Build

Implement bounded sampled retrieval, unit conversion, source timestamps, instance identity, and error handling for the catalogue. Render selected metrics through the existing standard controls and optional metrics profile.
Set caps for metrics/instances/query cost and handle disappearing/reappearing instance domains. Do not implicitly connect to arbitrary third-party endpoints or collect all available metrics.

##### Out of scope

No compulsory PCP dependency, arbitrary client code, implicit process controls on metrics, or unbounded expression/query execution.

##### Verification / acceptance criteria

Fixtures verify units, missing values, source gaps, source replacement, large instance domains, and cancellation. A configured real PCP endpoint validates one metric family. Typed sorting and visualization retain source provenance.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-102.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-103 — Add declarative dynamic metric views and PCP parity closure

**Phase:** L · **Scope size:** M · **Status:** planned · **Gate:** PCP-parity
**Dependencies:** PX-102, PX-069
**Read:** D1 §§8, 11, 26–27; sources H2

##### Build

Add bounded declarative definitions for saved metric columns/meters/screens using allowlisted catalogue references. Validate schema and dependencies; configuration remains data, not downloaded code or arbitrary expressions. Where reference PCP supports expressions, evaluate only through a reviewed server-side bounded facility or record a named gap and create its own ticket.
Build a separate pcp-htop parity ledger. Do not claim that ordinary htop Linux parity automatically covers the PCP variant or all its configurations.

##### Out of scope

No compulsory PCP dependency, arbitrary client code, implicit process controls on metrics, or unbounded expression/query execution.

##### Verification / acceptance criteria

Malformed/oversized configs, missing metrics, changing instances, and unsupported expressions fail clearly. Each baseline dynamic-view behavior maps to evidence or a gap. All required PCP gaps are split into bounded tickets before this branch can claim closure.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-103.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

### Phase M — Monitored-host platform parity

#### PX-104 — Freeze the portable collector/action conformance contract

**Phase:** M · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-089, PX-028, PX-042
**Read:** D1 §§1, 5.1, 27, 32; D2 T33; T37 is NOT assumed; sources H1

##### Build

Extract the proven platform-neutral provider boundaries without rewriting the Linux implementation: process/source identity, capability reporting, sample units, field availability, authorization, operation scope, and cleanup. Define recorded adapter fixtures and a shared test runner.
Separate client platform from monitored-host platform. macOS/AppKit is still the initial renderer; a FreeBSD collector does not require a FreeBSD GUI. Enumerate every targeted upstream OS/build independently, including Solaris and illumos where their interfaces differ.
Before implementation, produce a bounded feasibility prototype, supported API/platform matrix, review/test-environment requirements, and separate effort estimate. Split follow-on work into suffixed tickets. This branch is outside the R1 and safe-termination commitment.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

Linux still passes unchanged application tests. A fake non-Linux provider with different process states, signal numbering, and page sizes renders through the unmodified client. No /proc path, Linux PID assumption, or AppKit class escapes the appropriate adapter boundary.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-104.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-105 — macOS/Darwin: implement the basic read-only collector

**Phase:** M · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-104
**Read:** D1 §§1, 5.1, 6.2, 32; platform primary API docs to record; sources H1

##### Build

Add a macOS/Darwin provider for the R0 fields and robust process-instance identity. Use current primary OS documentation and native process/task interfaces and supported system metrics; verify Apple Silicon page sizes and process/thread semantics; pin the reference OS version and htop build in the ledger before coding. No process-changing operations.
Keep unsupported metrics explicit and preserve process/thread distinctions. Add native recorded fixtures, cancellation, and source/boot identity. Do not simply run Linux parsers against similarly named fields.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

The R0 live scenario passes on an identified macOS/Darwin host or VM, and adapter unit fixtures run in CI where possible. PID reuse, missing permissions, process exit, large values, and invalid names are covered. A cross-compile alone does not count as runtime verification.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-105.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-106 — macOS/Darwin: add extended fields and meter families

**Phase:** M · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-105
**Read:** D1 §§5.1, 6.5, 8, 32; pinned platform registries

##### Build

Enumerate the pinned macOS/Darwin upstream field/meter registries. Add the next missing supported family through the portable catalogue, with correct native units and availability. Instantiate a separate suffixed ticket for each additional nontrivial family; do not bundle an entire OS telemetry backend into one change.
Retain consistent UI semantics while documenting where OS definitions genuinely differ. Optional facilities such as ZFS, sensors, or hardware counters require their own capability and evidence.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

Every implemented family has native fixtures, unit/aggregation tests, cost limits, and live provenance. Missing implementation is distinguished from source-OS unsupported functionality. The Linux provider and generic client remain unchanged in behavior.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-106.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-107 — macOS/Darwin: add reviewed process actions and diagnostics

**Phase:** M · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-105, PX-028, PX-031, PX-076
**Read:** D1 §§7.7, 18.2, 27; native API docs to record

##### Build

Audit the OS target-identity and authorization model, then implement exactly one supported action or diagnostic through the existing contracts. Instantiate additional action/diagnostic families as separate suffixed tickets. Never assume Linux pidfd, ptrace, or signal semantics exist on macOS/Darwin.
Preserve confirmations, stale-target rejection, read-only mode, source binding, limits, and cleanup. Operations without an adequate identity-safety design stay explicitly unavailable and block the relevant full-parity claim; do not add unreviewed privilege elevation.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

Each instance tests valid action on an owned disposable worker, denied permission, identity replacement, duplicated event, timeout, and cleanup. Where live native CI is unavailable, report unverified rather than passing the platform release gate.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-107.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-108 — macOS/Darwin: close the platform feature ledger and release gate

**Phase:** M · **Scope size:** S · **Status:** planned · **Gate:** DARWIN-parity
**Dependencies:** PX-106, PX-107, PX-086
**Read:** D1 §§19, 25–27, 31–32

##### Build

Compare the complete pinned macOS/Darwin htop build with this adapter: user workflows, fields, meters, actions, optional integrations, and permission conditions. Require completion of all instantiated field/action gap tickets before closure. Publish supported OS versions and measured collection overhead.
Package the server app and same-user transport for the platform. Keep renderer availability as a separate deployment claim; this gate establishes monitored-host support.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

Run the full application conformance, stale-identity, action, reconnect, and bounded-resource suite on macOS/Darwin. Every required upstream feature has evidence or an explicit remaining gap that prevents full parity. No root sessiond or silent fallback changes are introduced.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-108.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-109 — FreeBSD: implement the basic read-only collector

**Phase:** M · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-104
**Read:** D1 §§1, 5.1, 6.2, 32; platform primary API docs to record; sources H1

##### Build

Add a FreeBSD provider for the R0 fields and robust process-instance identity. Use current primary OS documentation and supported process/sysctl interfaces and FreeBSD memory/accounting definitions; pin the reference OS version and htop build in the ledger before coding. No process-changing operations.
Keep unsupported metrics explicit and preserve process/thread distinctions. Add native recorded fixtures, cancellation, and source/boot identity. Do not simply run Linux parsers against similarly named fields.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

The R0 live scenario passes on an identified FreeBSD host or VM, and adapter unit fixtures run in CI where possible. PID reuse, missing permissions, process exit, large values, and invalid names are covered. A cross-compile alone does not count as runtime verification.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-109.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-110 — FreeBSD: add extended fields and meter families

**Phase:** M · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-109
**Read:** D1 §§5.1, 6.5, 8, 32; pinned platform registries

##### Build

Enumerate the pinned FreeBSD upstream field/meter registries. Add the next missing supported family through the portable catalogue, with correct native units and availability. Instantiate a separate suffixed ticket for each additional nontrivial family; do not bundle an entire OS telemetry backend into one change.
Retain consistent UI semantics while documenting where OS definitions genuinely differ. Optional facilities such as ZFS, sensors, or hardware counters require their own capability and evidence.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

Every implemented family has native fixtures, unit/aggregation tests, cost limits, and live provenance. Missing implementation is distinguished from source-OS unsupported functionality. The Linux provider and generic client remain unchanged in behavior.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-110.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-111 — FreeBSD: add reviewed process actions and diagnostics

**Phase:** M · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-109, PX-028, PX-031, PX-076
**Read:** D1 §§7.7, 18.2, 27; native API docs to record

##### Build

Audit the OS target-identity and authorization model, then implement exactly one supported action or diagnostic through the existing contracts. Instantiate additional action/diagnostic families as separate suffixed tickets. Never assume Linux pidfd, ptrace, or signal semantics exist on FreeBSD.
Preserve confirmations, stale-target rejection, read-only mode, source binding, limits, and cleanup. Operations without an adequate identity-safety design stay explicitly unavailable and block the relevant full-parity claim; do not add unreviewed privilege elevation.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

Each instance tests valid action on an owned disposable worker, denied permission, identity replacement, duplicated event, timeout, and cleanup. Where live native CI is unavailable, report unverified rather than passing the platform release gate.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-111.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-112 — FreeBSD: close the platform feature ledger and release gate

**Phase:** M · **Scope size:** S · **Status:** planned · **Gate:** FREEBSD-parity
**Dependencies:** PX-110, PX-111, PX-086
**Read:** D1 §§19, 25–27, 31–32

##### Build

Compare the complete pinned FreeBSD htop build with this adapter: user workflows, fields, meters, actions, optional integrations, and permission conditions. Require completion of all instantiated field/action gap tickets before closure. Publish supported OS versions and measured collection overhead.
Package the server app and same-user transport for the platform. Keep renderer availability as a separate deployment claim; this gate establishes monitored-host support.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

Run the full application conformance, stale-identity, action, reconnect, and bounded-resource suite on FreeBSD. Every required upstream feature has evidence or an explicit remaining gap that prevents full parity. No root sessiond or silent fallback changes are introduced.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-112.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-113 — NetBSD: implement the basic read-only collector

**Phase:** M · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-104
**Read:** D1 §§1, 5.1, 6.2, 32; platform primary API docs to record; sources H1

##### Build

Add a NetBSD provider for the R0 fields and robust process-instance identity. Use current primary OS documentation and NetBSD-specific process/sysctl interfaces and kernel feature availability; pin the reference OS version and htop build in the ledger before coding. No process-changing operations.
Keep unsupported metrics explicit and preserve process/thread distinctions. Add native recorded fixtures, cancellation, and source/boot identity. Do not simply run Linux parsers against similarly named fields.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

The R0 live scenario passes on an identified NetBSD host or VM, and adapter unit fixtures run in CI where possible. PID reuse, missing permissions, process exit, large values, and invalid names are covered. A cross-compile alone does not count as runtime verification.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-113.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-114 — NetBSD: add extended fields and meter families

**Phase:** M · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-113
**Read:** D1 §§5.1, 6.5, 8, 32; pinned platform registries

##### Build

Enumerate the pinned NetBSD upstream field/meter registries. Add the next missing supported family through the portable catalogue, with correct native units and availability. Instantiate a separate suffixed ticket for each additional nontrivial family; do not bundle an entire OS telemetry backend into one change.
Retain consistent UI semantics while documenting where OS definitions genuinely differ. Optional facilities such as ZFS, sensors, or hardware counters require their own capability and evidence.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

Every implemented family has native fixtures, unit/aggregation tests, cost limits, and live provenance. Missing implementation is distinguished from source-OS unsupported functionality. The Linux provider and generic client remain unchanged in behavior.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-114.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-115 — NetBSD: add reviewed process actions and diagnostics

**Phase:** M · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-113, PX-028, PX-031, PX-076
**Read:** D1 §§7.7, 18.2, 27; native API docs to record

##### Build

Audit the OS target-identity and authorization model, then implement exactly one supported action or diagnostic through the existing contracts. Instantiate additional action/diagnostic families as separate suffixed tickets. Never assume Linux pidfd, ptrace, or signal semantics exist on NetBSD.
Preserve confirmations, stale-target rejection, read-only mode, source binding, limits, and cleanup. Operations without an adequate identity-safety design stay explicitly unavailable and block the relevant full-parity claim; do not add unreviewed privilege elevation.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

Each instance tests valid action on an owned disposable worker, denied permission, identity replacement, duplicated event, timeout, and cleanup. Where live native CI is unavailable, report unverified rather than passing the platform release gate.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-115.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-116 — NetBSD: close the platform feature ledger and release gate

**Phase:** M · **Scope size:** S · **Status:** planned · **Gate:** NETBSD-parity
**Dependencies:** PX-114, PX-115, PX-086
**Read:** D1 §§19, 25–27, 31–32

##### Build

Compare the complete pinned NetBSD htop build with this adapter: user workflows, fields, meters, actions, optional integrations, and permission conditions. Require completion of all instantiated field/action gap tickets before closure. Publish supported OS versions and measured collection overhead.
Package the server app and same-user transport for the platform. Keep renderer availability as a separate deployment claim; this gate establishes monitored-host support.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

Run the full application conformance, stale-identity, action, reconnect, and bounded-resource suite on NetBSD. Every required upstream feature has evidence or an explicit remaining gap that prevents full parity. No root sessiond or silent fallback changes are introduced.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-116.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-117 — OpenBSD: implement the basic read-only collector

**Phase:** M · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-104
**Read:** D1 §§1, 5.1, 6.2, 32; platform primary API docs to record; sources H1

##### Build

Add a OpenBSD provider for the R0 fields and robust process-instance identity. Use current primary OS documentation and OpenBSD-specific process interfaces and permission/privilege restrictions; pin the reference OS version and htop build in the ledger before coding. No process-changing operations.
Keep unsupported metrics explicit and preserve process/thread distinctions. Add native recorded fixtures, cancellation, and source/boot identity. Do not simply run Linux parsers against similarly named fields.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

The R0 live scenario passes on an identified OpenBSD host or VM, and adapter unit fixtures run in CI where possible. PID reuse, missing permissions, process exit, large values, and invalid names are covered. A cross-compile alone does not count as runtime verification.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-117.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-118 — OpenBSD: add extended fields and meter families

**Phase:** M · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-117
**Read:** D1 §§5.1, 6.5, 8, 32; pinned platform registries

##### Build

Enumerate the pinned OpenBSD upstream field/meter registries. Add the next missing supported family through the portable catalogue, with correct native units and availability. Instantiate a separate suffixed ticket for each additional nontrivial family; do not bundle an entire OS telemetry backend into one change.
Retain consistent UI semantics while documenting where OS definitions genuinely differ. Optional facilities such as ZFS, sensors, or hardware counters require their own capability and evidence.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

Every implemented family has native fixtures, unit/aggregation tests, cost limits, and live provenance. Missing implementation is distinguished from source-OS unsupported functionality. The Linux provider and generic client remain unchanged in behavior.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-118.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-119 — OpenBSD: add reviewed process actions and diagnostics

**Phase:** M · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-117, PX-028, PX-031, PX-076
**Read:** D1 §§7.7, 18.2, 27; native API docs to record

##### Build

Audit the OS target-identity and authorization model, then implement exactly one supported action or diagnostic through the existing contracts. Instantiate additional action/diagnostic families as separate suffixed tickets. Never assume Linux pidfd, ptrace, or signal semantics exist on OpenBSD.
Preserve confirmations, stale-target rejection, read-only mode, source binding, limits, and cleanup. Operations without an adequate identity-safety design stay explicitly unavailable and block the relevant full-parity claim; do not add unreviewed privilege elevation.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

Each instance tests valid action on an owned disposable worker, denied permission, identity replacement, duplicated event, timeout, and cleanup. Where live native CI is unavailable, report unverified rather than passing the platform release gate.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-119.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-120 — OpenBSD: close the platform feature ledger and release gate

**Phase:** M · **Scope size:** S · **Status:** planned · **Gate:** OPENBSD-parity
**Dependencies:** PX-118, PX-119, PX-086
**Read:** D1 §§19, 25–27, 31–32

##### Build

Compare the complete pinned OpenBSD htop build with this adapter: user workflows, fields, meters, actions, optional integrations, and permission conditions. Require completion of all instantiated field/action gap tickets before closure. Publish supported OS versions and measured collection overhead.
Package the server app and same-user transport for the platform. Keep renderer availability as a separate deployment claim; this gate establishes monitored-host support.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

Run the full application conformance, stale-identity, action, reconnect, and bounded-resource suite on OpenBSD. Every required upstream feature has evidence or an explicit remaining gap that prevents full parity. No root sessiond or silent fallback changes are introduced.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-120.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-121 — DragonFly BSD: implement the basic read-only collector

**Phase:** M · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-104
**Read:** D1 §§1, 5.1, 6.2, 32; platform primary API docs to record; sources H1

##### Build

Add a DragonFly BSD provider for the R0 fields and robust process-instance identity. Use current primary OS documentation and DragonFly-specific process/accounting interfaces rather than assuming FreeBSD ABI compatibility; pin the reference OS version and htop build in the ledger before coding. No process-changing operations.
Keep unsupported metrics explicit and preserve process/thread distinctions. Add native recorded fixtures, cancellation, and source/boot identity. Do not simply run Linux parsers against similarly named fields.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

The R0 live scenario passes on an identified DragonFly BSD host or VM, and adapter unit fixtures run in CI where possible. PID reuse, missing permissions, process exit, large values, and invalid names are covered. A cross-compile alone does not count as runtime verification.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-121.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-122 — DragonFly BSD: add extended fields and meter families

**Phase:** M · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-121
**Read:** D1 §§5.1, 6.5, 8, 32; pinned platform registries

##### Build

Enumerate the pinned DragonFly BSD upstream field/meter registries. Add the next missing supported family through the portable catalogue, with correct native units and availability. Instantiate a separate suffixed ticket for each additional nontrivial family; do not bundle an entire OS telemetry backend into one change.
Retain consistent UI semantics while documenting where OS definitions genuinely differ. Optional facilities such as ZFS, sensors, or hardware counters require their own capability and evidence.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

Every implemented family has native fixtures, unit/aggregation tests, cost limits, and live provenance. Missing implementation is distinguished from source-OS unsupported functionality. The Linux provider and generic client remain unchanged in behavior.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-122.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-123 — DragonFly BSD: add reviewed process actions and diagnostics

**Phase:** M · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-121, PX-028, PX-031, PX-076
**Read:** D1 §§7.7, 18.2, 27; native API docs to record

##### Build

Audit the OS target-identity and authorization model, then implement exactly one supported action or diagnostic through the existing contracts. Instantiate additional action/diagnostic families as separate suffixed tickets. Never assume Linux pidfd, ptrace, or signal semantics exist on DragonFly BSD.
Preserve confirmations, stale-target rejection, read-only mode, source binding, limits, and cleanup. Operations without an adequate identity-safety design stay explicitly unavailable and block the relevant full-parity claim; do not add unreviewed privilege elevation.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

Each instance tests valid action on an owned disposable worker, denied permission, identity replacement, duplicated event, timeout, and cleanup. Where live native CI is unavailable, report unverified rather than passing the platform release gate.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-123.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-124 — DragonFly BSD: close the platform feature ledger and release gate

**Phase:** M · **Scope size:** S · **Status:** planned · **Gate:** DRAGONFLY-parity
**Dependencies:** PX-122, PX-123, PX-086
**Read:** D1 §§19, 25–27, 31–32

##### Build

Compare the complete pinned DragonFly BSD htop build with this adapter: user workflows, fields, meters, actions, optional integrations, and permission conditions. Require completion of all instantiated field/action gap tickets before closure. Publish supported OS versions and measured collection overhead.
Package the server app and same-user transport for the platform. Keep renderer availability as a separate deployment claim; this gate establishes monitored-host support.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

Run the full application conformance, stale-identity, action, reconnect, and bounded-resource suite on DragonFly BSD. Every required upstream feature has evidence or an explicit remaining gap that prevents full parity. No root sessiond or silent fallback changes are introduced.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-124.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-125 — Solaris: implement the basic read-only collector

**Phase:** M · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-104
**Read:** D1 §§1, 5.1, 6.2, 32; platform primary API docs to record; sources H1

##### Build

Add a Solaris provider for the R0 fields and robust process-instance identity. Use current primary OS documentation and Solaris process/accounting interfaces, projects/zones where represented by the reference build; pin the reference OS version and htop build in the ledger before coding. No process-changing operations.
Keep unsupported metrics explicit and preserve process/thread distinctions. Add native recorded fixtures, cancellation, and source/boot identity. Do not simply run Linux parsers against similarly named fields.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

The R0 live scenario passes on an identified Solaris host or VM, and adapter unit fixtures run in CI where possible. PID reuse, missing permissions, process exit, large values, and invalid names are covered. A cross-compile alone does not count as runtime verification.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-125.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-126 — Solaris: add extended fields and meter families

**Phase:** M · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-125
**Read:** D1 §§5.1, 6.5, 8, 32; pinned platform registries

##### Build

Enumerate the pinned Solaris upstream field/meter registries. Add the next missing supported family through the portable catalogue, with correct native units and availability. Instantiate a separate suffixed ticket for each additional nontrivial family; do not bundle an entire OS telemetry backend into one change.
Retain consistent UI semantics while documenting where OS definitions genuinely differ. Optional facilities such as ZFS, sensors, or hardware counters require their own capability and evidence.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

Every implemented family has native fixtures, unit/aggregation tests, cost limits, and live provenance. Missing implementation is distinguished from source-OS unsupported functionality. The Linux provider and generic client remain unchanged in behavior.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-126.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-127 — Solaris: add reviewed process actions and diagnostics

**Phase:** M · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-125, PX-028, PX-031, PX-076
**Read:** D1 §§7.7, 18.2, 27; native API docs to record

##### Build

Audit the OS target-identity and authorization model, then implement exactly one supported action or diagnostic through the existing contracts. Instantiate additional action/diagnostic families as separate suffixed tickets. Never assume Linux pidfd, ptrace, or signal semantics exist on Solaris.
Preserve confirmations, stale-target rejection, read-only mode, source binding, limits, and cleanup. Operations without an adequate identity-safety design stay explicitly unavailable and block the relevant full-parity claim; do not add unreviewed privilege elevation.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

Each instance tests valid action on an owned disposable worker, denied permission, identity replacement, duplicated event, timeout, and cleanup. Where live native CI is unavailable, report unverified rather than passing the platform release gate.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-127.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-128 — Solaris: close the platform feature ledger and release gate

**Phase:** M · **Scope size:** S · **Status:** planned · **Gate:** SOLARIS-parity
**Dependencies:** PX-126, PX-127, PX-086
**Read:** D1 §§19, 25–27, 31–32

##### Build

Compare the complete pinned Solaris htop build with this adapter: user workflows, fields, meters, actions, optional integrations, and permission conditions. Require completion of all instantiated field/action gap tickets before closure. Publish supported OS versions and measured collection overhead.
Package the server app and same-user transport for the platform. Keep renderer availability as a separate deployment claim; this gate establishes monitored-host support.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

Run the full application conformance, stale-identity, action, reconnect, and bounded-resource suite on Solaris. Every required upstream feature has evidence or an explicit remaining gap that prevents full parity. No root sessiond or silent fallback changes are introduced.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-128.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-129 — illumos: implement the basic read-only collector

**Phase:** M · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-104
**Read:** D1 §§1, 5.1, 6.2, 32; platform primary API docs to record; sources H1

##### Build

Add a illumos provider for the R0 fields and robust process-instance identity. Use current primary OS documentation and illumos process/accounting interfaces and distribution-specific availability rather than assuming Solaris equivalence; pin the reference OS version and htop build in the ledger before coding. No process-changing operations.
Keep unsupported metrics explicit and preserve process/thread distinctions. Add native recorded fixtures, cancellation, and source/boot identity. Do not simply run Linux parsers against similarly named fields.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

The R0 live scenario passes on an identified illumos host or VM, and adapter unit fixtures run in CI where possible. PID reuse, missing permissions, process exit, large values, and invalid names are covered. A cross-compile alone does not count as runtime verification.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-129.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-130 — illumos: add extended fields and meter families

**Phase:** M · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-129
**Read:** D1 §§5.1, 6.5, 8, 32; pinned platform registries

##### Build

Enumerate the pinned illumos upstream field/meter registries. Add the next missing supported family through the portable catalogue, with correct native units and availability. Instantiate a separate suffixed ticket for each additional nontrivial family; do not bundle an entire OS telemetry backend into one change.
Retain consistent UI semantics while documenting where OS definitions genuinely differ. Optional facilities such as ZFS, sensors, or hardware counters require their own capability and evidence.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

Every implemented family has native fixtures, unit/aggregation tests, cost limits, and live provenance. Missing implementation is distinguished from source-OS unsupported functionality. The Linux provider and generic client remain unchanged in behavior.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-130.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-131 — illumos: add reviewed process actions and diagnostics

**Phase:** M · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-129, PX-028, PX-031, PX-076
**Read:** D1 §§7.7, 18.2, 27; native API docs to record

##### Build

Audit the OS target-identity and authorization model, then implement exactly one supported action or diagnostic through the existing contracts. Instantiate additional action/diagnostic families as separate suffixed tickets. Never assume Linux pidfd, ptrace, or signal semantics exist on illumos.
Preserve confirmations, stale-target rejection, read-only mode, source binding, limits, and cleanup. Operations without an adequate identity-safety design stay explicitly unavailable and block the relevant full-parity claim; do not add unreviewed privilege elevation.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

Each instance tests valid action on an owned disposable worker, denied permission, identity replacement, duplicated event, timeout, and cleanup. Where live native CI is unavailable, report unverified rather than passing the platform release gate.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-131.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-132 — illumos: close the platform feature ledger and release gate

**Phase:** M · **Scope size:** S · **Status:** planned · **Gate:** ILLUMOS-parity
**Dependencies:** PX-130, PX-131, PX-086
**Read:** D1 §§19, 25–27, 31–32

##### Build

Compare the complete pinned illumos htop build with this adapter: user workflows, fields, meters, actions, optional integrations, and permission conditions. Require completion of all instantiated field/action gap tickets before closure. Publish supported OS versions and measured collection overhead.
Package the server app and same-user transport for the platform. Keep renderer availability as a separate deployment claim; this gate establishes monitored-host support.

##### Out of scope

No pretending one BSD/Solaris ABI covers another; no changes to other adapters or unreviewed privilege elevation.

##### Verification / acceptance criteria

Run the full application conformance, stale-identity, action, reconnect, and bounded-resource suite on illumos. Every required upstream feature has evidence or an explicit remaining gap that prevents full parity. No root sessiond or silent fallback changes are introduced.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-132.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

### Phase N — Terminal-only deployment

#### PX-133 — Build a minimal terminal-only SRUI replica client

**Phase:** N · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-104, PX-071
**Read:** D1 §§12.1, 15, 18–19, 32; D2 T37 not assumed

##### Build

Create a separately packaged Rust terminal client using the existing encoding/transport libraries where appropriate. It must use replica transaction semantics, not feed coalesced replica deltas into the authoritative Rust store. Implement same-user local attachment and existing SSH transport without turning the binary SRUI channel into a PTY.
Start with a headless state dump for Surface/Text/Table and a truthful required-profile capability declaration. This is not assumed to exist merely because T37 was listed as optional.
Before implementation, produce a bounded feasibility prototype, supported API/platform matrix, review/test-environment requirements, and separate effort estimate. Split follow-on work into suffixed tickets. This branch is outside the R1 and safe-termination commitment.

##### Out of scope

No htop screen scraping, second process-policy implementation, GUI dependency, or bypass of SRUI event/security semantics.

##### Verification / acceptance criteria

Core golden vectors, normal commits, coalesced deltas, explicit snapshots, and continuity decisions match the macOS replica. It runs without a GUI session. Unsupported required semantics fail explicitly rather than being silently ignored.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-133.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-134 — Render the explorer's read-only views in a terminal

**Phase:** N · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-133
**Read:** D1 §§5.1, 8, 12.2, 22.7, 32

##### Build

Implement retained terminal presentation for the standard controls actually used: layout, text/rich text, table/tree/list, progress, and loading states. Support bounded row virtualization, resizing, ASCII/Unicode and monochrome preferences, and a semantic fallback for metrics graphs.
Do not screen-scrape htop or reverse-engineer terminal output into the semantic model. The app remains the same server-side explorer, not a duplicate application.

##### Out of scope

No htop screen scraping, second process-policy implementation, GUI dependency, or bypass of SRUI event/security semantics.

##### Verification / acceptance criteria

The read-only explorer works in a small and large terminal with keyboard navigation and bounded memory. Rendering cadence does not change the semantic mutation stream. Missing ranges remain explicit and scrolling does not synchronously wait for layout measurements.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-134.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-135 — Add terminal-client semantic editing and safe actions

**Phase:** N · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-134, PX-086
**Read:** D1 §§7.6–7.7, 18.2, 26–27, 32

##### Build

Add local text editing/search, focus, buttons/toggles, confirmations, and event outbox integration for the explorer's workflows. Provide documented htop-style key equivalents where they do not conflict with editing, plus mouse support as an option.
All actions pass through the existing server authorization/intent pipeline. Terminal compatibility islands are optional; show a safe fallback rather than creating uncontrolled nested shells. Do not claim AppKit-equivalent IME/accessibility support without platform evidence.

##### Out of scope

No htop screen scraping, second process-policy implementation, GUI dependency, or bypass of SRUI event/security semantics.

##### Verification / acceptance criteria

Keyboard-only search/filter/select/confirm succeeds over latency. Duplicate/out-of-order ACKs, full event windows, disabled controls, and conflicting editor keys are tested. A malicious remote label cannot emit active terminal control sequences.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-135.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-136 — Close terminal-only deployment parity

**Phase:** N · **Scope size:** M · **Status:** planned · **Gate:** TUI-parity
**Dependencies:** PX-135, PX-075
**Read:** D1 §§18–19, 26–27, 31–32

##### Build

Implement/reuse resume, saved endpoints as appropriate, error handling, and local-user launch packaging. Audit every pinned startup/keyboard/display workflow for a terminal-native equivalent, including no-GUI operation. Add separate gap tickets for unmet renderer behavior.
Publish which monitored-host platforms can also run this client locally. Embedding htop or a Terminal node inside the macOS app does not satisfy this gate.

##### Out of scope

No htop screen scraping, second process-policy implementation, GUI dependency, or bypass of SRUI event/security semantics.

##### Verification / acceptance criteria

Start locally without macOS/desktop access, perform the standard workflow, interrupt/reconnect, and verify identity-safe actions. Tests cover narrow terminals, input-only operation, ASCII/monochrome, output escaping, and resource limits. Every claimed deployment has a real runtime test.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-136.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

### Phase O — Permission-gated external automation

#### PX-137 — Design a permission model for external local automation

**Phase:** O · **Scope size:** S · **Status:** planned · **Gate:** IPC-REVIEW
**Dependencies:** PX-026, PX-087, PX-095
**Read:** D1 §§4.18, 22.9, 26; D2 T35 explicitly excludes IPC

##### Build

Specify a local, opt-in inspection/action API above the existing T35 surface: caller authentication, consent, session/source binding, data redaction, action scopes, revocation, rate limits, and audit. Make read-only inspection the first scope; action execution is independently granted and cannot bypass confirmation/server policy.
Keep the interface disabled by default. Do not expose NSView identity, allow remote sessions to grant local consent, or make an unauthenticated network listener. This ticket is design/review only.
Before implementation, produce a bounded feasibility prototype, supported API/platform matrix, review/test-environment requirements, and separate effort estimate. Split follow-on work into suffixed tickets. This branch is outside the R1 and safe-termination commitment.

##### Out of scope

No unauthenticated/network-wide listener, remote-granted local consent, AppKit pointer API, or blanket local filesystem/clipboard authority.

##### Verification / acceptance criteria

Threat-model tests cover another local user, an untrusted process, a replaced remote session, revoked consent, and sensitive-field inspection. Security review is recorded before implementation; an agent cannot self-authorize a new local permission boundary.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-137.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-138 — Expose reviewed, opt-in read-only semantic inspection

**Phase:** O · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-137
**Read:** D1 §22.9; approved IPC contract

##### Build

Implement only the approved local read-only scope with authenticated callers, consent/revocation, bounded queries, and snapshot/source identity. Reuse the T35 semantic API. Do not expose action methods or secret-bearing data merely because it exists in the replica.
Add a small local demonstration client that lists a permitted tree and selected metrics without coordinates or screenshot parsing.

##### Out of scope

No unauthenticated/network-wide listener, remote-granted local consent, AppKit pointer API, or blanket local filesystem/clipboard authority.

##### Verification / acceptance criteria

Unauthorized, revoked, cross-user, over-budget, and sensitive-field requests fail. Session replacement invalidates stale handles. No operation mutates the replica, activates a control, or starts a remote action.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-138.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

#### PX-139 — Expose scoped semantic actions to trusted local automation

**Phase:** O · **Scope size:** M · **Status:** planned · **Gate:** AUTOMATION
**Dependencies:** PX-138, PX-028, PX-086
**Read:** D1 §§18.2, 22.9, 26–27; approved IPC contract

##### Build

Implement the separately approved action scope through exactly the same EventOutbox, intent preparation, user confirmation where required, and server policy as human interaction. Bind every handle to session incarnation and source; prevent stale actor handles from surviving replacement.
Demonstrate a harmless action and an explicitly user-approved disposable-worker operation. No autonomous remediation, downloaded agent code, or blanket local filesystem/clipboard permission.

##### Out of scope

No unauthenticated/network-wide listener, remote-granted local consent, AppKit pointer API, or blanket local filesystem/clipboard authority.

##### Verification / acceptance criteria

A trusted caller cannot bypass disabled/read-only/confirmation states or target another session. Duplicate requests preserve retry identity and revocation takes immediate effect. Audit events record caller, semantic action, target scope, and outcome without secret values.

##### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-139.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

---

## References and provenance

Checked 11 September 2026. The htop tag is the feature baseline, not a request to copy its source. Task designs, staging, budgets, and acceptance criteria are proposed engineering work, not upstream claims. Version-specific OS/API details must be verified again when each adapter is implemented. The original two documents are not reproduced in this pack.

- **[D1] User-supplied SRUI Semantic Remote UI design, v0.6, dated 29 August 2026.** `SRUI_Semantic_Remote_UI_Design_v0.6.md` at the repository root.
- **[D2] User-supplied sequential implementation plan T0–T38.** `SRUI_Implementation_Plan.md` at the repository root; completion is established by the revision-specific baseline, not by ticket numbering.
- **[H1] htop official project and releases; 3.5.3 was marked Latest when checked.** `https://htop.dev/ ; https://github.com/htop-dev/htop/releases`
- **[H2] htop 3.5.3 manual: user workflows, options and PCP variant.** `https://raw.githubusercontent.com/htop-dev/htop/3.5.3/htop.1.in`
- **[H3] htop 3.5.3 Linux process-field registry.** `https://raw.githubusercontent.com/htop-dev/htop/3.5.3/linux/LinuxProcess.c`
- **[H4] htop 3.5.3 Linux platform: meters and platform actions.** `https://raw.githubusercontent.com/htop-dev/htop/3.5.3/linux/Platform.c`
- **[H5] htop 3.5.3 common action registry.** `https://raw.githubusercontent.com/htop-dev/htop/3.5.3/Action.c`
- **[H6] htop 3.5.3 display-option definitions.** `https://raw.githubusercontent.com/htop-dev/htop/3.5.3/DisplayOptionsPanel.c`
- **[H7] htop 3.5.3 scheduling implementation.** `https://raw.githubusercontent.com/htop-dev/htop/3.5.3/Scheduling.c`
- **[H8] htop 3.5.3 changelog; corroborates optional and recent feature coverage.** `https://raw.githubusercontent.com/htop-dev/htop/3.5.3/ChangeLog`
- **[K1] Linux kernel /proc documentation: process data and field semantics.** `https://docs.kernel.org/filesystems/proc.html`
- **[K2] Linux man-pages: pidfd_send_signal.** `https://man7.org/linux/man-pages/man2/pidfd_send_signal.2.html`
- **[K3] Linux man-pages: pidfd_open.** `https://man7.org/linux/man-pages/man2/pidfd_open.2.html`
- **[K4] Linux man-pages: sched_setaffinity.** `https://man7.org/linux/man-pages/man2/sched_setaffinity.2.html`
- **[K5] Linux man-pages: getpriority/setpriority.** `https://man7.org/linux/man-pages/man2/setpriority.2.html`
- **[K6] Linux kernel: Pressure Stall Information.** `https://docs.kernel.org/accounting/psi.html`
- **[S1] sysinfo Process API; implementation must use the repository-pinned version, not assume latest API compatibility.** `https://docs.rs/sysinfo/latest/sysinfo/struct.Process.html`

- **[BT1] btop++ by aristocratos — dashboard inspiration.** [Upstream repository](https://github.com/aristocratos/btop). Pin a reference tag/commit and selected workflows at PX-070-G01. No latest-version, feature-parity, or performance claim was verified for this amendment.
