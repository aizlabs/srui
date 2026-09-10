# §31.2 serialization benchmark

Owner: `server-rust/benchmark-driver/src/serialization.rs::serialization`. The release driver parses the
same abstract coding-agent fixture used by §31.1, constructs the same two production
semantic-tree transactions at the fixture's `first_paint_node_count` boundary (revisions 0→1 and
1→2), and serializes each through the production Protobuf bridge. The canonical artifact frames
each transaction as `[u64 big-endian length][protobuf]`; Swift §31.1 and Rust §31.2 must report
the same byte count and SHA-256. Generation and serialization of the complete two-transaction plan
are timed separately; PTY spawn, transport, client decode, and rendering are excluded.

Combined Rust-driver command (the single invocation emits §31.2, §31.5, and §31.6):

    cargo run --quiet --release --manifest-path server-rust/Cargo.toml -p srui-benchmark-driver -- --fixture benchmarks/fixtures/coding-agent-ui.json --profile smoke --output /tmp/srui-rust.json

Metrics are `abstract_state_generation_ms`, `protobuf_serialization_ms`, and exact
`serialized_transaction_bytes`; `fixture_protobuf_valid` verifies that the shared fixture produced
two non-empty, revision-ordered production wire transactions.
