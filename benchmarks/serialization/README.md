# §31.2 serialization benchmark

The Rust release driver parses the same abstract coding-agent fixture used by §31.1, constructs
domain operations, and serializes the resulting transaction with the production Protobuf bridge.
Generation and serialization are timed separately. The encoded byte count is exact. PTY spawn,
transport, client decode, and rendering are excluded.
