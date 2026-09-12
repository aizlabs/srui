# Task index — SRUI Process Explorer

Edition 1.2 · 12 September 2026. IDs are stable; dependencies determine execution. All tickets remain planned until their own evidence passes. See [BASELINE.md](BASELINE.md) and [EXECUTION_TIMELINE.md](EXECUTION_TIMELINE.md).

## Delivery order

This priority order keeps R1 and safe termination first, then the later dashboard. Honor prerequisites; an assigned ticket is not authorization to execute the entire queue.

1. [PX-000](tickets/PX-000-audit.md) — Establish the app baseline and a versioned htop feature ledger
2. [PX-001](tickets/PX-001-shell.md) — Display an empty Process Explorer window over existing SRUI
3. [PX-002](tickets/PX-002-fake.md) — Populate the table with a deterministic fake process source
4. [PX-003](tickets/PX-003-real.md) — Read one real Linux process snapshot with instance identity
5. [PX-004](tickets/PX-004-live.md) — Refresh the process list with incremental transactions
6. [PX-005](tickets/PX-005-rss.md) — Add resident memory with accurate units and unavailable states
7. [PX-006](tickets/PX-006-cpu.md) — Add sampled per-process CPU usage
8. [PX-007](tickets/PX-007-summary.md) — Add the first system summary and data-freshness indicator
9. [PX-008](tickets/PX-008-firstgate.md) — Release gate: the minimal read-only monitor
10. [PX-042](tickets/PX-042-metricregistry.md) — Introduce a typed metric catalogue and collection-cost policy
11. [PX-009](tickets/PX-009-sort.md) — Sort by a selected column without losing process identity
12. [PX-010](tickets/PX-010-filter.md) — Add a responsive command filter
13. [PX-010-G01](tickets/PX-010-G01-early-scale.md) — Gate early live collection scale, progress, and resource budgets
14. [PX-068](tickets/PX-068-columns.md) — Let users choose and reorder process columns
15. [PX-011](tickets/PX-011-search.md) — Add find-next/find-previous and jump to PID
16. [PX-012](tickets/PX-012-users.md) — Filter by owner and an explicit PID set
17. [PX-013](tickets/PX-013-inspector.md) — Add a selected-process inspector
18. [PX-014](tickets/PX-014-command.md) — Show full command line and executable context
19. [PX-015](tickets/PX-015-pause.md) — Add sampling interval and a clearly labeled paused view
20. [PX-016](tickets/PX-016-keyboard.md) — Make the basic workflow keyboard accessible
21. [PX-017](tickets/PX-017-follow.md) — Add follow mode and stable viewport behavior
22. [PX-018](tickets/PX-018-tree.md) — Add parent/child process tree navigation
23. [PX-019](tickets/PX-019-threads.md) — Expose user threads and kernel-thread visibility
24. [PX-020](tickets/PX-020-tags.md) — Add stable multi-selection and descendant tagging
25. [PX-021](tickets/PX-021-lastseen.md) — Keep a last-observed record when a process disappears
26. [PX-022](tickets/PX-022-reconnect.md) — Expose disconnect, catch-up, and replacement correctly
27. [PX-023](tickets/PX-023-connect.md) — Add a minimal generic host connection form
28. [PX-024](tickets/PX-024-saved.md) — Save endpoints and safely reattach after client relaunch
29. [PX-025](tickets/PX-025-demo.md) — Add bounded demonstration workloads and latency scenarios
30. [PX-026](tickets/PX-026-automation.md) — Demonstrate semantic inspection without external IPC
31. [PX-026-G01](tickets/PX-026-G01-r1-package.md) — Gate installable R1 with compatibility, privacy, and native verification
32. [PX-027](tickets/PX-027-showcasegate.md) — Release gate: the first polished SRUI showcase
33. [PX-028](tickets/PX-028-policy.md) — Define action authorization and a dry-run action contract
34. [PX-029](tickets/PX-029-handles.md) — Resolve stable Linux process handles before enabling actions
35. [PX-030](tickets/PX-030-terminate.md) — Add confirmed graceful termination of one process
36. [PX-031](tickets/PX-031-outcomes.md) — Distinguish action acknowledgement from observed process outcome
37. [PX-095](tickets/PX-095-multihost.md) — Add multiple isolated host windows
38. [PX-069](tickets/PX-069-screens.md) — Add named saved views and a versioned settings store
39. [PX-070](tickets/PX-070-meterlayout.md) — Configure meter layout and non-graph display modes
40. [PX-070-G01](tickets/PX-070-G01-dashboard-design.md) — Specify the btop-inspired native dashboard and evaluation scope
41. [PX-071](tickets/PX-071-metricprofile.md) — Specify a small retained metrics-visualization profile
42. [PX-072](tickets/PX-072-metricserver.md) — Publish retained metric-series data from the server
43. [PX-073](tickets/PX-073-metricclient.md) — Render native retained gauges and history graphs
44. [PX-054](tickets/PX-054-hostmemory.md) — Add detailed host memory, swap, and huge-page statistics
45. [PX-055](tickets/PX-055-disk.md) — Add host and per-device disk activity
46. [PX-056](tickets/PX-056-network.md) — Add host and per-interface network activity
47. [PX-090](tickets/PX-090-history.md) — Retain a short, explicitly sampled process history
48. [PX-073-G01](tickets/PX-073-G01-dashboard-compose.md) — Compose the native host dashboard and process-history drill-down
49. [PX-073-G02](tickets/PX-073-G02-dashboard-gate.md) — Release gate for the measured native dashboard
50. [PX-091](tickets/PX-091-historicalview.md) — Inspect historical samples without changing live authority
51. [PX-092](tickets/PX-092-snapshotdiff.md) — Compare two recorded samples
52. [PX-093](tickets/PX-093-alerts.md) — Add user-defined threshold observations without automatic remediation
53. [PX-094](tickets/PX-094-export.md) — Export an explicitly selected diagnostic snapshot
54. [PX-096](tickets/PX-096-guided.md) — Add evidence-based explanations and a guided demo
55. [PX-097](tickets/PX-097-beyondgate.md) — Release gate: demonstrate concrete advantages beyond parity
56. [PX-032](tickets/PX-032-stopresume.md) — Add explicit Stop and Continue actions
57. [PX-033](tickets/PX-033-force.md) — Add explicit force termination without automatic escalation
58. [PX-034](tickets/PX-034-signals.md) — Add an advanced signal chooser
59. [PX-035](tickets/PX-035-bulk.md) — Apply actions to a frozen tagged target set
60. [PX-035-G01](tickets/PX-035-G01-action-feasibility.md) — Resolve identity-safe targeting feasibility for advanced controls
61. [PX-036](tickets/PX-036-nice.md) — Read and change nice values with operation-specific safety
62. [PX-037](tickets/PX-037-affinity.md) — Inspect and edit CPU affinity
63. [PX-038](tickets/PX-038-ioprio.md) — Inspect and edit I/O scheduling priority
64. [PX-039](tickets/PX-039-scheduling.md) — Inspect and edit scheduler policy safely
65. [PX-040](tickets/PX-040-autogroup.md) — Expose autogroup identity and priority changes
66. [PX-041](tickets/PX-041-controlgate.md) — Release gate: an everyday process explorer with safe controls
67. [PX-084](tickets/PX-084-virtualization.md) — Make live sorted and filtered collections range-safe
68. [PX-074](tickets/PX-074-displayprefs.md) — Add display preferences and remaining navigation modes
69. [PX-075](tickets/PX-075-startup.md) — Add validated startup options and configuration import
70. [PX-076](tickets/PX-076-runner.md) — Add a bounded server-side diagnostic job runner
71. [PX-057](tickets/PX-057-psi.md) — Add pressure-stall indicators
72. [PX-058](tickets/PX-058-hostextras.md) — Add host identity, task, descriptor, clock, and uptime meter variants
73. [PX-059](tickets/PX-059-battery.md) — Add optional battery and power-source meters
74. [PX-060](tickets/PX-060-gpu.md) — Add optional host GPU monitoring
75. [PX-061](tickets/PX-061-processgpu.md) — Add supported per-process GPU accounting
76. [PX-062](tickets/PX-062-zram.md) — Add zram meters
77. [PX-063](tickets/PX-063-zswap.md) — Add zswap meters
78. [PX-064](tickets/PX-064-zfs.md) — Add ZFS ARC and compressed-ARC meters
79. [PX-065](tickets/PX-065-systemd.md) — Add read-only systemd status and service-count meters
80. [PX-066](tickets/PX-066-openrc.md) — Add read-only OpenRC status meters
81. [PX-067](tickets/PX-067-selinux.md) — Add SELinux state and complete optional-meter registration
82. [PX-085](tickets/PX-085-budgets.md) — Enforce application memory, history, collection, and traffic budgets
83. [PX-077](tickets/PX-077-fds.md) — Inspect open descriptors and file information
84. [PX-078](tickets/PX-078-locks.md) — Inspect active file locks
85. [PX-079](tickets/PX-079-env.md) — Add opt-in environment inspection with privacy controls
86. [PX-080](tickets/PX-080-maps.md) — Inspect memory mappings and related details
87. [PX-081](tickets/PX-081-trace.md) — Launch and stop a process system-call trace
88. [PX-082](tickets/PX-082-traceview.md) — Render bounded trace output and add an optional terminal island
89. [PX-083](tickets/PX-083-backtrace.md) — Collect and display opt-in process/thread backtraces
90. [PX-086](tickets/PX-086-chaos.md) — Run application-specific reconnect and stale-intent fault tests
91. [PX-087](tickets/PX-087-privacy.md) — Audit sensitive data, parsing, and accessibility end to end
92. [PX-088](tickets/PX-088-parity.md) — Close the pinned Linux htop feature ledger
93. [PX-089](tickets/PX-089-package.md) — Package and document the verified Linux/macOS release
94. [PX-043](tickets/PX-043-processfields.md) — Add process identity, timing, and scheduling columns
95. [PX-044](tickets/PX-044-faults.md) — Add faults and context-switch accounting
96. [PX-045](tickets/PX-045-memoryfields.md) — Add detailed low-cost memory columns
97. [PX-046](tickets/PX-046-pss.md) — Add on-demand proportional memory accounting
98. [PX-047](tickets/PX-047-processio.md) — Add per-process I/O counters and rates
99. [PX-048](tickets/PX-048-cgroups.md) — Expose cgroups, namespaces, and container visibility
100. [PX-049](tickets/PX-049-securityfields.md) — Expose OOM, elevated privileges, and security context
101. [PX-050](tickets/PX-050-staleexe.md) — Show command variants and replaced executable/library warnings
102. [PX-051](tickets/PX-051-delayacct.md) — Add optional delay-accounting metrics
103. [PX-052](tickets/PX-052-cpudetail.md) — Add per-CPU usage classes and topology
104. [PX-053](tickets/PX-053-sensors.md) — Add CPU frequency and temperature readings
105. [PX-098](tickets/PX-098-adminspec.md) — Design an optional narrowly privileged remote helper
106. [PX-099](tickets/PX-099-admintransport.md) — Implement the reviewed helper boundary without process-changing operations
107. [PX-100](tickets/PX-100-adminops.md) — Add privileged operations one reviewed operation at a time
108. [PX-101](tickets/PX-101-pcpmetadata.md) — Add a read-only PCP metric catalogue adapter
109. [PX-102](tickets/PX-102-pcpsamples.md) — Fetch and display bounded PCP metric instances
110. [PX-103](tickets/PX-103-pcpscreens.md) — Add declarative dynamic metric views and PCP parity closure
111. [PX-104](tickets/PX-104-portcontract.md) — Freeze the portable collector/action conformance contract
112. [PX-105](tickets/PX-105-darwinbasic.md) — macOS/Darwin: implement the basic read-only collector
113. [PX-106](tickets/PX-106-darwinmetrics.md) — macOS/Darwin: add extended fields and meter families
114. [PX-107](tickets/PX-107-darwinactions.md) — macOS/Darwin: add reviewed process actions and diagnostics
115. [PX-108](tickets/PX-108-darwingate.md) — macOS/Darwin: close the platform feature ledger and release gate
116. [PX-109](tickets/PX-109-freebsdbasic.md) — FreeBSD: implement the basic read-only collector
117. [PX-110](tickets/PX-110-freebsdmetrics.md) — FreeBSD: add extended fields and meter families
118. [PX-111](tickets/PX-111-freebsdactions.md) — FreeBSD: add reviewed process actions and diagnostics
119. [PX-112](tickets/PX-112-freebsdgate.md) — FreeBSD: close the platform feature ledger and release gate
120. [PX-113](tickets/PX-113-netbsdbasic.md) — NetBSD: implement the basic read-only collector
121. [PX-114](tickets/PX-114-netbsdmetrics.md) — NetBSD: add extended fields and meter families
122. [PX-115](tickets/PX-115-netbsdactions.md) — NetBSD: add reviewed process actions and diagnostics
123. [PX-116](tickets/PX-116-netbsdgate.md) — NetBSD: close the platform feature ledger and release gate
124. [PX-117](tickets/PX-117-openbsdbasic.md) — OpenBSD: implement the basic read-only collector
125. [PX-118](tickets/PX-118-openbsdmetrics.md) — OpenBSD: add extended fields and meter families
126. [PX-119](tickets/PX-119-openbsdactions.md) — OpenBSD: add reviewed process actions and diagnostics
127. [PX-120](tickets/PX-120-openbsdgate.md) — OpenBSD: close the platform feature ledger and release gate
128. [PX-121](tickets/PX-121-dragonflybasic.md) — DragonFly BSD: implement the basic read-only collector
129. [PX-122](tickets/PX-122-dragonflymetrics.md) — DragonFly BSD: add extended fields and meter families
130. [PX-123](tickets/PX-123-dragonflyactions.md) — DragonFly BSD: add reviewed process actions and diagnostics
131. [PX-124](tickets/PX-124-dragonflygate.md) — DragonFly BSD: close the platform feature ledger and release gate
132. [PX-125](tickets/PX-125-solarisbasic.md) — Solaris: implement the basic read-only collector
133. [PX-126](tickets/PX-126-solarismetrics.md) — Solaris: add extended fields and meter families
134. [PX-127](tickets/PX-127-solarisactions.md) — Solaris: add reviewed process actions and diagnostics
135. [PX-128](tickets/PX-128-solarisgate.md) — Solaris: close the platform feature ledger and release gate
136. [PX-129](tickets/PX-129-illumosbasic.md) — illumos: implement the basic read-only collector
137. [PX-130](tickets/PX-130-illumosmetrics.md) — illumos: add extended fields and meter families
138. [PX-131](tickets/PX-131-illumosactions.md) — illumos: add reviewed process actions and diagnostics
139. [PX-132](tickets/PX-132-illumosgate.md) — illumos: close the platform feature ledger and release gate
140. [PX-133](tickets/PX-133-tuicore.md) — Build a minimal terminal-only SRUI replica client
141. [PX-134](tickets/PX-134-tuirender.md) — Render the explorer's read-only views in a terminal
142. [PX-135](tickets/PX-135-tuiinput.md) — Add terminal-client semantic editing and safe actions
143. [PX-136](tickets/PX-136-tuigate.md) — Close terminal-only deployment parity
144. [PX-137](tickets/PX-137-ipcspec.md) — Design a permission model for external local automation
145. [PX-138](tickets/PX-138-ipcread.md) — Expose reviewed, opt-in read-only semantic inspection
146. [PX-139](tickets/PX-139-ipcactions.md) — Expose scoped semantic actions to trusted local automation

## Ticket catalogue

### A. Start almost from zero

| Ticket | Task | Dependencies | Gate |
|---|---|---|---|
| [PX-000](tickets/PX-000-audit.md) | Establish the app baseline and a versioned htop feature ledger | Baseline audit | none |
| [PX-001](tickets/PX-001-shell.md) | Display an empty Process Explorer window over existing SRUI | PX-000 | none |
| [PX-002](tickets/PX-002-fake.md) | Populate the table with a deterministic fake process source | PX-001 | none |
| [PX-003](tickets/PX-003-real.md) | Read one real Linux process snapshot with instance identity | PX-002 | none |
| [PX-004](tickets/PX-004-live.md) | Refresh the process list with incremental transactions | PX-003 | none |
| [PX-005](tickets/PX-005-rss.md) | Add resident memory with accurate units and unavailable states | PX-004 | none |
| [PX-006](tickets/PX-006-cpu.md) | Add sampled per-process CPU usage | PX-005 | none |
| [PX-007](tickets/PX-007-summary.md) | Add the first system summary and data-freshness indicator | PX-006 | none |
| [PX-008](tickets/PX-008-firstgate.md) | Release gate: the minimal read-only monitor | PX-007 | R0 |

### B. Make the read-only monitor useful

| Ticket | Task | Dependencies | Gate |
|---|---|---|---|
| [PX-009](tickets/PX-009-sort.md) | Sort by a selected column without losing process identity | PX-008 | none |
| [PX-010](tickets/PX-010-filter.md) | Add a responsive command filter | PX-009 | none |
| [PX-010-G01](tickets/PX-010-G01-early-scale.md) | Gate early live collection scale, progress, and resource budgets | PX-010, PX-042 | R1-prerequisite |
| [PX-011](tickets/PX-011-search.md) | Add find-next/find-previous and jump to PID | PX-010-G01 | none |
| [PX-012](tickets/PX-012-users.md) | Filter by owner and an explicit PID set | PX-011 | none |
| [PX-013](tickets/PX-013-inspector.md) | Add a selected-process inspector | PX-012 | none |
| [PX-014](tickets/PX-014-command.md) | Show full command line and executable context | PX-013 | none |
| [PX-015](tickets/PX-015-pause.md) | Add sampling interval and a clearly labeled paused view | PX-014 | none |
| [PX-016](tickets/PX-016-keyboard.md) | Make the basic workflow keyboard accessible | PX-015 | none |
| [PX-017](tickets/PX-017-follow.md) | Add follow mode and stable viewport behavior | PX-016 | none |
| [PX-018](tickets/PX-018-tree.md) | Add parent/child process tree navigation | PX-017 | none |
| [PX-019](tickets/PX-019-threads.md) | Expose user threads and kernel-thread visibility | PX-018 | none |
| [PX-020](tickets/PX-020-tags.md) | Add stable multi-selection and descendant tagging | PX-019 | none |
| [PX-021](tickets/PX-021-lastseen.md) | Keep a last-observed record when a process disappears | PX-020 | none |

### C. Prove the SRUI advantages

| Ticket | Task | Dependencies | Gate |
|---|---|---|---|
| [PX-022](tickets/PX-022-reconnect.md) | Expose disconnect, catch-up, and replacement correctly | PX-021 | none |
| [PX-023](tickets/PX-023-connect.md) | Add a minimal generic host connection form | PX-022 | none |
| [PX-024](tickets/PX-024-saved.md) | Save endpoints and safely reattach after client relaunch | PX-023 | none |
| [PX-025](tickets/PX-025-demo.md) | Add bounded demonstration workloads and latency scenarios | PX-024 | none |
| [PX-026](tickets/PX-026-automation.md) | Demonstrate semantic inspection without external IPC | PX-025 | none |
| [PX-026-G01](tickets/PX-026-G01-r1-package.md) | Gate installable R1 with compatibility, privacy, and native verification | PX-026, PX-068, PX-010-G01 | R1-prerequisite |
| [PX-027](tickets/PX-027-showcasegate.md) | Release gate: the first polished SRUI showcase | PX-026, PX-026-G01, PX-068 | R1 |

### D. Add safe process controls

| Ticket | Task | Dependencies | Gate |
|---|---|---|---|
| [PX-028](tickets/PX-028-policy.md) | Define action authorization and a dry-run action contract | PX-027 | none |
| [PX-029](tickets/PX-029-handles.md) | Resolve stable Linux process handles before enabling actions | PX-028 | none |
| [PX-030](tickets/PX-030-terminate.md) | Add confirmed graceful termination of one process | PX-029 | none |
| [PX-031](tickets/PX-031-outcomes.md) | Distinguish action acknowledgement from observed process outcome | PX-030 | none |
| [PX-032](tickets/PX-032-stopresume.md) | Add explicit Stop and Continue actions | PX-031 | none |
| [PX-033](tickets/PX-033-force.md) | Add explicit force termination without automatic escalation | PX-032 | none |
| [PX-034](tickets/PX-034-signals.md) | Add an advanced signal chooser | PX-033 | none |
| [PX-035](tickets/PX-035-bulk.md) | Apply actions to a frozen tagged target set | PX-034 | none |
| [PX-035-G01](tickets/PX-035-G01-action-feasibility.md) | Resolve identity-safe targeting feasibility for advanced controls | PX-029, PX-031 | Advanced-controls-prerequisite |
| [PX-036](tickets/PX-036-nice.md) | Read and change nice values with operation-specific safety | PX-035, PX-035-G01 | none |
| [PX-037](tickets/PX-037-affinity.md) | Inspect and edit CPU affinity | PX-036 | none |
| [PX-038](tickets/PX-038-ioprio.md) | Inspect and edit I/O scheduling priority | PX-037 | none |
| [PX-039](tickets/PX-039-scheduling.md) | Inspect and edit scheduler policy safely | PX-038 | none |
| [PX-040](tickets/PX-040-autogroup.md) | Expose autogroup identity and priority changes | PX-039 | none |
| [PX-041](tickets/PX-041-controlgate.md) | Release gate: an everyday process explorer with safe controls | PX-040 | R2 |

### E. Expand monitoring depth

| Ticket | Task | Dependencies | Gate |
|---|---|---|---|
| [PX-042](tickets/PX-042-metricregistry.md) | Introduce a typed metric catalogue and collection-cost policy | PX-008 | none |
| [PX-043](tickets/PX-043-processfields.md) | Add process identity, timing, and scheduling columns | PX-042 | none |
| [PX-044](tickets/PX-044-faults.md) | Add faults and context-switch accounting | PX-043 | none |
| [PX-045](tickets/PX-045-memoryfields.md) | Add detailed low-cost memory columns | PX-044 | none |
| [PX-046](tickets/PX-046-pss.md) | Add on-demand proportional memory accounting | PX-045 | none |
| [PX-047](tickets/PX-047-processio.md) | Add per-process I/O counters and rates | PX-046 | none |
| [PX-048](tickets/PX-048-cgroups.md) | Expose cgroups, namespaces, and container visibility | PX-047 | none |
| [PX-049](tickets/PX-049-securityfields.md) | Expose OOM, elevated privileges, and security context | PX-048 | none |
| [PX-050](tickets/PX-050-staleexe.md) | Show command variants and replaced executable/library warnings | PX-049 | none |
| [PX-051](tickets/PX-051-delayacct.md) | Add optional delay-accounting metrics | PX-050 | none |
| [PX-052](tickets/PX-052-cpudetail.md) | Add per-CPU usage classes and topology | PX-051 | none |
| [PX-053](tickets/PX-053-sensors.md) | Add CPU frequency and temperature readings | PX-052 | none |
| [PX-054](tickets/PX-054-hostmemory.md) | Add detailed host memory, swap, and huge-page statistics | PX-042, PX-007 | none |
| [PX-055](tickets/PX-055-disk.md) | Add host and per-device disk activity | PX-042, PX-007 | none |
| [PX-056](tickets/PX-056-network.md) | Add host and per-interface network activity | PX-042, PX-007 | none |
| [PX-057](tickets/PX-057-psi.md) | Add pressure-stall indicators | PX-056 | none |
| [PX-058](tickets/PX-058-hostextras.md) | Add host identity, task, descriptor, clock, and uptime meter variants | PX-057 | none |
| [PX-059](tickets/PX-059-battery.md) | Add optional battery and power-source meters | PX-058 | none |

### F. Cover optional Linux facilities

| Ticket | Task | Dependencies | Gate |
|---|---|---|---|
| [PX-060](tickets/PX-060-gpu.md) | Add optional host GPU monitoring | PX-042 | none |
| [PX-061](tickets/PX-061-processgpu.md) | Add supported per-process GPU accounting | PX-060 | none |
| [PX-062](tickets/PX-062-zram.md) | Add zram meters | PX-042 | none |
| [PX-063](tickets/PX-063-zswap.md) | Add zswap meters | PX-042 | none |
| [PX-064](tickets/PX-064-zfs.md) | Add ZFS ARC and compressed-ARC meters | PX-042 | none |
| [PX-065](tickets/PX-065-systemd.md) | Add read-only systemd status and service-count meters | PX-042 | none |
| [PX-066](tickets/PX-066-openrc.md) | Add read-only OpenRC status meters | PX-042 | none |
| [PX-067](tickets/PX-067-selinux.md) | Add SELinux state and complete optional-meter registration | PX-042, PX-060, PX-061, PX-062, PX-063, PX-064, PX-065, PX-066 | none |

### G. Reach customization and visualization parity

| Ticket | Task | Dependencies | Gate |
|---|---|---|---|
| [PX-068](tickets/PX-068-columns.md) | Let users choose and reorder process columns | PX-042, PX-009 | none |
| [PX-069](tickets/PX-069-screens.md) | Add named saved views and a versioned settings store | PX-068 | none |
| [PX-070](tickets/PX-070-meterlayout.md) | Configure meter layout and non-graph display modes | PX-069 | none |
| [PX-070-G01](tickets/PX-070-G01-dashboard-design.md) | Specify the btop-inspired native dashboard and evaluation scope | PX-031, PX-070 | Dashboard-design |
| [PX-071](tickets/PX-071-metricprofile.md) | Specify a small retained metrics-visualization profile | PX-070 | none |
| [PX-072](tickets/PX-072-metricserver.md) | Publish retained metric-series data from the server | PX-071 | none |
| [PX-073](tickets/PX-073-metricclient.md) | Render native retained gauges and history graphs | PX-072 | none |
| [PX-073-G01](tickets/PX-073-G01-dashboard-compose.md) | Compose the native host dashboard and process-history drill-down | PX-070-G01, PX-073, PX-054, PX-055, PX-056, PX-090 | none |
| [PX-073-G02](tickets/PX-073-G02-dashboard-gate.md) | Release gate for the measured native dashboard | PX-073-G01, PX-095, PX-010-G01, PX-026-G01 | R-Dashboard |
| [PX-074](tickets/PX-074-displayprefs.md) | Add display preferences and remaining navigation modes | PX-073 | none |
| [PX-075](tickets/PX-075-startup.md) | Add validated startup options and configuration import | PX-074 | none |

### H. Add advanced inspection

| Ticket | Task | Dependencies | Gate |
|---|---|---|---|
| [PX-076](tickets/PX-076-runner.md) | Add a bounded server-side diagnostic job runner | PX-075, PX-031 | none |
| [PX-077](tickets/PX-077-fds.md) | Inspect open descriptors and file information | PX-076 | none |
| [PX-078](tickets/PX-078-locks.md) | Inspect active file locks | PX-077 | none |
| [PX-079](tickets/PX-079-env.md) | Add opt-in environment inspection with privacy controls | PX-078 | none |
| [PX-080](tickets/PX-080-maps.md) | Inspect memory mappings and related details | PX-079 | none |
| [PX-081](tickets/PX-081-trace.md) | Launch and stop a process system-call trace | PX-080 | none |
| [PX-082](tickets/PX-082-traceview.md) | Render bounded trace output and add an optional terminal island | PX-081 | none |
| [PX-083](tickets/PX-083-backtrace.md) | Collect and display opt-in process/thread backtraces | PX-082 | none |

### I. Prove scale, safety, and Linux coverage

| Ticket | Task | Dependencies | Gate |
|---|---|---|---|
| [PX-084](tickets/PX-084-virtualization.md) | Make live sorted and filtered collections range-safe | PX-010-G01, PX-021 | none |
| [PX-085](tickets/PX-085-budgets.md) | Enforce application memory, history, collection, and traffic budgets | PX-084, PX-076, PX-072, PX-059, PX-067 | none |
| [PX-086](tickets/PX-086-chaos.md) | Run application-specific reconnect and stale-intent fault tests | PX-085, PX-041, PX-024, PX-083 | none |
| [PX-087](tickets/PX-087-privacy.md) | Audit sensitive data, parsing, and accessibility end to end | PX-086, PX-083 | none |
| [PX-088](tickets/PX-088-parity.md) | Close the pinned Linux htop feature ledger | PX-087, PX-041, PX-059, PX-067, PX-075 | R3 |
| [PX-089](tickets/PX-089-package.md) | Package and document the verified Linux/macOS release | PX-088 | R3-release |

### J. Go beyond the baseline

| Ticket | Task | Dependencies | Gate |
|---|---|---|---|
| [PX-090](tickets/PX-090-history.md) | Retain a short, explicitly sampled process history | PX-027, PX-072, PX-021, PX-010-G01 | none |
| [PX-091](tickets/PX-091-historicalview.md) | Inspect historical samples without changing live authority | PX-090 | none |
| [PX-092](tickets/PX-092-snapshotdiff.md) | Compare two recorded samples | PX-091 | none |
| [PX-093](tickets/PX-093-alerts.md) | Add user-defined threshold observations without automatic remediation | PX-092 | none |
| [PX-094](tickets/PX-094-export.md) | Export an explicitly selected diagnostic snapshot | PX-093 | none |
| [PX-095](tickets/PX-095-multihost.md) | Add multiple isolated host windows | PX-027, PX-031 | none |
| [PX-096](tickets/PX-096-guided.md) | Add evidence-based explanations and a guided demo | PX-095, PX-094 | none |
| [PX-097](tickets/PX-097-beyondgate.md) | Release gate: demonstrate concrete advantages beyond parity | PX-096, PX-073-G02 | R4 |

### K. Administrative parity without a root daemon

| Ticket | Task | Dependencies | Gate |
|---|---|---|---|
| [PX-098](tickets/PX-098-adminspec.md) | Design an optional narrowly privileged remote helper | PX-028, PX-029, PX-087, PX-088 | SECURITY-REVIEW |
| [PX-099](tickets/PX-099-admintransport.md) | Implement the reviewed helper boundary without process-changing operations | PX-098 | none |
| [PX-100](tickets/PX-100-adminops.md) | Add privileged operations one reviewed operation at a time | PX-099 | ADMIN-per-operation |

### L. PCP and declarative dynamic metrics

| Ticket | Task | Dependencies | Gate |
|---|---|---|---|
| [PX-101](tickets/PX-101-pcpmetadata.md) | Add a read-only PCP metric catalogue adapter | PX-042, PX-088 | none |
| [PX-102](tickets/PX-102-pcpsamples.md) | Fetch and display bounded PCP metric instances | PX-101, PX-072, PX-073 | none |
| [PX-103](tickets/PX-103-pcpscreens.md) | Add declarative dynamic metric views and PCP parity closure | PX-102, PX-069 | PCP-parity |

### M. Monitored-host platform parity

| Ticket | Task | Dependencies | Gate |
|---|---|---|---|
| [PX-104](tickets/PX-104-portcontract.md) | Freeze the portable collector/action conformance contract | PX-089, PX-028, PX-042 | none |
| [PX-105](tickets/PX-105-darwinbasic.md) | macOS/Darwin: implement the basic read-only collector | PX-104 | none |
| [PX-106](tickets/PX-106-darwinmetrics.md) | macOS/Darwin: add extended fields and meter families | PX-105 | none |
| [PX-107](tickets/PX-107-darwinactions.md) | macOS/Darwin: add reviewed process actions and diagnostics | PX-105, PX-028, PX-031, PX-076 | none |
| [PX-108](tickets/PX-108-darwingate.md) | macOS/Darwin: close the platform feature ledger and release gate | PX-106, PX-107, PX-086 | DARWIN-parity |
| [PX-109](tickets/PX-109-freebsdbasic.md) | FreeBSD: implement the basic read-only collector | PX-104 | none |
| [PX-110](tickets/PX-110-freebsdmetrics.md) | FreeBSD: add extended fields and meter families | PX-109 | none |
| [PX-111](tickets/PX-111-freebsdactions.md) | FreeBSD: add reviewed process actions and diagnostics | PX-109, PX-028, PX-031, PX-076 | none |
| [PX-112](tickets/PX-112-freebsdgate.md) | FreeBSD: close the platform feature ledger and release gate | PX-110, PX-111, PX-086 | FREEBSD-parity |
| [PX-113](tickets/PX-113-netbsdbasic.md) | NetBSD: implement the basic read-only collector | PX-104 | none |
| [PX-114](tickets/PX-114-netbsdmetrics.md) | NetBSD: add extended fields and meter families | PX-113 | none |
| [PX-115](tickets/PX-115-netbsdactions.md) | NetBSD: add reviewed process actions and diagnostics | PX-113, PX-028, PX-031, PX-076 | none |
| [PX-116](tickets/PX-116-netbsdgate.md) | NetBSD: close the platform feature ledger and release gate | PX-114, PX-115, PX-086 | NETBSD-parity |
| [PX-117](tickets/PX-117-openbsdbasic.md) | OpenBSD: implement the basic read-only collector | PX-104 | none |
| [PX-118](tickets/PX-118-openbsdmetrics.md) | OpenBSD: add extended fields and meter families | PX-117 | none |
| [PX-119](tickets/PX-119-openbsdactions.md) | OpenBSD: add reviewed process actions and diagnostics | PX-117, PX-028, PX-031, PX-076 | none |
| [PX-120](tickets/PX-120-openbsdgate.md) | OpenBSD: close the platform feature ledger and release gate | PX-118, PX-119, PX-086 | OPENBSD-parity |
| [PX-121](tickets/PX-121-dragonflybasic.md) | DragonFly BSD: implement the basic read-only collector | PX-104 | none |
| [PX-122](tickets/PX-122-dragonflymetrics.md) | DragonFly BSD: add extended fields and meter families | PX-121 | none |
| [PX-123](tickets/PX-123-dragonflyactions.md) | DragonFly BSD: add reviewed process actions and diagnostics | PX-121, PX-028, PX-031, PX-076 | none |
| [PX-124](tickets/PX-124-dragonflygate.md) | DragonFly BSD: close the platform feature ledger and release gate | PX-122, PX-123, PX-086 | DRAGONFLY-parity |
| [PX-125](tickets/PX-125-solarisbasic.md) | Solaris: implement the basic read-only collector | PX-104 | none |
| [PX-126](tickets/PX-126-solarismetrics.md) | Solaris: add extended fields and meter families | PX-125 | none |
| [PX-127](tickets/PX-127-solarisactions.md) | Solaris: add reviewed process actions and diagnostics | PX-125, PX-028, PX-031, PX-076 | none |
| [PX-128](tickets/PX-128-solarisgate.md) | Solaris: close the platform feature ledger and release gate | PX-126, PX-127, PX-086 | SOLARIS-parity |
| [PX-129](tickets/PX-129-illumosbasic.md) | illumos: implement the basic read-only collector | PX-104 | none |
| [PX-130](tickets/PX-130-illumosmetrics.md) | illumos: add extended fields and meter families | PX-129 | none |
| [PX-131](tickets/PX-131-illumosactions.md) | illumos: add reviewed process actions and diagnostics | PX-129, PX-028, PX-031, PX-076 | none |
| [PX-132](tickets/PX-132-illumosgate.md) | illumos: close the platform feature ledger and release gate | PX-130, PX-131, PX-086 | ILLUMOS-parity |

### N. Terminal-only deployment

| Ticket | Task | Dependencies | Gate |
|---|---|---|---|
| [PX-133](tickets/PX-133-tuicore.md) | Build a minimal terminal-only SRUI replica client | PX-104, PX-071 | none |
| [PX-134](tickets/PX-134-tuirender.md) | Render the explorer's read-only views in a terminal | PX-133 | none |
| [PX-135](tickets/PX-135-tuiinput.md) | Add terminal-client semantic editing and safe actions | PX-134, PX-086 | none |
| [PX-136](tickets/PX-136-tuigate.md) | Close terminal-only deployment parity | PX-135, PX-075 | TUI-parity |

### O. Permission-gated external automation

| Ticket | Task | Dependencies | Gate |
|---|---|---|---|
| [PX-137](tickets/PX-137-ipcspec.md) | Design a permission model for external local automation | PX-026, PX-087, PX-095 | IPC-REVIEW |
| [PX-138](tickets/PX-138-ipcread.md) | Expose reviewed, opt-in read-only semantic inspection | PX-137 | none |
| [PX-139](tickets/PX-139-ipcactions.md) | Expose scoped semantic actions to trusted local automation | PX-138, PX-028, PX-086 | AUTOMATION |
