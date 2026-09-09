# §31.5 reconnect benchmark

The Rust driver measures mid-resource discard, atomic mid-transaction rollback, event admission,
the lost-ACK replay window, replay within journal retention, resync beyond retention, and
superseded-attempt rejection.

The orchestration layer also times the real cross-language §32 reconnect suite. That production
gate covers partial resources/transactions and the Task 23 client resume state machine. The
lost-ACK case asserts the Task 24 result-cache contract directly: replay returns DUPLICATE with the
cached result and the side-effect counter remains exactly one.
