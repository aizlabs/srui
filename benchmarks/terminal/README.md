# §31.6 terminal benchmark

Rust owners: `server-rust/benchmark-driver/src/terminal.rs::{pty_roundtrip,embedded_pty_roundtrip,terminal}`.
The standalone and embedded cases each run a real PTY and capture the emitted byte stream. This is a
server transport/process baseline: the embedded production `PTYManager` stream must finish at EOF
with byte-for-byte equality to its expected payload and valid frame bounds. It is not presented as
a GUI terminal comparison. The production output ring is then exhausted and must report
`TerminalResyncReason::RetentionLoss` for a reconnect offset older than its retained start.

Swift owners: `client-macos/Benchmarks/TerminalBenchmark.swift::{productionTerminalSample,standaloneTerminalSample,terminal}`.
The embedded path sends the exact ANSI payload through framed production transport,
`SessionController`, the renderer-owned `TerminalSession`, and `TerminalView`. The standalone
control sends the same `Data` directly through a fresh production `TerminalSession` and
`TerminalView` with identical grid geometry and bounds. Both paths measure decode-to-visible and
draw-only latency, verify exact terminal offsets and content hashes, and require byte-identical
rendered rasters. Smoke uses the deterministic offscreen draw boundary; full mode additionally
requires WindowServer framebuffer advancement and a draw-completion marker. Every sample starts
with fresh terminal state.

Commands (the Rust invocation emits §31.2, §31.5, and §31.6 together; Swift can be focused):

    cargo run --quiet --release --manifest-path server-rust/Cargo.toml -p srui-benchmark-driver -- --fixture benchmarks/fixtures/coding-agent-ui.json --profile smoke --output /tmp/srui-rust.json
    client-macos/.build/release/BenchmarkDriver --fixture benchmarks/fixtures/coding-agent-ui.json --profile smoke --only-section 31.6 --output /tmp/srui-macos-31.6.json

Rust metrics include `standalone_pty_interaction_ms`, `embedded_pty_interaction_ms`,
`embedded_terminal_frame_count`, and `terminal_retention_loss_ms`; assertions include
`pty_payload_identical`, `embedded_pty_eof_exact`, and `terminal_ring_retention_loss`.

Swift metrics include `client_terminal.{decode_visible,draw_only,frame_bytes,raster_completions}`,
`standalone_terminal.{decode_visible,draw_only,raster_completions}`, and the
`terminal_display.embedded_to_standalone_{decode,draw}_ratio` controls. Assertions require exact
offsets on both paths, draw completion, fresh state, identical content, and byte-identical rasters.
