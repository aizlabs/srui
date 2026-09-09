# §31.6 terminal benchmark

The Rust driver measures a standalone PTY producing a fixed byte payload and the SRUI output
ring framing the same payload. The Swift driver measures that payload through TerminalSession,
TerminalView snapshot application, and local layout.

Correctness assertions require contiguous embedded offsets, byte-exact retained replay, and an
explicit RangeUnavailable result after reconnect requests data older than the ring retained start.
