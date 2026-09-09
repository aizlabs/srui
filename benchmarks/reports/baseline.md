# SRUI benchmark report

- Generated: 2026-09-08T23:55:46.472585+00:00
- Profile: full
- Host: macOS-26.4.1-arm64-arm-64bit-Mach-O / arm64
- Fixture: benchmarks/fixtures/coding-agent-ui.json

## §31.1 Local renderer

| Metric | Value | Statistic | Target |
|---|---:|---|---:|
| SRUI first visible paint | 20.04 ms | p50 | — |
| SRUI first visible paint | 23.06 ms | p95 | — |
| SRUI complete paint | 20.07 ms | p50 | — |
| SRUI complete paint | 23.1 ms | p95 | — |
| SRUI complete paint | 25.92 ms | p99 | — |
| SRUI CPU time | 16.81 ms | p50 | — |
| SRUI CPU time | 18.96 ms | p95 | — |
| SRUI live allocation delta | 1.21e+04 allocations | p50 | — |
| SRUI process resident peak | 80.27 MiB | p50 | — |
| renderer short-soak net heap growth | 10.82 MiB | last-first | — |
| WKWebView first visible paint | 0.5378 ms | p50 | — |
| WKWebView complete paint | 0.9415 ms | p50 | — |
| WKWebView host-process CPU time | 1.027 ms | p50 | — |
| WKWebView host-process live allocation delta | 0 allocations | p50 | — |
| WKWebView host-process resident peak | 116.8 MiB | p50 | — |
| SRUI representation | 890 bytes | exact | — |
| HTML representation | 1082 bytes | exact | — |

Assertions:

- **PASS** representative fixture mounts all nodes — 20 AppKit render handles
- **PASS** warm WKWebView completed representative load — first progress and navigation completion observed

Notes:

- Full profile forces AppKit display after layout.
- Live allocation delta is a process allocator sample; authoritative allocation attribution uses the documented xctrace full-profile command.
- WKWebView host-process CPU and memory exclude WebContent helpers; the Instruments trace provides cross-process attribution.
- A positive short-soak heap delta is an Instruments follow-up signal, not by itself a leak classification.

## §31.2 Serialization

| Metric | Value | Statistic | Target |
|---|---:|---|---:|
| abstract state generation | 0.002625 ms | p50 | — |
| abstract state generation | 0.003875 ms | p95 | — |
| abstract state generation | 0.008708 ms | p99 | — |
| protobuf serialization | 0.01004 ms | p50 | — |
| protobuf serialization | 0.01408 ms | p95 | — |
| protobuf serialization | 0.0225 ms | p99 | — |
| serialized transaction size | 890 bytes | exact | — |

Assertions:

- **PASS** shared fixture produced SRUI protobuf — 20 nodes encoded into 890 bytes
- **PASS** renderer and serializer consume the same abstract state — 890 bytes on both paths

Notes:

- PTY spawn and renderer work are excluded from server serialization timing.

## §31.3 Mutation and frame independence

| Metric | Value | Statistic | Target |
|---|---:|---|---:|
| 1 updates semantic decode/apply | 0.007625 ms | p50 | — |
| 1 updates semantic decode/apply | 0.01537 ms | p95 | — |
| 1 updates semantic decode/apply | 0.1051 ms | p99 | — |
| 1 updates decode-to-visible | 8.353 ms | p50 | — |
| 1 updates decode-to-visible | 8.758 ms | p95 | — |
| 1 updates wire bytes | 23 bytes | exact | — |
| 1 updates message count | 1 messages | exact | — |
| 100 updates semantic decode/apply | 0.08775 ms | p50 | ≤ 1 ms |
| 100 updates semantic decode/apply | 0.09817 ms | p95 | — |
| 100 updates semantic decode/apply | 0.1085 ms | p99 | — |
| 100 updates decode-to-visible | 8.465 ms | p50 | — |
| 100 updates decode-to-visible | 9.204 ms | p95 | — |
| 100 updates wire bytes | 2102 bytes | exact | — |
| 100 updates message count | 1 messages | exact | — |
| 1000 updates semantic decode/apply | 0.7884 ms | p50 | ≤ 5 ms |
| 1000 updates semantic decode/apply | 0.831 ms | p95 | — |
| 1000 updates semantic decode/apply | 0.8591 ms | p99 | — |
| 1000 updates decode-to-visible | 9.813 ms | p50 | — |
| 1000 updates decode-to-visible | 10.45 ms | p95 | — |
| 1000 updates wire bytes | 2.1e+04 bytes | exact | — |
| 1000 updates message count | 1 messages | exact | — |
| 60Hz wire bytes | 598 bytes | exact | — |
| 60Hz message count | 24 messages | exact | — |
| 60Hz repaint count | 6 repaints | exact | — |
| 120Hz wire bytes | 598 bytes | exact | — |
| 120Hz message count | 24 messages | exact | — |
| 120Hz repaint count | 12 repaints | exact | — |
| 144Hz wire bytes | 598 bytes | exact | — |
| 144Hz message count | 24 messages | exact | — |
| 144Hz repaint count | 12 repaints | exact | — |
| 240Hz wire bytes | 598 bytes | exact | — |
| 240Hz message count | 24 messages | exact | — |
| 240Hz repaint count | 24 repaints | exact | — |
| idle UI wire bytes | 0 bytes | exact | — |
| idle UI message count | 0 messages | exact | — |

Assertions:

- **PASS** idle semantic UI emits zero SRUI traffic — no transaction is produced without a semantic mutation
- **PASS** wire bytes and message count are cadence independent — 598 bytes and 24 messages at every cadence
- **PASS** local repaint count may vary independently — repaint counts [6, 12, 12, 24]
- **PASS** coalesced presentation preserves final state and scalar classification — all cadences rendered progress 1.0 without structural invalidation

## §31.4 Network and local interaction

| Metric | Value | Statistic | Target |
|---|---:|---|---:|
| IME composition at 0ms RTT | 0.1453 ms | p50 | ≤ 16.67 ms |
| caret movement at 0ms RTT | 0.02683 ms | p50 | ≤ 16.67 ms |
| hover and pressed at 0ms RTT | 0.005541 ms | p50 | ≤ 16.67 ms |
| menu opening at 0ms RTT | 0.003917 ms | p50 | ≤ 16.67 ms |
| scrolling at 0ms RTT | 0.024 ms | p50 | ≤ 16.67 ms |
| text entry at 0ms RTT | 0.00125 ms | p50 | ≤ 16.67 ms |
| text selection at 0ms RTT | 0.02679 ms | p50 | ≤ 16.67 ms |
| server-dependent round trip at 0ms RTT | 12.02 ms | p50 | — |
| IME composition at 100ms RTT | 0.1556 ms | p50 | ≤ 16.67 ms |
| caret movement at 100ms RTT | 0.02871 ms | p50 | ≤ 16.67 ms |
| hover and pressed at 100ms RTT | 0.005917 ms | p50 | ≤ 16.67 ms |
| menu opening at 100ms RTT | 0.00425 ms | p50 | ≤ 16.67 ms |
| scrolling at 100ms RTT | 0.026 ms | p50 | ≤ 16.67 ms |
| text entry at 100ms RTT | 0.001375 ms | p50 | ≤ 16.67 ms |
| text selection at 100ms RTT | 0.02846 ms | p50 | ≤ 16.67 ms |
| server-dependent round trip at 100ms RTT | 307.7 ms | p50 | — |
| IME composition at 300ms RTT | 0.1616 ms | p50 | ≤ 16.67 ms |
| caret movement at 300ms RTT | 0.02833 ms | p50 | ≤ 16.67 ms |
| hover and pressed at 300ms RTT | 0.006083 ms | p50 | ≤ 16.67 ms |
| menu opening at 300ms RTT | 0.004375 ms | p50 | ≤ 16.67 ms |
| scrolling at 300ms RTT | 0.03771 ms | p50 | ≤ 16.67 ms |
| text entry at 300ms RTT | 0.001375 ms | p50 | ≤ 16.67 ms |
| text selection at 300ms RTT | 0.02975 ms | p50 | ≤ 16.67 ms |
| server-dependent round trip at 300ms RTT | 304.3 ms | p50 | — |
| IME composition at 600ms RTT | 0.1671 ms | p50 | ≤ 16.67 ms |
| caret movement at 600ms RTT | 0.03117 ms | p50 | ≤ 16.67 ms |
| hover and pressed at 600ms RTT | 0.006167 ms | p50 | ≤ 16.67 ms |
| menu opening at 600ms RTT | 0.0045 ms | p50 | ≤ 16.67 ms |
| scrolling at 600ms RTT | 0.02933 ms | p50 | ≤ 16.67 ms |
| text entry at 600ms RTT | 0.001417 ms | p50 | ≤ 16.67 ms |
| text selection at 600ms RTT | 0.02933 ms | p50 | ≤ 16.67 ms |
| server-dependent round trip at 600ms RTT | 636 ms | p50 | — |
| 1MiB/s bandwidth-limited 16KiB transfer | 23.4 ms | p50 | — |
| deterministic lost-frame retry penalty | 109.9 ms | p50 | — |
| controlled transport interruption | 52.18 ms | p50 | — |
| maximum RTT-induced local latency delta | 0.02179 ms | p50 | ≤ 16.67 ms |

Assertions:

- **PASS** local interactions do not acquire one RTT — largest p50 increase was 0.0218 ms
- **PASS** injected transport delay affects server-dependent feedback — controlled waits tracked 100/300/600ms RTT
- **PASS** render and local-feedback paths perform no synchronous network RTT — network waits occur only in the separately measured server-dependent path
- **PASS** bandwidth, loss, and interruption controls were exercised — 1MiB/s serialization, one 100ms retry, and a 50ms interruption

Notes:

- Menu measurement covers local NSPopUpButton menu preparation without entering a blocking tracking loop.

## §31.5 Reconnect

| Metric | Value | Statistic | Target |
|---|---:|---|---:|
| beyond journal retention | 4.2e-05 ms | p50 | — |
| immediately after event receipt | 0.000833 ms | p50 | — |
| immediately before event receipt | 4.2e-05 ms | p50 | — |
| lost ACK duplicate response | 0.00025 ms | p50 | — |
| mid-resource discard | 0.004959 ms | p50 | — |
| mid-transaction rollback | 0.000917 ms | p50 | — |
| superseded resume response | 8.3e-05 ms | p50 | — |
| within journal retention | 0.000208 ms | p50 | — |
| production reconnect boundary suite | 1.742e+04 ms | wall | — |

Assertions:

- **PASS** all reconnect boundary outcomes are deterministic — partial state discarded; retained replay/resync split preserved
- **PASS** lost ACK replay is DUPLICATE without a second side effect — cached accepted result at revision 2; side-effect count remained 1
- **PASS** superseded resume response is inert — attempt-token model is backed by the measured production reconnect suite
- **PASS** production reconnect boundary suite — exit 0; SRUI §32 conformance — both
==============================================================================
 #  suite                    result  notes
------------------------------------------------------------------------------
 8  reconnect                PASS    9 runner(s)
==============================================================================
1 passed, 0 failed, 0 documented gap(s), 0 not applicable

## §31.6 Terminal

| Metric | Value | Statistic | Target |
|---|---:|---|---:|
| standalone PTY echo roundtrip | 6.075 ms | p50 | — |
| standalone PTY echo roundtrip | 6.492 ms | p95 | — |
| SRUI output ring append and frame | 0.00025 ms | p50 | — |
| SRUI output ring append and frame | 0.000375 ms | p95 | — |
| terminal payload | 6912 bytes | exact | — |
| embedded Terminal decode-to-visible | 0.8442 ms | p50 | — |
| embedded Terminal decode-to-visible | 1.007 ms | p95 | — |
| embedded terminal frame | 6912 bytes | exact | — |

Assertions:

- **PASS** embedded replay preserves the complete retained byte stream — 6912 bytes compared with standalone shell PTY
- **PASS** reconnect ring-buffer exhaustion is explicit — request before retained_start returned RangeUnavailable
- **PASS** embedded terminal offsets remain contiguous — final offset 138240

Notes:

- The embedded measurement includes VT parsing, TerminalView snapshot apply, and local layout.

## Follow-up flags

- No §23 target was missed by more than 2x.
