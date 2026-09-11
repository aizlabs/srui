# §31.5 reconnect benchmark

Rust owners: `server-rust/benchmark-driver/src/reconnect.rs::reconnect` and
`reconnect_wire.rs::{wire_pre_receipt_disconnect,wire_lost_ack_reconnect,
wire_partial_transaction_reconnect,wire_partial_event_reconnect}`. It exercises the production resource lane, codec, `Session`, semantic store transaction
rollback, journal retention, and event result cache. The lost-ACK replay must return
`DUPLICATE` with the cached result and exact settled event sequence while the side-effect counter
remains one.

Swift owner: `client-macos/Benchmarks/ReconnectBenchmark.swift::reconnect`, including
`preReceiptEventReplaySample`, plus the production `SessionController` resume/resource paths. It
measures pending-event replay before receipt, interrupted resource recovery, and a resume attempt
superseded mid-flight; the old attempt's eventual response and bytes must be inert. The consolidated
runner separately times `scripts/run-conformance --suite 8 --implementation both`.

Commands (the Rust invocation emits §31.2, §31.5, and §31.6 together; Swift can be focused):

    cargo run --quiet --release --locked --manifest-path server-rust/Cargo.toml -p srui-benchmark-driver -- --fixture benchmarks/fixtures/coding-agent-ui.json --profile smoke --output /tmp/srui-rust.json
    client-macos/Benchmarks/.build/release/BenchmarkDriver --fixture benchmarks/fixtures/coding-agent-ui.json --profile smoke --only-section 31.5 --output /tmp/srui-macos-31.5.json

The Rust metric family includes `disconnect_before_event_receipt_ms`,
`lost_ack_wire_duplicate_ms`, `mid_resource_reconnect_ms`, partial transaction/event replay, and
within/beyond-retention resume timings. Swift publishes `pre_receipt_event_replay`,
`mid_resource_recovery`, `superseded_response`, and `active_response`. Boundary-specific
assertions include `mid_resource_exact_restart`, `mid_transaction_wire_atomic`,
`partial_event_wire_once`, `lost_ack_wire_duplicate_once`, `journal_retention_boundary`,
`pre_receipt_pending_replay`, and `superseded_response_inert`.
