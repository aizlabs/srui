# Retained metrics-visualization profile `io.github.aizlabs.srui.metrics/1` (PX-071)

This is the specification of a small, separately versioned, optional SRUI extension profile for numeric gauges and bounded time series. It is written before any implementation: PX-072 implements the server side, PX-073 the client renderer, PX-073-G01 composes it into the dashboard, and PX-090 reuses it for sampled process history. Design citations are to D1, `SRUI_Semantic_Remote_UI_Design_v0.6.md` (read-only); D2 is `SRUI_Implementation_Plan.md` (T20 capability negotiation, T31 extension fallback, T36 VectorScene). Repository citations are to the checkout at `242c9348` (`origin/main`).

**Status.** Specification only. Nothing in this document is registered, built or enabled: no `protocol/registry.yaml`, `protocol/srui.proto`, generated, SDK, runtime, client or srtop file changes, no production graph exists, and no session advertises the profile. The profile needs no new wire message, operation, value kind, registry entry or capability mechanism; everything below is expressed with the existing §6.4 namespaces, §6.5 values, §8/§13 models and operations, §11.1 fallback and §15 negotiation.

The golden fixtures are documentation fixtures in [`metrics-profile-fixtures/`](metrics-profile-fixtures/) (section 12), not `.bin` conformance vectors.

## 1. Inputs and requirements

| Requirement | Source | Where met |
| --- | --- | --- |
| Separately versioned optional profile; project-owned canonical namespace assigned by normal negotiation, not `org.srui` | ticket build text | section 3 |
| Numeric gauges and bounded time series with units, source timestamps, series identity, append/replace/evict semantics, axis intent, limits, accessibility summaries | ticket build text | sections 4–7, 9 |
| Standard Widget fallbacks and golden fixtures; no paths, shaders, drawing scripts or downloaded expressions | ticket build text | sections 8, 12; section 2 |
| Why a domain extension, not T36 VectorScene or graphs in Image resources | ticket build text | section 2 |
| Independent panel timestamps, explicit gaps and staleness, units and denominators, bounded windows, stable host/device identity; no implied atomic host snapshot | ticket build text | sections 5.1, 5.3, 5.5–5.7 |
| One graph node carries at least 18 series, each with its own stable identity, unit and gaps | dashboard.md section 5 | sections 4, 5.2, 7 |
| That node's Standard Widget fallback is one bounded model-backed `Table` of the most recent 60 samples of every series, at most 64 bytes per row on the wire | dashboard.md sections 5, 7 | section 8.2 |
| Real per-sample encoding so PX-073-G01 can recompute the snapshot reserves | dashboard.md section 7 | section 7.2 |
| Schema review: units, negative values, missing samples/gaps, window bounds, profile-version mismatch, unsupported-client fallback; both sides implementable without AppKit | ticket verify text | sections 10, 11 |

## 2. Why a domain-specific extension profile

A retained graph of sampled values is application-domain meaning ("this host's receive rate was 1.2 MiB/s at 08:00:01, then the collector failed"), not a paint technique. The profile carries that meaning and nothing else; the renderer chooses lines, colours, ticks, density and motion locally.

- **Not a Standard Widget Profile type.** Appendix A puts a concept in the Standard Widget Profile only if every toolkit can reasonably implement it and it is basic UI semantics. Sampled time series with units, gaps and retention are a monitoring domain; §11 is where domain semantics go ("syntax-highlighted Diff — coding extension profile"). Adding a namespace-0 node type would also be a registry and protocol change binding every SRUI client, which this ticket may not make.
- **Not T36 `VectorScene`.** §11.2 makes VectorScene "the escape hatch for arbitrary retained drawing" with `Path`, `Rect`, `Transform`, `Gradient`. Lowering a graph to it would move axis scaling, tick placement, line geometry and gap drawing to the server, send geometry that must be recomputed whenever the window, divider or theme changes (contradicting §12.2 and the dashboard's "local resizing sends nothing"), and leave a client nothing to read aloud except shapes. Every new sample would rewrite path geometry rather than append one value. T36 is optional, "only after a real need appears", and not built in this checkout (`capability.rs` has only the `PROFILE_VECTOR_SCENE` constant; dashboard.md section 10). This profile neither needs nor assumes it.
- **Not graphs in `Image` resources.** §14 resources are immutable content-addressed blobs. A graph image would be re-rendered and re-sent on every sample (the "stream of frames" §5.2 excludes for ordinary UI; §4 inv. 9 confines pixels to explicit media/surface profiles; §12.2), at a resolution and theme the server guessed, with no accessible values, and its bytes would scale with pixels, not with sampled changes (§31.3). The profile's steady-state traffic is a few bytes per sample (section 7.3).
- **No code or expressions.** The profile has no paths, shaders, drawing scripts, formulas, downloaded expressions, CSS or HTML. Every rule is a closed check over SRUI values, and every token set is closed (§26 "the base protocol permits no remote executable code").

## 3. Identity, versioning and negotiation

### 3.1 Canonical name

The canonical profile identifier is **`io.github.aizlabs.srui.metrics/1`** (name `io.github.aizlabs.srui.metrics`, major version `1`).

- **Ownership.** The project's canonical repository is `https://github.com/aizlabs/srui` (`git remote -v` of this checkout). `io.github.aizlabs` is the reverse-DNS form of `aizlabs.github.io`, which only the `aizlabs` organization controls; that is the same convention §6.4 shows with `org.example.coding/2`. The name does not use `org.srui`: the project does not establish ownership of `srui.org`, and the `org.srui.*` names in §6.4/§11 and `capability.rs` belong to the design's own profiles. Decision A1 (section 14) records this for review.
- **Grammar.** The string parses with the existing `Profile::parse` (`server-rust/semantic-tree/src/capability.rs`): a non-empty name, `/`, a positive integer major version.

### 3.2 Negotiation rules (N)

- **N1 — optional, one major, assigned by the server.** A server advertises the profile only in `optional_profiles`, never in `required_profiles`: every profile node carries a Standard Widget fallback (§11.1 "a server MUST NOT send an unsupported extension as required content"). It registers at most one major version per session, before any client attaches, through the existing `Session::register_optional_extension_profile` (`server-rust/sessiond/src/session/extensions.rs`), which allocates the session's non-zero namespace number and puts the mapping in `extension_namespaces` (§6.4, §15). A server or SDK that would advertise two majors, or the profile as required, refuses at registration; the existing function does not do this yet (it registers `/1` and `/2` independently), so the refusal is a new PX-072 obligation (section 13). Clients never choose or assume the namespace number (section 12, fixture 06 uses 2 and 7).
- **N2 — exact activation.** The profile is active for a client exactly when the client offers `io.github.aizlabs.srui.metrics/1` in `ClientHello.profiles` (or `ClientResume.profiles`) and the server advertised it: §15's intersection, which `CapabilitySet::negotiate` computes with exact `contains`. A client implements the profile's nodes natively only when it is active and the mapping for that URI is present; it then registers the profile's two node types as native extension kinds (the role of `ExtensionMountResolver.register(typeRef:kind:)`). Otherwise it is a base client for this profile.
- **N3 — no partial understanding across majors.** Any change to a rule, limit, token set, local ID or encoding is a new major version with a new URI suffix. There are no minor versions and no feature flags. A client implementing only `/2` in a session that advertises `/1` is a base client: it renders the fallbacks, refuses nothing, and does not interpret `/1` content with `/2` rules.
- **N4 — reconnect.** Resume and resynchronization reuse §15 and §18 unchanged: profiles are re-advertised on `CLIENT_RESUME`, mappings are re-advertised on `RESUME_OK`/`RESYNC_REQUIRED`, and a client that no longer implements the profile simply mounts the fallbacks of the replica it already holds. The existing cold-resume rule (§18: an extension-bearing replica without its mappings fails closed) applies as for any extension.

| Situation | Outcome (fixture 06) |
| --- | --- |
| Client offers and implements `/1`; server advertises `/1` optional with a mapping | native |
| Client offers only `/2` (implements `/2`) | fallback; nothing refused |
| Client offers `/1` and `/2`; server advertises `/1` | native `/1` |
| Client offers neither | fallback |
| Server would advertise `/1` and `/2`, or `/1` as required | the server refuses (N1); never sent |
| Server advertises `/1` without a namespace mapping | handshake fails before snapshot (existing §15 rule, `handshake.rs` "has no extension namespace mapping") |
| Session-assigned namespace is not the one a client expected | native; the number is whatever `extension_namespaces` says |

## 4. Object model

All profile references are `(namespace_id, local_id)` pairs whose `namespace_id` is the session-assigned number for the profile (§6.4). Type local IDs and property local IDs are separate spaces, as in namespace 0. In this document and the fixtures, `m:Name` means the profile's entry and `std:name` a namespace-0 entry.

### 4.1 Types

| Local ID | Name | Kind | Purpose |
| --- | --- | --- | --- |
| 1 | `Gauge` | node type | One current numeric value with unit, state, sample time and axis |
| 2 | `SeriesGraph` | node type | Up to 18 bounded sampled series on one time axis and at most two value axes |
| 3 | `SeriesSet` | model type (`CREATE_MODEL.model_type`) | A graph's series descriptors, one item per series, in legend order |
| 4 | `SampleSeries` | model type | One series' samples and gaps, oldest first |
| 5 | `AxisSpec` | small typed record type (§6.5 `SmallRecord.type`) | A value axis: unit, intent, range |

### 4.2 Properties

| Local ID | Name | Value | Carried by |
| --- | --- | --- | --- |
| 1 | `unit` | string token (section 5.4) | `AxisSpec`, descriptor |
| 2 | `metric_state` | string token (section 6) | `Gauge` |
| 3 | `metric_value` | float64 | `Gauge` |
| 4 | `axis_intent` | string token: `share`, `zero_based`, `signed` | `AxisSpec` |
| 5 | `range_min` | float64 | `AxisSpec` (`share` only) |
| 6 | `range_max` | float64 | `AxisSpec` (`share` only) |
| 7 | `denominator` | string, 1–64 bytes | `Gauge`, descriptor (optional) |
| 8 | `sample_unix_ms` | uint64 | `Gauge` |
| 9 | `interval_ms` | uint64, 50–300,000 | `Gauge`, descriptor |
| 10 | `subject_key` | string key (section 5.6) | `Gauge`, descriptor |
| 11 | `subject_label` | string, 1–64 bytes | `Gauge`, descriptor |
| 12 | `epoch_unix_ms` | uint64 > 0, immutable | `SeriesGraph` |
| 13 | `window_ms` | uint64, 1,000–900,000 | `SeriesGraph` |
| 14 | `primary_axis` | `AxisSpec` record | `SeriesGraph`, `Gauge` (required) |
| 15 | `secondary_axis` | `AxisSpec` record | `SeriesGraph` (optional) |
| 16 | `sample_models` | list of exactly 18 uint64 model IDs, immutable | `SeriesGraph` |
| 17 | `series_key` | string key (section 5.6) | descriptor |
| 18 | `series_label` | string, 1–32 bytes | descriptor |
| 19 | `series_axis` | string token: `primary`, `secondary` | descriptor |
| 20 | `samples_model` | uint64 model ID | descriptor |
| 21 | `sample_kind` | string token: `instant`, `interval` | descriptor |

Standard properties keep their standard meaning (§6.4 "an extension may not change the meaning of a standard node or standard property"). Only these are admitted on profile nodes: `label` (1–64 bytes, required), `value_description` (1–512 bytes, required: the accessibility summary, section 9) and `visibility` on both; `model_ref` (required, names the graph's `SeriesSet`) on `SeriesGraph`. Any other standard property, any unknown profile property, and any profile property on a standard node is refused (S2).

### 4.3 Shapes

```text
SeriesGraph #10                          std:label, std:value_description, std:model_ref = 100,
 │                                       m:epoch_unix_ms, m:window_ms, m:primary_axis,
 │                                       [m:secondary_axis], m:sample_models = [101 … 118]
 └── Table #11   (the only child: the Standard Widget fallback, section 8.2; model 119)
Model 100 : m:SeriesSet      items = descriptors (item_id = series id, value = null, properties = section 4.2)
Model 101 … 118 : m:SampleSeries   18 slots; a slot is bound to at most one descriptor
Model 119 : std:Table        fallback rows

Gauge #40                                std:label, std:value_description, m:primary_axis,
 │                                       m:metric_state, [m:metric_value, m:sample_unix_ms],
 │                                       m:interval_ms, m:subject_key, m:subject_label, [m:denominator]
 └── Text #41   (or Row/Column of exactly Text then Progress; section 8.1)
```

**Fixed model slots.** SRUI has no wire operation that deletes a model (`StandardOperation` in `protocol/srui.proto` ends at `BATCH_PROPERTY_SET`; the Rust `SemanticStore::delete_model` in `server-rust/semantic-tree/src/store/mod.rs` is programmatic only, as its doc comment says). A graph therefore creates its `SeriesSet`, its 18 `SampleSeries` slots and its fallback model once, with the graph, and reuses slots: adding a series binds a free slot, removing one deletes its descriptor and empties its slot in the same transaction (D4). Models live until the session ends; S4 bounds how many graphs a session may ever create, so the model count is finite (section 7.1). This is a deliberate ceiling of the current protocol, not of the profile; a generic wire `DELETE_MODEL` would remove it and is out of scope.

## 5. Series semantics

### 5.1 Timeline and source timestamps

- A graph has one timeline: `epoch_unix_ms` is the wall-clock instant (Unix milliseconds) of offset 0, captured when the graph was created, and never changes (S3).
- A sample's **`item_id` is its offset**: milliseconds since the epoch on the server's monotonic clock, taken when the collector read the source (the source timestamp), not when the transaction committed. Offsets are below 2^40 (about 34.8 years) and strictly increase along a series (T1). Sampling intervals are at least 50 ms, so offsets never collide; using the offset as the item identity makes every sample's identity stable across snapshots, replay and reconnect, and costs no extra property.
- The displayed sample time is `epoch_unix_ms + offset`. A wall-clock step on the server does not rewrite history and does not move the timeline backwards; the panel's freshness line (server text) states current wall-clock times as before.
- **Independent panels.** Each graph has its own epoch, each series its own offsets, each gauge its own `sample_unix_ms` (fixture 08). Series in one graph need not share sample times (a disk device may be read a few milliseconds after another). No profile property states a host-wide snapshot time; a transaction that carries samples of several panels asserts only that they were published together (§12.2: commits are not frames), and a client never aligns or labels panels by revision, transaction or arrival time.

### 5.2 Samples and gaps

A `SampleSeries` item is exactly one of:

- **a sample**: value `float64`, finite, |v| ≤ 10^18, no item properties (T2); or
- **a gap**: value `uint64` gap-reason code, no item properties (T2).

| Code | Reason | Meaning | Fallback word (section 8.2) |
| --- | --- | --- | --- |
| 1 | `warming_up` | a rate needs a second read; a counter decrease, reset or device reappearance started a new baseline | `warming up` |
| 2 | `unavailable` | the collector could not state the figure | `unavailable` |
| 3 | `denied` | the source refused access | `denied` |
| 4 | `not_provided` | this source does not provide the figure | `not provided` |
| 5 | `none_configured` | nothing to measure (no swap, no device) | `unconfigured` |
| 6 | `collector_error` | the read failed | `error` |
| 7 | `stale` | no successful sample for 3 intervals, detected by the server's own timer (dashboard.md section 5) | `stale` |
| 8 | `paused` | collection stopped by the user (PX-015) | `paused` |

- A gap begins at its offset and lasts until the next item of its series; the last item, if a gap, is ongoing. Its duration is never stated separately on the profile wire, so it cannot disagree with the offsets.
- Two consecutive gap items with the same code are refused (T4): a gap is written once, at its start. A different reason after a gap starts a new gap (an error that becomes stale).
- A gap is never a zero, never `null`, never a string, and never interpolated across. A counter reset is a `warming_up` gap, never a negative or zero rate (fixture 03). The first read after Resume follows the `paused` gap with `warming_up` when the series is a rate (fixture 04).
- The server appends one item per actual read; it never appends to fill time, never repeats a previous value for a tick without a read, and never extrapolates. Equal values from two reads are two samples. This is the "repeated-sample suppression" PX-072 tests: suppression of non-reads, not of equal values.
- `sample_kind` says how to read a sample: `instant` (a level at its offset: memory used) or `interval` (a mean over the interval that ends at its offset and starts at the previous item's offset, sample or gap: a CPU share or a rate). The interval of the oldest retained sample after eviction is unknown and is drawn from its own offset.

### 5.3 Append, evict and replace

Every operation below has its existing generic meaning (§13; Rust `Model` in `server-rust/semantic-tree/src/model.rs`, Swift `Model` in `client-macos/SemanticModel/Model.swift`), so a base client that ignores the profile ends in exactly the state a capable client reaches. On a `SampleSeries` model only these operations are valid (T5):

| Operation | Rule |
| --- | --- |
| `MODEL_INSERT` | append only: `index` equals the current item count; offsets continue the strictly increasing sequence |
| `MODEL_DELETE` | evict only: `index` 0 and `count` ≥ 1 (the `item_ids` form is refused) |
| `MODEL_RESET_RANGE` | **fill only**: `total_count` 0, at least one item, and every position it writes is inside the item count and holds no item yet. This is the form sessiond's snapshot exporter already emits (`export_snapshot_transaction` in `server-rust/sessiond/src/session/snapshot.rs`: `CREATE_MODEL` with `item_count` n, then `RESET_RANGE` chunks with `total_count` 0). A reset that writes a held position, carries no items, or sets `total_count` is refused |
| `MODEL_UPDATE` | refused: a published sample is immutable |

- **Why replace is not a reset.** Generically, `MODEL_RESET_RANGE` ignores `total_count` 0 and rewrites only [`start_index`, `start_index` + number of items). A shorter reset therefore leaves the old items beyond its range cached past the new count, and an empty reset is a no-op (fixture 05 `reset-range-cannot-shrink-a-series`, `reset-range-cannot-empty-a-series`, where the base client's resulting state is asserted). Both sides implement this identically, and a base client cannot apply profile-specific meaning.
- **Dense models** (T7): at the end of every transaction a `SeriesSet` or `SampleSeries` holds an item at every index below its item count. A profile model created with a non-zero `item_count` must be filled in the same transaction (fixture 05 `snapshot-fill-form`, `fill-leaves-a-hole`). Clients never range-request a profile model: it is always complete.
- **Retention is exact** (T6): after every transaction a series holds exactly the samples whose offset is at least *newest offset − `window_ms`*, and at most `MAX_SAMPLES_PER_SERIES` (900) of the newest. At every operation boundary it holds at most 900 items, so a transaction at the cap evicts before it appends (fixture 05).
- **Replace** rebuilds a series on the server (for example after the series store restarts), never corrects a sample. It is two generic operations in one transaction:
  1. `MODEL_DELETE` with `index` 0 and `count` equal to the current item count, omitted when the series is empty;
  2. `MODEL_INSERT` at `index` 0 with the new items, omitted when there are none.

  The delete's count form removes every item on both sides: Rust `Model::delete_items` and Swift `deleteItems` remove [`index`, `index` + `count`) and lower the count by `count`. A count of 0 is a generic no-op, which T5 refuses. Both operations are within `max_items_per_model_operation` (≤ 900 items). Reconnect needs no replace: a snapshot carries the current contents and `RESUME_OK` replays the appends (§18).
- Eviction is the server's. A client never evicts, trims or resamples its replica; it may draw fewer points than it holds.
- A `SeriesSet` accepts `MODEL_INSERT` at any index (legend order), `MODEL_DELETE` by index and a count ≥ 1, `MODEL_RESET_RANGE` in the fill form only, and `MODEL_UPDATE` that changes only `series_label`, `subject_label`, `denominator` or `interval_ms` (D3). Replacing the whole set is delete-then-insert, as for a series. A different `series_key`, unit, axis, kind, subject or slot is a different series: a new descriptor with a new series id.

### 5.4 Units, denominators and axes

- **Units** (closed set, D2/G2): `percent`, `bytes`, `bytes_per_second`, `operations_per_second`, `packets_per_second`, `count`, `seconds`, `hertz`, `degrees_celsius`, `watts`. Bits per second do not exist (dashboard.md section 5). An unknown unit is refused, never guessed.
- Every series names its own `unit`, which must equal the unit of the axis it is on (D2). A graph has a `primary_axis` and at most one `secondary_axis`, so one graph shows at most two units (fixture 02).
- **`denominator`** (optional, ≤ 64 bytes) is the words that make a share or level complete: `of 8 logical CPUs`, `of 15.5 GiB`. It is server text; the client shows or reads it, never computes it.
- **Axis intent** (`AxisSpec.axis_intent`):
  - `share`: values in [`range_min`, `range_max`] with `range_min` exactly 0.0 and a finite `range_max` > 0 (100 for a percentage of one thing, total memory for bytes used); both are required.
  - `zero_based`: values ≥ 0; the client chooses the upper bound from the values it shows (auto-scaling, never below the largest shown value); no range properties.
  - `signed`: any finite value; the client chooses a range that includes zero; no range properties.
- Axis intent is meaning, not geometry: tick count, labels, log or linear drawing, colour and density are the client's.

### 5.5 Negative values

Only a `signed` axis admits a negative value. On a `share` or `zero_based` axis any value whose sign bit is set is refused, `-0.0` included, so Rust (`f64::is_sign_negative`) and Swift (`Double.sign == .minus`) reach the same verdict (T3). NaN, ±infinity and |v| > 10^18 are refused on every axis (T2). A share above `range_max` is refused, not clamped; the server rounds or clamps its own arithmetic before publishing (fixture 03). The same rules apply to a gauge's `metric_value` against its `primary_axis` (U2).

### 5.6 Identity

- **Series id** — the descriptor's `item_id`: server-assigned, unique within the session, never reused for another series, even after the series is removed (D1).
- **`series_key`** — a stable name, unique within its graph, matching `^[a-z0-9][a-z0-9._:/-]{0,63}$` (for example `host/net/eth0/rx`). It is what a saved view (PX-069) may store to restore "Show series" for a device.
- **`subject_key` / `subject_label`** — the stable identity and display name of what is measured: `host` for an aggregate, a device or interface key for a device series (for example `disk:nvme-eui.0025388b91b2c4a1`), a `ProcessKey`-derived key for PX-090 history. The key is assigned by the collector ticket that owns the source (PX-055 devices, PX-056 interfaces, PX-090 processes) and is never a PID, row index or display name alone.
- **Host identity is not profile data.** The host, account and session incarnation a graph belongs to are named by PX-095's client-owned window chrome (`client-macos/RendererAppKit/SessionWindowIdentity.swift`); no profile string may imitate it.

### 5.7 Staleness and transport state

Data staleness is a server fact, published as a `stale` gap (series) or `stale` state (gauge). A client never infers data staleness from local time and never ages a graph on its own clock. Transport staleness (disconnected, resynchronizing) is PX-022-G01's client chrome: while stale, the window keeps the last applied revision, graphs stay as applied, and nothing is extrapolated (dashboard.md section 4).

## 6. Gauge semantics

- `metric_state` is one of `ok`, `warming_up`, `unavailable`, `denied`, `not_provided`, `none_configured`, `collector_error`, `stale`, `paused` (U1).
- `ok` requires `metric_value`. `stale` and `collector_error` may keep the last good `metric_value`, shown as such with its own sample time. Every other state carries no `metric_value`: an unknown figure never becomes zero (fixture 04).
- `sample_unix_ms` (the source read time) is present exactly when `metric_value` is.
- `primary_axis` gives the gauge's unit and intent; a `share` gauge may be drawn as a level indicator, any other as a number.
- The dashboard's compact presentation keeps PX-070's standard meters (dashboard.md section 4); `Gauge` is for later panels and other applications and is not required by the core dashboard.

## 7. Limits and encoding

### 7.1 Limits

Every limit is checked identically on both sides; raising one is a new major version (N3).

| Constant | Value | Rule |
| --- | --- | --- |
| `MAX_SERIES_PER_GRAPH` | 18 (dashboard.md: 2 aggregate + 2 × 8 devices) | D1 |
| `SLOTS_PER_GRAPH` | 18 `SampleSeries` models, fixed at creation | S3 |
| `MAX_SAMPLES_PER_SERIES` | 900 (dashboard.md `MAX_SERIES_SAMPLES`) | T6 |
| `window_ms` | 1,000 … 900,000 (15 minutes) | G1 |
| `interval_ms` | 50 … 300,000 (srtop's `MIN_`/`MAX_REFRESH_INTERVAL`) | D1, U1 |
| sample offset | < 2^40 | T1 |
| sample magnitude | finite, ≤ 10^18 | T2 |
| `MAX_GRAPHS_PER_SESSION` | 16 `SeriesGraph` nodes created over the session's lifetime | S4 |
| `MAX_GAUGES` | 64 live `Gauge` nodes | S4 |
| strings | `label` 1–64, `value_description` 1–512, `series_label` 1–32, `subject_label` 1–64, `denominator` 1–64 bytes; keys ≤ 64 bytes | S2, D1, U1, G1 |
| `MAX_FALLBACK_SAMPLES` | 60 per series (dashboard.md) | F2 |
| `MAX_FALLBACK_ROW_BYTES` | 64 bytes on the wire (dashboard.md) | F3 |
| fallback row ids | < 2^35 (60 years of rows at 18 rows/s) | F3 |

The profile's own models per session are at most 16 × 20 = 320, inside the default §26 `max_model_count` of 1,000 (`DEFAULT_MAX_MODEL_COUNT`, `server-rust/semantic-tree/src/store/limits.rs`). One graph's snapshot needs at most 1 + 18 + 1 model operations with items: the `SeriesSet` with ≤ 18 items, each slot with ≤ 900, and the fallback model with ≤ 18 × 60 = 1,080, all inside `max_items_per_model_operation` (10,000). Every §26 limit (frame, node count, depth, string length, operations per transaction) still applies to profile content as to any content.

### 7.2 Encoded sizes

Measured with `protoc --encode` (libprotoc 29.3) against this checkout's `protocol/srui.proto`, with every field at its cap and a one-byte session namespace number. Each size includes the field tag and length prefix of the container it travels in: an item inside its operation's `items`, an operation inside `Transaction.operations` (logs `impl1-encoding-sizes.log`, `impl2-encoding-sizes.log`):

| Item | Bytes on the wire |
| --- | --- |
| Sample in a snapshot or append (`ModelItem` in an operation's `items`, offset 2^40 − 1, any float64) | **20** |
| Gap item (reason code, same offset) | 13 |
| Fallback row at the cell caps (14 + 8 + 17 bytes, row id 2^35 − 1) | **64** |
| Series descriptor at every cap (in `items`) | 475 |
| `SeriesGraph` `CREATE_NODE` at every cap (in `Transaction.operations`) | 1,170 |
| `Gauge` `CREATE_NODE` at every cap (in `Transaction.operations`) | 1,063 |
| `CREATE_MODEL` (in `Transaction.operations`) | ≤ 24 |
| Item-carrying operation header, measured with its full payload (900 samples, 1,080 fallback rows or 18 descriptors), excluding the items | ≤ 22 |

**Effect on dashboard.md section 7.** Its assumption of at most 24 bytes per sample holds (20). Recomputed with these sizes, everything the profile adds for the 39 host series is 723,479 bytes (series items 39 × 900 × 20 = 702,000; descriptors 39 × 475 = 18,525; 80 `CREATE_MODEL`s 1,920; 47 item-operation headers 47 × 22 = 1,034), against the 842,400 the dashboard charged; the fallback rows stay exactly 39 × 60 × 64 = 149,760. The four graph nodes exceed the dashboard's 640-byte node estimate by at most 4 × 530 = 2,120 bytes. `DASHBOARD_RESERVE_BYTES` (1,179,648) therefore still covers its sum, now at most 1,002,335 bytes. For PX-090 history (4 graphs of 4 series) the profile adds 298,048 bytes (288,000 items, 7,600 descriptors, 1,920 models, 24 headers 528) plus 61,440 fallback and 4,680 for the graph nodes: 364,168 against `HISTORY_RESERVE_BYTES` (458,752). PX-073-G01 still recomputes from its real nodes, as that section requires.

### 7.3 Steady-state traffic, and an open budget finding

Per series per sample the server sends one append and, at steady state, one eviction, plus one fallback-row insert and one fallback-row delete (each operation including its 2-byte slot in `Transaction.operations`):

| | Append | Evict | Fallback insert | Fallback delete | Per series per sample | 39 host series at 1 s | Default layout (3 series) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Typical (model ID < 2^14, offset about one day, 12/8/12-byte cells) | 28 | 9 | 65 | 12 | 114 B | 4,446 B/s | 342 B/s |
| Worst (every field at its cap) | 38 | 17 | 82 | 20 | 157 B | 6,123 B/s | 471 B/s |

**Finding PX071-F1 (budget; follow-up [aizlabs/srui#128](https://github.com/aizlabs/srui/issues/128)).** With every panel expanded and 8 devices and 8 interfaces showing series, the series and their mandatory fallback rows alone are about 4,446 B/s, before panel text, gauges and transaction envelopes; dashboard.md section 7 fixes the steady-state bound at 4 KiB/s (4,096 B/s) for that configuration and PX-073-G02 uses it as a pass/fail threshold. The fallback rows are 77 of the 114 bytes, and they cannot be thinned without breaking the dashboard's own requirement that they equal the published samples (E2(b)) and are always carried (§11.1, one tree for every client). This ticket does not change the dashboard specification. The user decided on 2026-11-28 to record it as a follow-up (option (a)), tracked as issue #128: PX-073-G01 recomputes the figure from the real encoding and amends dashboard.md section 7's bound (likely to 8 KiB/s) before PX-073-G02 measures. The PX-073-G01 ticket carries a note pointing here. The default layout is about 342 B/s.

## 8. Standard Widget fallback

The server always publishes each profile node's fallback, for every client, because a session has one semantic tree (§11.1; dashboard.md section 7). A client that implements the profile suppresses the fallback (`ExtensionMountResolver.isSuppressedFallbackNode`); a base client mounts it (`ExtensionMountDecision.fallback(roots:)`). The fallback must carry the whole meaning in standard widgets and words; it never names a process as responsible for host activity (dashboard.md section 5).

### 8.1 Gauge fallback (S1)

Exactly one child: a standard `Text` whose text is the gauge's complete sentence (≤ 512 bytes: label, value with unit and denominator, or the state in words, and the sample time and interval), or a `Row`/`Column` holding exactly that `Text` followed by one `Progress` whose value is the share (for `share` gauges). Example: `CPU: 37.4% busy of 8 logical CPUs, sampled 08:00:01, every 1 s`.

### 8.2 SeriesGraph fallback: one bounded `Table` of recent samples (S1, F1–F4)

Exactly one child: a standard model-backed `Table` with no children, backed by a standard model of type `Table`, and nothing else references the graph's profile models.

- **F1 — columns.** `columns` = `["Series", "Time", "Value"]`; `selection_mode` `none`; `label` names the history (`CPU history samples`); `accessible_description` states the order and the gap words: "One row per sample, oldest first within each series. A gap row names its state and lasts until the next row of its series; error means collector error and unconfigured means none configured."
- **F2 — rows.** For each descriptor in `SeriesSet` order, the newest min(60, n) items of its series, oldest first: series-major, so a VoiceOver user reads one device's trend without skipping other devices' rows. A sample and a gap are each one row. Rows change in the same transaction as the samples they mirror: insert the new row at the end of its series' block, delete the series' oldest row when it falls out of the newest 60, and delete a removed series' rows with its descriptor. On a series replace, `MODEL_DELETE` the series' block (its first index and its row count) and `MODEL_INSERT` the new rows there. The fallback model is a standard `Table` model, so the same generic-semantics argument as section 5.3 applies: never `MODEL_RESET_RANGE` to shorten or empty it.
- **F3 — cells.** Each row's item value is a list of three strings:
  - **Series** ≤ 14 bytes: the `series_label` if it is at most 14 bytes. Otherwise it is cut on **Unicode scalar boundaries**: the longest prefix of whole scalars of at most 7 UTF-8 bytes, then `…` (U+2026, 3 bytes), then the longest suffix of whole scalars of at most 4 bytes. Every scalar is 1–4 bytes, so both parts are non-empty and the cell is at most 14 bytes and always valid UTF-8. Examples, all in fixture 07's `series_cells` table:
    - `br-4f1c2d3e5a6b rx` → `br-4f1c…b rx`
    - `网络流量接收速率` (24 bytes) → `网络…率`
    - `Netzwerk Empfang über` → `Netzwer…ber`
    - `Taux d’écriture` → `Taux d…ture`
    - `receive wlan0 📶` → `receive…📶`
    - `wlan0 📶 receive` → `wlan0 …eive`

    D1's distinctness rule compares these cut cells: `网络流量接收速率` and `网络流量发送速率` both cut to `网络…率`, so they are refused together (fixture 07 `cjk-series-cells-collide`). Scalars rather than extended grapheme clusters, because a scalar boundary is defined by UTF-8 alone. Rust checks it with `str::is_char_boundary` or `chars()`; Swift with `String.unicodeScalars`. Grapheme segmentation follows each platform's Unicode version tables (Rust crates against Swift `Character`) and could cut differently. The cost is that a combining sequence or emoji ZWJ sequence may be cut inside a grapheme; the full label remains in the descriptor;
  - **Time** exactly 8 bytes, `HH:MM:SS` of `epoch_unix_ms + offset`, in the same presentation time zone as the panel's freshness line;
  - **Value** ≤ 17 bytes: the sample formatted per section 8.3, or the gap word and its duration.
  With row ids below 2^35 a row is at most 64 bytes on the wire (section 7.2), the dashboard's `MAX_FALLBACK_ROW_BYTES`.
- **F4 — gaps.** A gap row's Value is its word (section 5.2 table) followed by a space and its duration, truncated to whole units: `Ns` below 1 minute, `Nm` below 1 hour, `Nh` below 100 hours, otherwise `>99h` (`paused 3s`, `unavailable 12m`). An ongoing gap shows the word alone; the server updates that row (an ordinary `MODEL_UPDATE` of a standard model) when the gap ends.

Fixture 01's fallback after five reads:

| Series | Time | Value |
| --- | --- | --- |
| CPU busy | 08:00:00 | warming up 1s |
| CPU busy | 08:00:01 | 12.5% |
| CPU busy | 08:00:02 | 37.4% |
| CPU busy | 08:00:03 | 100.0% |
| CPU busy | 08:00:04 | 0.0% |

### 8.3 Formatting (normative for fallback text and accessibility values)

The same table produces the fallback Value cells, the server's sentences and a capable client's accessibility values, so a VoiceOver user hears the same text with or without the profile. Axis tick labels are local presentation and may use the locale.

- Round the exact binary value to the stated decimals, ties to even (Rust `format!("{:.1}")`, Swift `String(format: "%.1f")`); a value that rounds to zero has no sign (`-0.0` and `-0.04` give `0.0`). Scaling divides by the exact power of the step; if the rounded result reaches the step, use the next prefix.
- `percent`: one decimal and `%` below 10,000 (`37.4%`, `6400.0%`), then SI prefixes (`12.8k%`).
- `bytes`: whole bytes below 1,024 (`512 B`), else IEC with one decimal, `KiB` … `EiB` (`5.2 GiB`); `bytes_per_second` adds `/s` (`1.2 MiB/s`).
- `operations_per_second`, `packets_per_second`, `seconds`, `degrees_celsius`: one decimal below 10,000, then SI `k M G T P E` attached to the number (`9999.9 ops/s`, `12.3k ops/s`, `14.9M packets/s`, `12.3 s`, `-3.5 °C`).
- `count`: whole number below 10,000 (`42`), then SI (`25.6k`).
- `hertz`, `watts`: one decimal below 1,000, then SI before the unit (`50.0 Hz`, `2.4 GHz`, `35.2 W`, `1.2 kW`).

With |v| ≤ 10^18 the longest result is 17 bytes (`-9999.9 packets/s`). Fixture 02 holds the parity tables (26 formatting rows, 8 duration rows) that the Rust and Swift formatters must both reproduce.

## 9. Accessibility summaries

- **A1 — server summary.** Every profile node carries `label` (what it is: `Network history`) and `value_description` (≤ 512 bytes: the current values of its aggregate series or the gauge's figure, with unit, denominator, sample time, interval and window, as complete words), maintained by the server. That summary is required because 18 series cannot all be named in 512 bytes; it covers the aggregates, and per-series detail comes from A2 or the fallback.
- **A2 — capable client.** A client that implements the profile exposes each `SeriesGraph` as one accessibility container labelled `label` with value `value_description`, containing one element per series whose label is `series_label` (with `subject_label` where it adds information) and whose value is its newest item formatted per section 8.3, or its gap word and duration, followed by its sample time; it also states each axis's unit and, for `share`, its range. A gap is announced, never read as zero. The graph or a numeric alternative is keyboard-reachable (PX-073: "VoiceOver or the accessibility API can obtain units, current values, series names, and unavailable state"). A `Gauge` is one element with label `label` and value `value_description`.
- **A3 — base client.** The fallback (section 8) is the accessible alternative: sentences in `Text` and the sample history in a `Table`, in the words section 8.3 defines. Every dashboard evaluation task is completable from it (dashboard.md section 11.3).
- Reduced motion, animation and density are client-local; the profile has no animation, cadence or frame concept (§12.2).

## 10. Validation, failure and implementability

### 10.1 Rules

| ID | Rule |
| --- | --- |
| N1–N4 | negotiation (section 3.2) |
| S1 | each profile node has exactly one child, of the section 8 shape; a fallback subtree holds standard widgets only (no profile node inside a fallback) |
| S2 | only the admitted properties on each carrier (section 4.2); required properties present; value kinds as tabled; no profile property on a standard node; profile node types are only `Gauge` and `SeriesGraph` |
| S3 | a graph's `model_ref` names a `SeriesSet`; `sample_models` lists exactly 18 distinct `SampleSeries` models; these 19 models are referenced by that graph only; `epoch_unix_ms`, `sample_models` and `model_ref` cannot be set again or cleared; a profile model no graph owns holds no items |
| S4 | at most 16 `SeriesGraph` nodes created in a session's lifetime; at most 64 live `Gauge` nodes |
| G1 | `window_ms` in [1,000, 900,000]; `epoch_unix_ms` > 0; `label` and `value_description` lengths |
| G2 | `AxisSpec`: known unit and intent; `share` has `range_min` 0.0 and finite `range_max` > 0; other intents have no range |
| D1 | ≤ 18 descriptors; descriptor value `null`; required properties and lengths; `series_key` grammar and uniqueness in the graph; series id never reused in the session; `samples_model` is a distinct slot of the graph; `interval_ms` range; `sample_kind` token; shortened Series cells distinct |
| D2 | `series_axis` names an axis the graph has; known unit equal to that axis's unit |
| D3 | a descriptor update keeps its series id and changes only `series_label`, `subject_label`, `denominator`, `interval_ms` |
| D4 | an unbound slot holds no items; a slot whose series changed in a transaction holds only items appended after it was emptied in that transaction |
| T1 | offsets in [0, 2^40) and strictly increasing |
| T2 | an item is a finite float64 sample with \|v\| ≤ 10^18 or a gap code 1–8, with no item properties |
| T3 | sample values satisfy their series' axis (section 5.5) |
| T4 | no two consecutive gap items with the same code |
| T5 | append-only, front-evict-only (count ≥ 1), fill-only `MODEL_RESET_RANGE` on `SampleSeries`; `MODEL_UPDATE` refused (section 5.3); fill-only reset and index-form delete with count ≥ 1 on `SeriesSet` |
| T6 | ≤ 900 items at every operation boundary (a `SampleSeries` created with `item_count` > 900 is refused, and a `SeriesSet` created with more than 18 under D1); exact age retention at the end of each transaction |
| T7 | every `SeriesSet` and `SampleSeries` is dense at the end of each transaction (section 5.3) |
| U1, U2 | gauge state/value/sample-time coupling (section 6); value against `primary_axis` |
| F1–F4 | the fallback rows (section 8.2): server obligations, checked by PX-072's tests, not by clients |

### 10.2 Where rules are enforced

- **Authoritative side.** PX-072's SDK refuses to build profile content that breaks any rule, and the session refuses a commit that touches a profile node or model and breaks one, before it is journaled or broadcast — the same place the store already checks `Split` pane counts and `shortcut` admission. A refused commit has no effect (§12.1).
- **Capable client.** A client that implements the profile checks S, G, D, T and U rules on the staged replica of every transaction that touches a profile node or model, at the same operation index or transaction end as the server, and refuses the whole transaction with `metrics_profile_violation` and the rule ID (`metrics_profile_violation:T6`). That is a fail-closed apply failure handled by the existing path (§4 inv. 13, §26 "malformed tree operations fail the transaction rather than corrupt state"). A correct server never triggers it.
- **Base client.** Does not check profile rules (it does not implement the profile, N3). It stores profile nodes, models and properties opaquely under the generic store and §26 limits, and mounts the fallback; an extension node without a child still fails at mount as today (`unsupportedExtensionWithoutFallback`). Fixture 07.

If PX-072 or PX-073 finds that the Rust store or the Swift applier has no hook for an extension's apply-time rules, adding that generic hook is a narrowly scoped prerequisite ticket with conformance tests (standing contract), not part of this profile.

### 10.3 Evaluation order (normative for the reported rule ID)

A transaction that breaks any rule is refused whatever the order. The order fixes *which* rule ID is reported, so Rust and Swift report the same `metrics_profile_violation:<rule>`. The first check that fails is reported.

1. **Per operation, in operation order:**
   1. *Profile operation rules*, against the replica as it is before the operation: S4 (creating a 17th `SeriesGraph`); T6 or D1 (creating a profile model sized beyond its cap); S3 (setting or clearing an immutable graph property, even to the same value); T5 (operation shape on a `SampleSeries` or `SeriesSet`, including the fill form); D3 (descriptor update); D1 (series-id reuse); then, for each item in order, T1 (offset range, and strictly increasing against the held neighbours of the written range), T2, T4.
   2. *Generic checks and apply*: node, parent and model existence, duplicate node or item IDs, §26 limits. These are the checks every client runs, with or without the profile.
   3. *Profile counts* after the operation: T6 (≤ 900 items) and D1 (≤ 18 descriptors).
2. **At the end of the transaction**, first T7 for every profile model in ascending model ID, then every node in ascending node ID:
   - a standard node holding a profile property: S2;
   - a `Gauge`: S2, S1, G2 (its axis), U1 (state and value presence), U2, U1 (sample time, interval, subject), G1 (label lengths);
   - a `SeriesGraph`: S2, S1, G1, G2, S3 (references and ownership); then each descriptor in `SeriesSet` order, D1 then D2; then D1 (distinct Series cells); then each slot in `sample_models` order, D4, T3, T6 (age).
3. **Last**, S4 (live gauge count), then S3 (profile models no graph owns that hold items).

A base client runs only step 1.2 and the §11.1 mount rule. Every refusing fixture case but one breaks exactly one rule, so its expected ID does not depend on this order. This was checked by re-running each case with its expected rule disabled (completion note, `impl3-fixture-isolation.log`). The exception is `reset-range-cannot-shrink-a-series`: a shrinking reset necessarily also leaves the series sparse (T7), and the order reports T5. Implementations follow the order for any content that breaks several rules.

### 10.4 Implementable without AppKit

Every rule is stated over SRUI types both sides already have without a UI toolkit: `TypeRef`, `PropertyRef`, `Value` (float64, uint64, string, list, small record), `Model` items and the §13 model operations, plus `f64`/`Double` sign and finiteness tests and integer comparisons. Rust implements them beside `semantic-tree`'s store checks (PX-072); Swift implements them in a target that does not import AppKit, under the same rule `SemanticModel` follows (PX-073), so both run in tests without a window. As evidence for this ticket, an independent scratch implementation in Python with no UI dependency applies every fixture under both the capable and the base rules (completion note).

## 11. Schema review

The six cases the ticket names, each with the rules and fixture cases that decide it.

| Case | Decision | Fixture cases |
| --- | --- | --- |
| **Units** | Closed unit set, one unit per series equal to its axis's unit, at most two axes per graph, denominators as server words, one normative formatter for both sides; `bits_per_second` does not exist | 02: `series-unit-differs-from-axis`, `unknown-unit-bits`, `secondary-axis-missing`, `secondary-axis-with-its-own-unit`, `share-axis-without-range`, `share-axis-not-starting-at-zero`, `zero-based-axis-with-range`, `unknown-axis-intent`; 26 formatting and 8 duration parity rows |
| **Negative values** | Only `signed` axes; the sign bit decides (−0.0 refused on non-negative axes); non-finite and > 10^18 refused everywhere; share above range refused, not clamped; counter reset is a `warming_up` gap | 03: `signed-axis-accepts-negative`, `share-above-range-max`, `share-at-range-max`, `zero-based-negative`, `zero-based-negative-zero`, `nan`, `infinity`, `magnitude-above-1e18`, `gauge-negative-on-share`, `counter-reset-is-a-warming-up-gap` |
| **Missing samples / gaps** | A gap is an item with a reason code lasting until the next item; no `null`, string, zero or interpolation; one item per gap start; staleness is a server gap; gauge states without values | 04: `repeated-gap-reason`, `error-then-stale`, `recovery-after-error`, `unknown-gap-reason`, `string-is-not-a-gap`, `null-is-not-a-gap`, `sample-with-properties`, `gauge-unavailable-with-value`, `gauge-ok-without-value`, `gauge-value-without-sample-time`, `gauge-stale-keeps-last-good-value` |
| **Window bounds** | 18 series, 900 samples at every operation boundary, exact age retention, append/evict/replace shapes, offsets < 2^40, immutable epoch and slots, slot reuse emptied, 16 graphs | 05: 30 cases from `append-at-cap-without-eviction` to `sixteenth-graph-in-a-session` (accepted) and `seventeenth-graph-in-a-session` (refused), including `replace-by-delete-then-insert`, `reset-range-cannot-shrink-a-series`, `reset-range-cannot-empty-a-series`, `series-set-replace-by-reset`, `snapshot-fill-form` and `fill-leaves-a-hole` with the base client's resulting model state asserted |
| **Profile-version mismatch** | Exact-major activation, one major per session, optional only; a non-implementing client is a base client and refuses nothing | 06: 8 negotiation scenarios and `v2-only-client-receives-v1-content` |
| **Unsupported-client fallback** | One standard child of a fixed shape; base clients accept profile content opaquely and mount it; a missing fallback still fails at mount; shortened Series cells distinct | 07: `graph-without-fallback`, `graph-fallback-is-not-a-table`, `graph-fallback-is-a-list`, `fallback-table-on-a-profile-model`, `gauge-inside-a-fallback`, `gauge-fallback-text-and-progress`, `profile-property-on-a-standard-node`, `standard-property-not-admitted-on-a-graph`, `shortened-series-cells-collide`, `multilingual-series-cells`, `cjk-series-cells-collide`; 8-row `series_cells` table (ASCII, 2-byte Latin, 3-byte CJK and punctuation, 4-byte emoji) |

Worked example (fixture 04, gaps). The series receives: `0` warming_up, `1000` 1258291.2, `2000` paused, `5000` warming_up, `6000` 87040.0, `7000` collector_error. A capable client draws two separated segments of one point each and no line across 2000–6000; the fallback shows `warming up 1s`, `1.2 MiB/s`, `paused 3s`, `warming up 1s`, `85.0 KiB/s`, `error`. Appending a second collector_error at `8000` is refused (T4); appending `stale` at `10000` is accepted and the server updates the error row to `error 3s`.

## 12. Golden fixtures

[`metrics-profile-fixtures/`](metrics-profile-fixtures/) holds eight JSON documents. They are documentation fixtures in a semantic notation, not wire bytes; PX-072 and PX-073 turn them into Rust and Swift tests (and, if they choose, `.bin` vectors under the normal manifest process).

| File | Purpose |
| --- | --- |
| `01-negotiated-cpu-graph.json` | positive path: negotiation, one share series, appends, fallback rows |
| `02-units-and-axes.json` | units, axes, denominators; formatting and duration parity tables |
| `03-negative-values.json` | negative values and non-finite values |
| `04-gaps-and-staleness.json` | gaps, staleness, gauge states |
| `05-window-bounds.json` | every count, age, offset and operation-shape bound |
| `06-profile-version-mismatch.json` | negotiation and version mismatch |
| `07-unsupported-client-fallback.json` | base-client behavior and fallback shapes |
| `08-independent-panels.json` | independent timestamps and epochs |

**Notation.** A fixture has `fixture`, `profile`, `purpose`, `rules`, and any of `negotiation` (scenarios with `expect` one of `native`, `fallback`, `server_refuses:N1`, `handshake_fails:unmapped_profile`), `setup` (transactions applied in order; every one is accepted by both clients), `cases` (each applied alone on top of `setup`, with `expect.capable` and `expect.base` of `accept`, `refuse:<rule>` or `generic:<reason>` for a failure every client reaches without the profile, and optional `capable_state`/`base_state` lists asserting a model's resulting `item_count` and number of held items after generic apply), `formatting`/`durations`/`series_cells` (parity rows) and `render` (prose). Operations are `create_node` (`id`, `parent`, `type`, `props`), `set_property`, `clear_property`, `create_model` (`model`, `type`, `item_count`), `model_insert`, `model_delete`, `model_update` and `model_reset_range` with the §13 fields. Values are `{"float": x}` (with `"NaN"`, `"Infinity"`, `"-Infinity"`, `"-0.0"` spelled as strings), `{"uint": n}`, `{"string": s}`, `{"null": true}`, `{"list": [...]}`, `{"record": {"type", "props"}}`, `{"enum": "Enum.value"}`. Three shorthands: `{"gap": "<reason>"}` is the reason's uint64 code; `{"uint_range": [a, b]}` is the list of uints a…b; `create_model` with `model_range: [a, b]` is one `create_model` per id; `items: {"repeat": {count, first_id, id_step, value}}` is that many items. Fixture times use the epoch `1800000000000` (2027-01-15T08:00:00Z) and present in UTC; the profile namespace is session namespace 2 except where 06 shows otherwise.

## 13. Obligations on later tickets

- **PX-072** registers the profile with `register_optional_extension_profile` only for sessions whose application asks for it, implements the bounded series store with exact retention, one item per read, gap items, slot reuse and the fallback rows (F1–F4) and formatting (section 8.3), refuses invalid content (section 10.2) in the section 10.3 order, replaces a series or series set only by delete-then-insert (section 5.3), keeps every profile model dense, and turns every fixture into a Rust test, including the formatting parity table. It also adds the N1 refusal at registration, which `register_optional_extension_profile` does not have today: a second major of the same profile name, or the profile as required, is refused (fixture 06 `server-advertises-two-majors`, `server-requires-the-profile`). Its "unsupported clients receive a valid fallback subtree" is met by always carrying the fallback.
- **PX-073** registers `Gauge` and `SeriesGraph` as native kinds only when the profile is active (N2), validates in a non-AppKit target in the section 10.3 order (section 10.4), draws locally with no interpolation across gaps and no extrapolation (the right edge is the graph's newest offset), implements A2, the formatter and the scalar-boundary Series cell (F3), and turns every fixture into a Swift test. It verifies on a real client that a base client stores the profile's models and properties opaquely (fixture 07 assumes the generic stores accept non-standard model types and property references, as Swift `SemanticStore.createModel` and Rust `SemanticStore::set_property` read today: neither checks the namespace).
- **PX-073-G01** recomputes the reserves (section 7.2 gives the per-item sizes). It owns finding PX071-F1 (issue #128): it recomputes steady-state traffic from the real encoding and amends dashboard.md section 7's bound (likely to 8 KiB/s) before PX-073-G02 measures.
- **PX-090** uses one `SeriesGraph` per watched process (4 series), reusing graphs by rebinding slots when a watched process changes, within S4.

## 14. Assumptions and decisions

- **A1 — canonical name `io.github.aizlabs.srui.metrics/1`. Approved by the user on 2026-11-28 ("yes keep the name").** It is derived from the project's GitHub organization (section 3.1). From now on, any other name is a new profile.
- **A1a — PX071-F1 is a recorded follow-up, not a PX-071 change. Decided by the user on 2026-11-28 (option (a)).** dashboard.md is unchanged here. Issue [#128](https://github.com/aizlabs/srui/issues/128) tracks the follow-up; PX-073-G01 owns it (section 13).
- **A2 — fixed 18 slots per graph.** Chosen because the wire has no model deletion; it costs 18 empty `CREATE_MODEL`s per graph and bounds models per session at 320.
- **A3 — the offset is the sample's item id.** Chosen for stable identity and a 20-byte sample; the timeline is monotonic from the graph's epoch.
- **A4 — gap reasons are uint64 codes.** A string reason would cost up to 28 bytes per gap item and break the dashboard's 24-byte-per-item reserve; the codes are fixed by the table in section 5.2.
- **A5 — series-major fallback rows.** Chosen for VoiceOver reading of one device's trend; a time-major order would batch one insert and one delete per graph per tick and save about 20 bytes per series per sample, which does not by itself resolve PX071-F1.
- **A6 — fallback words.** `error` and `unconfigured` stand for `collector error` and `none configured` so that every gap row fits the dashboard's 64-byte row; the Table's description spells them out.
- **A7 — no minor versions.** Every change is a new major (N3), so no client interprets content it does not fully understand.
- **A8 — replace is delete-then-insert; `MODEL_RESET_RANGE` only fills** (PR #129 review). A reset cannot shorten or empty a model under the existing generic semantics on either side, and the profile adds no protocol change. The snapshot exporter's existing fill form stays valid.
- **A9 — Series cells cut on Unicode scalar boundaries** (PR #129 review). This is deterministic across Rust and Swift; grapheme boundaries depend on Unicode tables (section 8.2 F3).
