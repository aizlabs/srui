//! Fuzz target for decode-then-apply transaction paths (§12.1, §26).
//!
//! Invariants: no panic/hang; failed apply leaves every observable store field unchanged.

#![no_main]

mod store_snapshot;

use libfuzzer_sys::fuzz_target;
use srui_semantic_tree::{decode_transaction, SemanticStore};
use store_snapshot::take_snapshot;

const MAX_FUZZ_TX_BYTES: usize = 256 * 1024;

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_FUZZ_TX_BYTES {
        return;
    }

    let Ok(transaction) = decode_transaction(data) else {
        return;
    };
    let mut store = SemanticStore::new();
    let before = take_snapshot(&store);

    if store.apply_transaction_record(&transaction).is_err() {
        assert_eq!(
            before,
            take_snapshot(&store),
            "failed transaction must not mutate any observable store state"
        );
    } else {
        let _ = take_snapshot(&store);
    }
});
