# Native host dashboard: specification and evaluation protocol (PX-070-G01)

This is the Dashboard-design gate's specification, written before any dashboard code exists. It is read by PX-070-G02, PX-071–PX-073, PX-054–PX-056, PX-090, PX-073-G01 (composition) and PX-073-G02 (the R-Dashboard release gate). It is not product code and changes no behavior. Design citations are to D1, `SRUI_Semantic_Remote_UI_Design_v0.6.md` (read-only); repository citations are to the checkout at `de1edd69` (`origin/main`, PX-070 merged).

## 1. Scope and non-commitments

- **What this is.** A native SRUI dashboard for srtop: four host panels (CPU aggregate, memory/swap, disk, network) beside the existing searchable process table and inspector, plus the inspector's explicitly sampled process history. It is inspired by selected btop++ workflows (section 3).
- **What this is not.** No btop parity commitment, at any version. No copied btop code, themes, glyph sets, screenshots or other assets. No claim that the dashboard is better than btop or htop in general; the evaluation (section 11) records specific, matched observations only.
- **Baselines.** htop 3.5.3 stays the process-management baseline (`task-index.json` `htop_baseline`, SOURCES H1–H8). btop is the inspiration for the host-activity workflows only.
- **R1 and safe termination do not depend on this dashboard.** Neither PX-027 (`delivery_milestones.R1`) nor PX-031 (`delivery_milestones.safe_termination`) has any dashboard ticket (PX-070-G01, PX-070-G02, PX-071–PX-073, PX-073-G01, PX-073-G02) in its transitive `depends_on` closure; this was checked mechanically on the plan (completion note, "R1 independence"). The dashboard adds no action, no authorization path and no change to the R1 package. Nothing in the dashboard may become an R1 prerequisite without a reviewed plan amendment.
- **Later panels.** Detailed per-CPU (PX-052), CPU frequency/temperature (PX-053) and GPU (PX-060, PX-061) panels arrive later, each with its own verified collector and live evidence. Their absence, or missing hardware, never suppresses or delays the four core panels.

## 2. Reference versions (BT1, H1)

| Item | Pinned value | How it was verified (27 November 2026) |
| --- | --- | --- |
| btop++ release | tag `v1.4.8` ("btop++ (v1.4.8) Bug fixes"), published 2026-10-25T08:02:10Z, not a draft or prerelease | `https://api.github.com/repos/aristocratos/btop/releases/latest` returned `tag_name: v1.4.8` |
| btop++ commit | `cf1da5900b1e9eaa1c20fd7148dd812b43340b8a` ("v1.4.8 Bug fixes"); the annotated tag object is `842fa7fbeabe4704b56d66e7aa31d173e628abec` | `git ls-remote --tags https://github.com/aristocratos/btop` listed `842fa7fb… refs/tags/v1.4.8` and `cf1da590… refs/tags/v1.4.8^{}`; `https://api.github.com/repos/aristocratos/btop/commits/v1.4.8` returned the same SHA, and its diff sets `Version = "1.4.8"` in `src/btop.cpp` |
| btop++ license | Apache License 2.0 (`Apache-2.0`), `LICENSE` blob `d645695673349e3947e8e5ae42332d0ac3164cd7` | `https://api.github.com/repos/aristocratos/btop/license` |
| btop++ evaluation binaries | release assets `btop-aarch64-unknown-linux-musl.tar.gz` (`sha256:95328e26ab08f42033af26628e2d169206a5a420f35669ce17459ef85114c14c`) and `btop-x86_64-unknown-linux-musl.tar.gz` (`sha256:af589cf82cc9ea9c29ce050b9239a13bef2d1492256d476156e1054a42ce89ad`); statically linked with musl, no GPU support | the same release JSON (`assets[].digest`) |
| htop | 3.5.3 (unchanged) | SOURCES H1; not re-checked here |

The btop facts this specification relies on were read from the pinned commit's `README.md` (`https://raw.githubusercontent.com/aristocratos/btop/cf1da5900b1e9eaa1c20fd7148dd812b43340b8a/README.md`): the description "shows usage and stats for processor, memory, disks, network and processes"; the default `shown_boxes = "cpu mem net proc"`; the default `update_ms = 2000` ("recommended 2000 ms or above for better sample times for graphs"); `show_disks = true` inside the memory box; `net_auto = true` auto-scaling network graphs; process filter, sort, tree view, pause, a detailed view of the selected process and signals to it. Nothing was cloned, downloaded into the repository, or copied. Apache-2.0 matters only if btop material were redistributed; this plan redistributes none, and the evaluation (section 11) downloads the pinned binary into the disposable devbox at run time and checks its digest.

Rebaselining to a newer btop is an explicit plan change: record the new tag, commit, license and digests here and rerun the evaluation; never compare against "latest".

## 3. Selected workflows

| ID | Workflow (btop's equivalent) | What the dashboard provides |
| --- | --- | --- |
| W1 | At-a-glance host activity (btop's cpu/mem/net boxes) | Four panels with a current value in words and, where the client negotiates the metrics profile, a bounded recent-history graph; each panel names its unit, interval and sample time |
| W2 | Process discovery (btop's proc box: filter, sort, select, detailed view) | The existing srtop table: CPU sort (PX-009), filter (PX-010), search (PX-011), selection and the inspector (PX-013) beside the panels, unchanged in authority and identity |
| W3 | Sampled-trend inspection (btop's graphs and the selected-process view) | Host series in the panels (PX-072/PX-073) and, for the selected process, its explicitly sampled history in the inspector (PX-090), with a labelled recording start and visible gaps |
| W4 | Recovery after disconnect (btop has none: an SSH drop ends the btop process) | SRUI continuity (§§17–18): a stale-labelled window keeps the last applied revision and resumes or resynchronizes; host and process history collected while detached is kept server-side (PX-090) |

Not adopted from btop: themes and colour gradients (appearance is client-local, §7.1), braille/block glyph graphs (the renderer draws natively, PX-073), mouse-driven option menus, the preset string format, per-row process CPU graphs, battery, the signal menu (srtop's only process-changing path stays PX-028–PX-031's confirmed, identity-bound flow), setuid/setcap collection, and any btop configuration file.

## 4. Dashboard composition and layout

One Surface per session window (PX-095 isolates hosts). Inside it, from top to bottom:

1. **Panel region**: the four core panels in a bounded number of semantic groups (at most 2, the PX-070 rule), default order CPU, Memory/swap, Disk, Network.
2. **Process region**: the existing filter/search fields, process table, inspector and action panel, unchanged.

The two regions are panes of one standard `Split` (stacked axis), so the user can trade panel space for table rows by dragging the divider. `Split` is a §7.3 SHOULD-tier standard widget that this checkout does not build or render; PX-070-G02 adds it (section 10). The divider position is local interaction state (§7.6, §22.5): never sent, never saved by the server.

**Panel presentations** (a saved semantic choice per panel, never geometry):

- **Compact**: one line per figure in words plus a native bar where the figure is a share. For CPU and Memory/swap this is exactly the PX-070 meter (`apps/srtop/src/meters.rs`, `apps/srtop/src/summary.rs`), reused, not duplicated: the same nodes, the same `Sample`, the same never-zero rules. Disk and Network compact forms are one line each for the aggregate read/write and receive/transmit rates.
- **Expanded**: compact content plus one series graph node (PX-073) and its fallback, and, for Disk and Network, a model-backed device/interface `Table` (§8) with at most `MAX_DASHBOARD_DEVICE_ROWS` rows; at most `MAX_EXPANDED_DEVICES` devices or interfaces may additionally show their own series. A panel's aggregate series and every device series it shows are carried by that one graph node (a PX-071 requirement, section 5), so showing a device's series adds series data, never nodes.
- **Hidden**: the panel is moved into a collapsed container (the PX-070 `METER_HIDDEN` pattern), so no node is created or deleted after the shell and node IDs are never reused (§6.2).

**Default layout.** All four panels shown, CPU and Memory/swap expanded, Disk and Network compact, no per-device expansion. Load, uptime and the process count stay PX-070 meters. A client without the metrics profile shows the Standard Widget fallback (section 5) in every presentation.

**Bounded panel configuration.** Per panel: Show/Hide, Move earlier, Move later, Compact/Expanded; per device row in an expanded panel: Show series/Hide series (refused past `MAX_EXPANDED_DEVICES`); plus Restore default panels. All are standard `Button`s with `ACTIVATE` only; a control that would change nothing is disabled and refused by event validation (the PX-070 rule). A change is one transaction of `MOVE_NODE`s and visibility/label properties; a sample is scalar `SET_PROPERTY` plus series appends only and never structural (§12.1; PX-070 per-sample rule). Visibility, order, presentation and device expansion are saved in saved views (PX-069) through a settings format version bump owned by PX-073-G01, with migration from version 3; window size, divider position, colours and theme are never stored or sent.

**Local resizing.** Window resize, divider drag, graph reflow, axis density and reduced motion are client-local (§10, §22.5; PX-073). Resizing sends no event and changes no sample traffic (§12.2, §31.3). A narrow window wraps or scrolls the panel groups exactly as the generic client already does for PX-070's meter groups.

**Keyboard focus.** The AppKit key-view loop reaches, in order: filter, search, process table, inspector controls, action controls, then each panel's controls in panel order and each expanded device `Table`. Focus shortcuts use only PX-016-G01's admitted types and grammar (`primary+` required for printable keys): at least one for the process table and one per expanded device table; their exact keys are PX-073-G01's choice and must be listed in its completion note. Graph keyboard reachability is PX-073's ("VoiceOver or the accessibility API can obtain units, current values, series names, and unavailable state"); if a graph is not focusable, its numeric fallback is.

**Host and account labels.** PX-073-G01 depends on PX-095 for them. The trusted host/account/session-incarnation label is PX-095's client-owned window chrome (`client-macos/RendererAppKit/SessionWindowIdentity.swift`, set by `ConnectionManager` through `setSessionWindowIdentity`); no server text may imitate it. Server text names only the collection source (the PX-007 freshness line already does) and each panel's scope ("host", a device or interface name).

**Disconnected presentation.** Transport state is PX-022-G01's client-owned subtitle (`connecting|disconnected|resynchronizing — showing data from revision N (stale)`, `session replaced — rebuilding`); a server cannot publish while disconnected, so panels never claim to be live or disconnected themselves. While stale the window keeps the last applied revision: values, sample times and graphs stay as applied, nothing is extrapolated, and user selection changes are refused locally (PX-022-G02). After `RESUME_OK` the replayed commits bring the panels forward; after a snapshot resynchronization the panels and series are rebuilt from the snapshot. A `REPLACED` session is a new recording: PX-095 labels the new incarnation and history restarts (PX-090).

**Paused presentation.** PX-015's Pause view stops collection server-side; every panel's freshness line says `paused — collection stopped at <time>`, no series receives a sample while paused, and the first sample after Resume follows a recorded gap (never an interpolated or zero sample).

## 5. Rules common to every panel

- **Units.** Shares are percentages with one decimal and an explicit denominator ("of 8 logical CPUs", "of total memory"). Sizes are IEC bytes (`KiB`, `MiB`, `GiB`, one decimal), formatted server-side by the existing `metric.rs` formatters. Rates are IEC bytes per second (`KiB/s`); packet rates are `packets/s`. Bits per second are not used, so btop's network bit rates must be normalized in the evaluation (section 11.3).
- **Sample interval.** Host panels sample on srtop's one refresh loop (`apps/srtop/src/sampling.rs`; `DEFAULT_REFRESH_INTERVAL` 1 s, `MIN_REFRESH_INTERVAL` 50 ms, `MAX_REFRESH_INTERVAL` 300 s in `apps/srtop/src/refresh.rs`). PX-042's collection-cost policy may give a panel a longer interval, never a shorter one. Each panel's freshness line states its interval.
- **Independent timestamps.** Every panel carries its own last-successful-sample time. CPU and Memory/swap come from the same scan as the process table (`Sample::of`, `summary.rs`) and therefore share that scan's time; Disk and Network have their own reads and their own times. No panel, row or graph implies an atomic host snapshot across panels (PX-071).
- **States, never zero.** A figure the collector could not state is never shown as `0` in text, bar or graph: `warming up` (a rate needs a second sample, or a counter reset or device reappearance started a new baseline), `unavailable` with the reason, `denied` naming the file and errno class, `not provided` by this source, `none configured` (no swap, no disks, no non-loopback interface), `collector error` (the last good sample is kept and the line is in the warning role), `stale` (server-side: no successful sample for 3 intervals, detected by the refresh loop's own timer, the PX-031 deadline pattern), `paused`. The PX-007/PX-070 tests already enforce this for CPU, memory and swap (`unavailable_and_stale_figures_never_become_zero_in_any_presentation`, `apps/srtop/tests/meters_test.rs`); PX-073-G01 extends them to every panel and presentation.
- **One graph node per panel (PX-071 requirement).** The PX-071 profile must let one graph node carry several series, at least `MAX_SERIES_PER_GRAPH` = 18 (the Disk and Network maximum: 2 aggregate + 2 × 8 devices), each with its own stable series identity (host, device or interface, direction), unit and gaps, and must define that node's Standard Widget fallback as the single bounded `Table` above. PX-071 now depends on PX-070-G01 so it reads this requirement. Node counts in section 6 therefore do not grow with device expansion.
- **Gaps.** A missing sample is a gap in the series and in its numeric fallback, labelled with its start and duration; graphs never interpolate across it (PX-072, PX-073).
- **Accessible alternative.** Each panel has a heading `Text` naming it; its current values are complete sentences in `Text` (unit, denominator, scope, sample time and interval), readable alone; and its series history has a nonvisual numeric alternative: the PX-071 Standard Widget fallback, one model-backed `Table` per graph node whose rows are the most recent `MAX_FALLBACK_SAMPLES` (60) samples of every series the graph shows, aggregate and per-device alike, each row naming the series (device or interface and direction), the sample time, and the value with its unit or the gap and its duration, at most `MAX_FALLBACK_ROW_BYTES` (64) bytes on the wire per row. The device table gives current rates; the fallback table gives the history, so a per-device series is never graph-only. Every evaluation task (section 11) must be completable with graphs absent. Whether a labelled `Row`/`Column` is exposed as a native accessibility group was not verified in this checkout (`ControlFactory.apply(property:value:to:store:)` sets the label on the `NSStackView`, which AppKit may not expose as an element); the per-panel sentences do not depend on grouping, and PX-073-G01 must verify grouping on a real window and append a renderer prerequisite if it is missing.
- **Scope honesty.** Host disk and network activity never names a process as responsible; per-process I/O is PX-047's and per-process network attribution is out of scope (PX-073-G01 out of scope).

## 6. Panel specifications

Budget constants are maxima PX-073-G01 must define as named constants and assert in tests at and past each bound. They may be lowered by measurement; raising one needs an amendment to this document recorded before the rerun.

### 6.1 CPU aggregate

| Aspect | Specification |
| --- | --- |
| Fields | Busy share of all logical CPUs over the last interval, from `/proc/stat` busy (user, nice, system, irq, softirq, steal) over busy + idle (idle, iowait) ticks; the logical CPU count named. Guest ticks are not added (already inside user/nice, PX-052 records the formula) |
| Collector owner | Existing: `apps/srtop/src/procfs.rs` (`/proc/stat`, `SystemCpuInterval` in `apps/srtop/src/source.rs`), PX-006/PX-007; per-CPU classes later PX-052 |
| Renderer owner | Compact: existing `Text` + `Progress` (PX-070 meter). Expanded graph: PX-073 over the PX-071 profile, series published by PX-072. Layout: existing `Row`/`Column`, `Split` from PX-070-G02 |
| Units / freshness | `%` of N logical CPUs, one decimal; shares the scan's sample time; `warming up` until two scans; `stale` after 3 intervals without a scan |
| Budget | ≤ 12 nodes (including the PX-070 meter's nodes, the graph node and its fallback `Table`); 1 series; ≤ `MAX_SERIES_SAMPLES` (900) samples; fallback ≤ 60 rows; each line ≤ 512 bytes (`MAX_SUMMARY_TEXT_BYTES`) |
| Accessible fallback | `CPU: 37.4% busy of 8 logical CPUs, sampled 14:02:07, every 1 s` plus the fallback sample table |

### 6.2 Memory and swap

| Aspect | Specification |
| --- | --- |
| Fields | Memory used = `MemTotal − MemAvailable` of `MemTotal`; swap used = `SwapTotal − SwapFree` of `SwapTotal` (the PX-007 definitions in `summary.rs`). Cache/buffer/shared breakdown and huge pages arrive with PX-054 under its overlap rules; the panel never shows segments that imply an invalid sum |
| Collector owner | Existing: `procfs.rs` `meminfo` (`MemoryFigures`, `SwapFigures`), PX-007; breakdown PX-054 |
| Renderer owner | Compact: existing PX-070 meters (memory and swap bars). Expanded: PX-073 graph of the two series; breakdown as standard `Text`/`Progress` or a small `Table` (PX-054) |
| Units / freshness | IEC bytes and `%` of total; shares the scan's time; `none configured` for no swap (never 0% of 0); `available` is labelled as available, not free (PX-054 verify) |
| Budget | ≤ 16 nodes (one graph node and one fallback `Table` for both series); 2 series; ≤ 900 samples each; fallback ≤ 120 rows; lines ≤ 512 bytes |
| Accessible fallback | `Memory: 5.2 GiB used of 15.5 GiB (33.5% of total)` and `Swap: none configured`, plus the fallback sample table |

### 6.3 Disk

| Aspect | Specification |
| --- | --- |
| Fields | Aggregate read and write bytes/s; per device read/write bytes/s and operations/s, plus busy share where the source provides it. Aggregation excludes partitions and stacked devices so nothing is double counted; the rule is PX-055's and is stated in the panel's scope line |
| Collector owner | **PX-055** (none exists in this checkout: no `diskstats` reader in `apps/srtop/src/`) |
| Renderer owner | Compact: existing `Text`. Expanded: model-backed `Table` (existing, §8, virtualized) for devices, and one PX-073 graph node carrying the aggregate and ≤ `MAX_EXPANDED_DEVICES` devices' series |
| Units / freshness | IEC bytes/s and `ops/s`; own sample time; `warming up` on the first sample, on a counter decrease and on device (re)appearance; a device that disappears releases its series and baseline |
| Budget | ≤ 16 nodes whatever the expansion (one graph node, one fallback `Table`, one device `Table`); device table ≤ `MAX_DASHBOARD_DEVICE_ROWS` (64) rows, each ≤ 512 bytes, with a `N more devices not shown` line beyond; series: 2 aggregate + 2 × `MAX_EXPANDED_DEVICES` (8) = 18 in the one graph node; ≤ 900 samples each; fallback ≤ 18 × 60 = 1,080 rows |
| Accessible fallback | `Disk (host, physical devices): read 12.0 MiB/s, write 3.4 MiB/s, sampled 14:02:07, every 1 s`; the device table for current per-device rates; the fallback sample table for the history of the aggregate and of every expanded device |

### 6.4 Network

| Aspect | Specification |
| --- | --- |
| Fields | Aggregate receive and transmit bytes/s and packets/s; per interface the same. The aggregate excludes loopback and states how virtual interfaces are treated (PX-056's rule); counters are passive reads only |
| Collector owner | **PX-056** (none exists in this checkout: no `/proc/net/dev` reader in `apps/srtop/src/`) |
| Renderer owner | As Disk: existing `Text` and model-backed `Table`, one PX-073 graph node for the aggregate and expanded interfaces |
| Units / freshness | IEC bytes/s and `packets/s`, never bits; own sample time; `warming up` on first sample, counter decrease, rename or recreation; `none configured` when only loopback exists |
| Budget | ≤ 16 nodes whatever the expansion; interface table ≤ 64 rows of ≤ 512 bytes; series 2 aggregate + 2 × 8 = 18 in the one graph node; ≤ 900 samples each; fallback ≤ 1,080 rows |
| Accessible fallback | `Network (host, non-loopback interfaces): receive 1.2 MiB/s, transmit 85.0 KiB/s, sampled 14:02:07, every 1 s`; the interface table for current rates; the fallback sample table for the history of the aggregate and every expanded interface |

### 6.5 Process table, inspector and sampled history

| Aspect | Specification |
| --- | --- |
| Table | Existing and unchanged: model-backed `Table`, CPU sort, filter, search, follow, tree (PX-009–PX-018). Selection, filter and viewport survive every panel update and every layout change (asserted by PX-073-G01) |
| Inspector | Existing `apps/srtop/src/inspector.rs` (PX-013): selection resolves only against published rows to a `ProcessKey`, never a PID |
| History | **PX-090**'s opt-in bounded store for selected/watched processes, rendered with the same PX-073 renderer; only samples actually collected for that `ProcessKey`, with a labelled recording start; an exited process's history stays viewable and non-actionable; a new daemon starts a new recording. Historical browsing with a Live/History switch is PX-091 (later, not a core dependency) |
| Budget | PX-090's caps: count, byte and age limits with a 15-minute maximum; at most `MAX_HISTORY_PROCESSES` (4) processes × `MAX_HISTORY_SERIES_PER_PROCESS` (4: CPU share and resident memory now, room for PX-047's read/write rates) series, ≤ 900 samples each; charged to `HISTORY_RESERVE_BYTES` (section 7) |
| Fallback | The inspector's existing text lines plus the history's fallback sample table (≤ 60 rows per series, ≤ 64 bytes each) |

## 7. Dashboard budget summary

| Bound | Maximum | Basis |
| --- | --- | --- |
| Dashboard nodes beyond today's shell | ≤ 96 (panels 12 + 16 + 16 + 16, ≤ 18 panel controls, ≤ 8 layout and `Split` nodes, rounded up) | constant after the shell, like PX-070's constant 111-node shell |
| Host series | ≤ 39 (1 + 2 + 2 + 16 + 2 + 16) | section 6 |
| Samples per series | `MAX_SERIES_SAMPLES` = 900 and an age cap of 15 minutes, whichever is reached first (15 minutes at 1 s; 45 s at the 50 ms minimum interval) | PX-090's 15-minute maximum |
| Device/interface rows | ≤ 64 per panel, ≤ 512 bytes each | section 6 |
| Fallback sample rows | ≤ `MAX_FALLBACK_SAMPLES` (60) per series, ≤ `MAX_FALLBACK_ROW_BYTES` (64) bytes each on the wire | section 5 |
| History series | ≤ 16 (4 processes × 4 series), ≤ 900 samples each | section 6.5 |
| Snapshot allowance | Two reserves charged against the row ceiling of the one 16 MiB catch-up frame, exactly as `METERS_RESERVE_BYTES` and the other reserves in `apps/srtop/src/refresh.rs` are: `DASHBOARD_RESERVE_BYTES` = 1,179,648 (1,152 KiB) and `HISTORY_RESERVE_BYTES` = 458,752 (448 KiB), 1,638,400 bytes (1,600 KiB) together; the arithmetic follows the table. Because a semantic tree carries an extension node's fallback subtree with it (§11.1), both the profile series and their fallback rows are charged, whichever the client renders | PX-004-G01, PX-070 |
| Steady-state panel payload | ≤ 4 KiB/s at the 1 s default with every panel expanded and 8 devices and 8 interfaces showing series, measured as SRUI payload apart from SSH overhead (the PX-027 method) | pass/fail threshold for PX-073-G02 |
| Server memory for host and history series | ≤ 4 MiB | PX-072's and PX-090's bounded stores |

**Snapshot arithmetic.** A sample is assumed to cost at most 24 bytes in the PX-071 encoding (a timestamp, a sequence number and a float64); PX-071 fixes the real encoding and PX-073-G01 recomputes every line below from it, keeping each reserve at or above its sum.

| Charged to | Item | Bytes |
| --- | --- | --- |
| `DASHBOARD_RESERVE_BYTES` | host series: 39 × 900 × 24 | 842,400 |
| | host fallback rows: 39 × 60 × 64 | 149,760 |
| | device and interface rows: 2 × 64 × 512 | 65,536 |
| | dashboard nodes: 96 × 640 (≤ 512 bytes of text plus node and property framing) | 61,440 |
| | **sum** (reserve 1,179,648; margin 60,512) | **1,119,136** |
| `HISTORY_RESERVE_BYTES` | history series: 4 × 4 × 900 × 24 | 345,600 |
| | history fallback rows: 4 × 4 × 60 × 64 | 61,440 |
| | **sum** (reserve 458,752; margin 51,712) | **407,040** |
| Both | **total reserve** | **1,638,400** |

**Effect on the row ceiling.** The reserves come out of the rows a catch-up snapshot can hold. PX-070's widest-row fixture (`a_high_core_count_host_stays_bounded_in_every_layout`, `apps/srtop/tests/meters_test.rs`) publishes 25,564 rows at `de1edd69`; with `METERS_RESERVE_BYTES` raised by 1,638,400 bytes in a scratch copy of the same commit, it publishes **23,055** rows (2,509 fewer, the snapshot 13,075,303–13,075,315 bytes instead of 14,500,426–14,500,438), the same for every CPU count. PX-073-G01 must re-baseline every row-ceiling assertion (that test, `the_rows_at_the_ceiling_are_the_frame_less_the_fixed_allowances_whatever_the_summary` in `apps/srtop/src/refresh.rs`, and the PX-010-G01 scale tests) to its real reserves, and the R-Dashboard envelope states the lower ceiling.

## 8. Authority and safety

- Collection, interpretation, aggregation rules, freshness and history retention are server-side; the client renders strings, bar fractions and series it is given (the `summary.rs` rule). No process-name, PID or device-specific client code path.
- The dashboard has no process-changing control. The existing action panel keeps PX-028–PX-031's rules: actions bind to an immutable server-validated `ProcessKey` target, never to a panel, a graph point or a history sample. Paused, historical, stale or replaced contexts cannot authorize an action; a history row is never a target.
- Panel sampling does not use the transaction journal as history and history does not use the journal (PX-090).

## 9. Relationship to the existing plan tickets

PX-070 (meters, merged) provides compact CPU/memory/swap and the moves-only layout pattern; PX-071 specifies the metrics profile (and now depends on PX-070-G01 for the one-graph-node-per-panel requirement of section 5); PX-072 publishes series; PX-073 renders them; PX-054/PX-055/PX-056 provide the detailed memory, disk and network collectors; PX-090 provides process history; PX-070-G02 (new, section 10) provides `Split`; PX-073-G01 composes all of them to this specification; PX-073-G02 runs section 11 against the pinned btop and gates R-Dashboard. PX-095 (merged) supplies host-isolated windows and chrome and is now an explicit dependency of PX-073-G01 (A8); PX-022-G01/PX-022-G02 supply stale-window status and selection freeze; PX-015 supplies pause.

## 10. Generic widget and profile support in this checkout

Reviewed at `de1edd69`.

| Capability the dashboard needs | Present? | Evidence | Owner if missing |
| --- | --- | --- | --- |
| `Row`, `Column`, `Grid`, `Text`, `Progress`, `Button`, `Scroll` | yes: registry, SDK builder and AppKit renderer | `protocol/registry.yaml` node types 3–5, 9, 11, 15, 8; `server-rust/sdk/src/widgets/{primitives,text,buttons,media}.rs`; `client-macos/RendererAppKit/ControlFactory.swift` (`makeHandle` cases `.row`, `.column`, `.grid`, `.text`, `.button`, `.progress`, `.scroll`) | — |
| Model-backed `Table` with virtualized rows, sort, selection | yes | registry type 18; `server-rust/sdk/src/widgets/collections.rs`; `ControlFactory.makeTable`; PX-004-G01, PX-010-G01 scale evidence | — |
| Visibility `collapsed`, `MOVE_NODE` layout changes | yes | registry `visibility` (`Visibility` enum: visible, hidden, collapsed); PX-070's `METER_HIDDEN` and moves-only transitions (`apps/srtop/src/meters.rs`) | — |
| Keyboard shortcuts for focus and buttons | yes | registry property `shortcut` (PX-016-G01) | — |
| Client-owned host/account/incarnation chrome | yes | `client-macos/RendererAppKit/SessionWindowIdentity.swift` (PX-095) | — |
| Client-owned stale/disconnected status and stale selection freeze | yes | PX-022-G01 (`setSessionWindowStatus`, window subtitle), PX-022-G02 | — |
| Extension negotiation with Standard Widget fallback (§11.1) | yes, with one native extension kind | `client-macos/RendererAppKit/ExtensionMountResolver.swift` (`ExtensionControlKind` has only `.terminal`; `.fallback(roots:)` mounts a fallback subtree); `server-rust/semantic-tree/src/capability.rs` profile constants (`org.srui.standard-widgets`, `org.srui.terminal`, `org.srui.vector-scene`, …) | — |
| Numeric gauge / bounded time series (sparkline, history graph) | **no**: no node type, profile or renderer; no series store | registry has 27 standard node types and none is a graph; no `TimeSeries`/`Sparkline` anywhere in `server-rust`, `client-macos`, `protocol` or `apps/srtop/src`; `ExtensionControlKind` has no metrics kind | already planned: **PX-071** (profile, project-owned namespace, not `org.srui`), **PX-072** (server series), **PX-073** (renderer). Not a new ticket |
| User-resizable panes (`Split`) | **no**: registry only | registry type 25 `Split` (tier `should`, emits `VALUE_CHANGED`); no SDK builder (`impl_widget_boilerplate!` covers Surface, Row, Column, Grid, Spacer, Separator, Scroll, Text, RichText, Button, Toggle, TextInput, TextArea, Progress, Image, List, Table, Tree only); `ControlFactory.makeHandle` has no `.split` case and throws `ControlFactoryError.unsupportedNodeType` (`default:`); the registry has no pane-axis property or enum, and D1 names none | **new prerequisite PX-070-G02**, added to the plan by this ticket; PX-073-G01 now depends on it |
| `Tabs` | no (registry only) | registry type 24; no builder, no renderer case | not needed by the core dashboard; no ticket |
| Labelled layout container as an accessibility group | not verified | section 5 | PX-073-G01 verifies; appends a renderer prerequisite if missing |
| Host disk and network collectors | no | no reader in `apps/srtop/src/` | already planned: PX-055, PX-056 |
| Detailed memory breakdown | partial (used/total, swap) | `summary.rs`, `procfs.rs` | already planned: PX-054 |
| Selected-process sampled history | no | — | already planned: PX-090 |
| VectorScene (`org.srui.vector-scene`) | constant only | `capability.rs` `PROFILE_VECTOR_SCENE`; no renderer | not used: PX-071 explains why graphs are a domain profile, not T36 VectorScene |

**PX-070-G02 — Render the standard Split container with locally resized panes** (phase G, size M, gate none, depends on PX-070-G01; ticket `tickets/PX-070-G02-split.md`). Generic: an SDK builder, one registry-owned pane-axis property (`split_axis`, enum `SplitAxis`: `side_by_side` default, `stacked`), the AppKit `NSSplitView` mapping (§22.4), local divider positions with no event and no geometry on the wire, at most 8 panes, codegen and conformance vectors (suites 1, 2, 3, 4, 9, 10, 12), native AppKit and accessibility tests. Its full build, out-of-scope and acceptance text are in the ticket. It changes the registry and therefore regenerates the committed Swift tables and updates the `semantic-tree` build.rs count asserts in its own commit, like PX-016-G01, PX-017-G01 and PX-018-G01 did. The user decided on 2026-11-27 that it is a required prerequisite of PX-073-G01 (A2) and that it plans `split_axis`/`SplitAxis` (A3); the registry change itself still needs recorded user approval when PX-070-G02 is implemented.

## 11. Evaluation protocol (defined before implementation)

PX-073-G02 runs this protocol. The thresholds below are fixed now; a failed threshold keeps R-Dashboard open and may be changed only by an amendment to this section committed before the rerun, with the reason. Observations of btop are recorded side by side and are never pass/fail for btop.

### 11.1 Matched environment

- **Host.** One Linux environment for both tools, recorded exactly: the isolated devbox (`apps/srtop/devbox/`, as PX-027 and PX-031 used it) or a named VM, with kernel, architecture, logical CPU count, memory, cgroup limits, PID and network namespaces. Both tools run in the same box, as the same unprivileged account, against the same `/proc`; neither gets setuid, setcap or root (btop's README recommends `make setcap`/`setuid`; it is not used).
- **btop build.** The pinned `v1.4.8` musl release binary for the box's architecture, its digest checked against section 2 before use; it lives only in the disposable box. A distribution package of another version is a recorded mismatch and voids comparison (the PX-027 htop-version lesson: Debian 12 packaged 3.2.2, not the baseline).
- **Transport.** Both over SSH to the same box through the same TCP relay with RTT 0, 100, 300 and 600 ms (the PX-027 relay): srtop through `srui-ssh-bridge` and the native Mac client; btop in a terminal over `ssh -t`. Record the client Mac model, macOS version, display refresh rate, terminal application and its size in cells.
- **Sampling.** Identical intervals set explicitly on both: btop `-u 1000` and `-u 2000`; srtop `--refresh-interval-ms 1000` and `2000`. btop's default (2000 ms) and srtop's default (1000 ms) differ, so no default-to-default comparison is reported.
- **Fields.** Only fields both tools show are compared: per-process CPU share and resident memory; host CPU busy share; memory used and swap used; disk read/write rates for the same device; network receive/transmit rates for the same interface (btop `net_iface` set to it). btop's per-mountpoint disk view is mapped to devices and the mapping recorded. Units are normalized to IEC bytes per second before comparison (btop network figures may be bit rates). A field only one tool shows is "not comparable", never "worse".
- **Repetition.** At least 3 runs per condition; report min / median / max.

### 11.2 Workloads (owned, bounded, disposable)

Every worker is started by the harness, owned by the test account, ended and reaped by it, and bounded in time (as PX-031's guards do): `background` about 200 idle sleepers (the PX-027 busy shape); `busy` one CPU-bound worker with a unique name, at most 120 s; `trend` one worker alternating 10 s busy and 10 s idle for 120 s; `disk` 512 MiB written to a temporary file in the box in 8 MiB chunks with fsync over at least 20 s, then deleted (never a raw device); `net` 256 MiB transferred over the box's non-loopback interface to a peer the harness owns, rate-capped, over at least 20 s; `cut` the relay killed (EOF) for 10 s, and separately for long enough that the journal (`DEFAULT_MAX_JOURNAL_ENTRIES` 1024 in `server-rust/journal/src/lib.rs`) no longer covers the client, confirmed by a snapshot resynchronization rather than `RESUME_OK`; `replace` srtop restarted while attached. The harness computes reference values from its own reads of `/proc/stat`, `/proc/meminfo`, `/proc/diskstats` and `/proc/net/dev` at the tool's sample times.

### 11.3 Tasks and pass/fail criteria for the dashboard

Each task runs three ways: the in-process T35 automation driver through the normal event path; a keyboard-only run; and a VoiceOver run on a client without the metrics profile (numeric fallback only). All three must pass.

| Task | Start → success condition | Pass criteria (dashboard) |
| --- | --- | --- |
| E1 Locate a busy process | Default layout, no filter, `background` running; start `busy` → the inspector names its `ProcessKey` (PID and start time) and name | (a) the worker is the first row of the CPU sort within 2 intervals + 1 s of starting; (b) reachable in at most 3 user actions from the default state (or filter text plus 2); (c) the CPU panel's median busy share over 10 samples is within ±2 percentage points of the harness reference; (d) local selection and typing feedback ≤ 50 ms at every RTT and not more than 20 ms above its 0 ms value (§31.4); (e) the inspector result within RTT + 1 interval + 250 ms of the selection |
| E2 Inspect a trend | `trend` for 60 s with the worker selected → the CPU panel graph (or fallback) and the inspector history show the duty cycle | (a) at least 3 busy/idle transitions visible at the harness's times ±1 interval in the host series and in the process history; (b) the fallback's values equal the published samples exactly and are within ±2 percentage points of the reference; (c) a 3-interval Pause appears as a labelled gap with its duration, never zero or interpolated; (d) the history shows its recording start; after the worker exits the history stays viewable and no action is enabled for it; (e) retained samples never exceed section 7's caps |
| E3 Inspect disk and network activity | `disk` then `net` → the Disk and Network panels show the activity on the right device and interface | (a) mean write rate over the transfer within ±10% of the harness's `/proc/diskstats` delta over the same samples, and receive/transmit within ±10% of the `/proc/net/dev` delta; (b) each figure states unit, scope, interval and sample time; (c) `warming up` on the first sample, no zero; (d) no panel text names a process as responsible; (e) expanding the device and showing its series changes structure exactly once and per-sample traffic stays under section 7's steady-state bound |
| E4 Recover after disconnect | `busy` and `net` running, a process selected and a filter set; `cut` for 10 s, then the long `cut`, then `replace` → the window is current again | (a) the stale subtitle naming the revision appears within 1 s of the relay EOF; (b) while stale no value becomes zero, no selection change is accepted and no action is enabled; (c) after the short cut `RESUME_OK`, and after the long cut a snapshot resynchronization, each complete — subtitle cleared and every panel showing a sample newer than the reconnect — within 12 × RTT + 3 s; (d) selection, filter and table viewport preserved after `RESUME_OK`; (e) host series and history include the samples the server took while detached and no sample the server did not take; (f) after `replace`, PX-095's chrome shows the new incarnation, history restarts with a new recording start, and no old intent survives (PX-031) |

### 11.4 Measurements recorded for both tools

Task completion time and success; user actions per task; collection overhead (CPU share and resident memory of btop versus `srtop` + `srui-sessiond` + `srui-ssh-bridge`, mean over 60 s per workload); encrypted SSH bytes per second for both, and SRUI payload bytes apart from SSH overhead for srtop (the PX-027 method); local input-to-feedback latency and remote-result latency at each RTT; memory and queue caps reached. Optional panels (per-CPU, temperature, GPU) are reported separately as verified or explicitly unavailable.

### 11.5 Reporting rules

- State each comparison as a measured number under named matched conditions ("at 300 ms RTT and a 1 s interval, the filter result arrived in X ms; btop's echoed keystroke took Y ms"), never as general superiority.
- No conclusion is drawn from screenshots. Screenshots may illustrate a recorded measurement and are labelled as illustration (§32: screenshots are not semantic conformance criteria).
- Differences in field definitions (for example btop's memory "used" versus `MemTotal − MemAvailable`) are named; non-matching fields are not compared.
- Passing does not certify htop or btop parity (PX-073-G02 verify).

## 12. Assumptions and decisions

A2 and A3 were decided by the user on 2026-11-27, answering the orchestrator's question; the answers are quoted. The rest are defaults within this ticket's scope.

- **A1 — btop v1.4.8 is the reference.** Latest release on 27 November 2026. Rebaselining is an explicit plan change (section 2).
- **A2 — `Split` is a required prerequisite of the core dashboard. Decided: "Draggable divider (Recommended)".** PX-070-G02 stays in PX-073-G01's `depends_on`, so the user can drag the panel/table divider. The rejected alternative limited local resizing to window resize, graph reflow and compact/expanded presentations and kept `Split` optional.
- **A3 — `Split` gets one new registry property for its axis. Decided: "Plan split_axis (Recommended)".** PX-070-G02 plans the registry property `split_axis` with enum `SplitAxis` (`side_by_side`, `stacked`). D1 defines `Split` but no orientation, and no existing property expresses it. The rejected alternatives were a fixed axis per node (no registry change, but a stacked dashboard and a side-by-side split could not both be expressed) and two node types (contradicts D1's single `Split`). This decision approves the plan only: the protocol/registry change itself still needs recorded user approval when PX-070-G02 is implemented, before it is merged.
- **A8 — PX-073-G01 depends on PX-095.** The host/account/incarnation labels the dashboard relies on are PX-095's client chrome, so PX-095 is an explicit dependency of PX-073-G01 (it was already one of PX-073-G02's).
- **A4 — Compact CPU and Memory/swap reuse the PX-070 meters** rather than adding parallel panel nodes, so the same figure never appears twice with different freshness.
- **A5 — Bytes per second, never bits**, across the dashboard; the evaluation normalizes btop's units.
- **A6 — Budgets and thresholds in sections 7 and 11 are design maxima** chosen from existing srtop bounds and PX-027/PX-010-G01 measurements, not measured for the dashboard; PX-073-G01/PX-073-G02 measure them.
- **A7 — The evaluation downloads the pinned btop binary into the disposable devbox only**, checked against its published digest; nothing from btop enters the repository.
