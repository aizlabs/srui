//! Fuzz target for decode-then-apply transaction paths (§12.1, §26).
//!
//! Invariants: no panic/hang; failed apply leaves store revision and counts unchanged.

#![no_main]

use libfuzzer_sys::fuzz_target;
use srui_semantic_tree::{decode_transaction, SemanticStore};

const MAX_FUZZ_TX_BYTES: usize = 256 * 1024;

fn store_fingerprint(store: &SemanticStore) -> (u64, usize, usize) {
    (
        store.revision().0,
        store.node_count(),
        store.model_count(),
    )
}

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_FUZZ_TX_BYTES {
        return;
    }

    let mut store = SemanticStore::new();
    let before = store_fingerprint(&store);

    let Ok(txn) = decode_transaction(data) else {
        return;
    };

    match store.apply_transaction_record(&txn) {
        Ok(_) => {}
        Err(_) => {
            let after = store_fingerprint(&store);
            assert_eq!(
                before, after,
                "failed transaction must not mutate authoritative store state"
            );
        }
    }
});
