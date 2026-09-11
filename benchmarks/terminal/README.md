# §31.6 terminal benchmark

Rust owners: `server-rust/benchmark-driver/src/terminal.rs::{standalone_pty_roundtrip,
embedded_pty_roundtrip,ring_exhaustion,terminal}`.

Both Rust comparison paths allocate a real PTY after their clocks start and run the same shell
script, which emits the same 6,912-byte ANSI payload. `standalone_pty_roundtrip` reads the PTY
master directly on a bounded blocking worker. `embedded_pty_roundtrip` uses the ordinary production
`PTYManager`, `TerminalSubscription`, and `OutputRing` path. It accepts only nonempty production
frames within `MAX_TERMINAL_OUTPUT_FRAME_BYTES`, requires contiguous exact offsets, records the
timed boundary when the expected byte count is first reached, then drains to ordinary bounded
natural EOF. Either path rejects a trailing byte, an early EOF, a timeout, a failed standalone
child exit, a resync, or an offset mismatch. There is no benchmark-only `srui-pty` feature or exit
observer.

The Rust reconnect case uses a 1,024-byte production output ring, consumes the complete stream, and
then subscribes from offset zero. Both the subscribe snapshot and catch-up event must report
`TerminalResyncReason::RetentionLoss`, a positive retained start, and the exact final resume offset.

Swift owners: `client-macos/Benchmarks/TerminalBenchmark.swift::{productionTerminalSample,
standaloneTerminalDisplaySample,terminal}`.

The embedded display path sends the exact ANSI payload through framed production transport,
`SessionController`, the renderer-owned `TerminalSession`, and `TerminalView`. The standalone
control sends the same `Data` directly through a fresh production `TerminalSession` and
`TerminalView` with identical grid geometry and bounds. Both paths measure decode-to-visible and
draw-only latency, verify exact terminal offsets and content hashes, and require byte-identical
rendered rasters. Smoke uses the deterministic offscreen draw boundary; full mode additionally
requires WindowServer framebuffer advancement and a draw-completion marker. Every sample starts
with fresh terminal state.

Commands (the Rust invocation emits §§31.2, 31.5, and 31.6; Swift can be focused):

    cargo run --quiet --release --locked --manifest-path server-rust/Cargo.toml -p srui-benchmark-driver -- --fixture benchmarks/fixtures/coding-agent-ui.json --profile smoke --output /tmp/srui-rust.json
    client-macos/Benchmarks/.build/release/BenchmarkDriver --fixture benchmarks/fixtures/coding-agent-ui.json --profile smoke --only-section 31.6 --output /tmp/srui-macos-31.6.json

Rust metrics include `standalone_pty_interaction_ms`, `embedded_pty_interaction_ms`,
`embedded_terminal_frame_count`, and `terminal_retention_loss_ms`; assertions cover identical
payloads, successful bounded standalone exit, exact embedded EOF and frame bounds, and reconnect
retention loss.

Swift metrics include `client_terminal.{decode_visible,draw_only,frame_bytes,raster_completions}`,
`standalone_terminal.{decode_visible,draw_only,raster_completions}`, and the
`terminal_display.embedded_to_standalone_{decode,draw}_ratio` controls. Assertions require exact
offsets, draw completion, fresh state, identical content, and byte-identical rasters.
