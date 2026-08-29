//! Stateful transaction-sequence fuzzing for rollback and graph invariants (§12.1, §13, §26).

#![no_main]

mod store_snapshot;

use libfuzzer_sys::fuzz_target;
use srui_semantic_tree::{decode_transaction, SemanticStore};
use store_snapshot::take_snapshot;

const MAX_INPUT_BYTES: usize = 256 * 1024;
const MAX_TRANSACTIONS: usize = 64;

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_BYTES {
        return;
    }

    let mut store = SemanticStore::new();
    // Replaying the same decoded transaction guarantees a rejection against populated state
    // whenever the first application commits, exercising rollback beyond an empty store.
    apply_if_decodable(data, &mut store);
    apply_if_decodable(data, &mut store);

    let mut offset = 0usize;
    for _ in 0..MAX_TRANSACTIONS {
        let Some(prefix) = data.get(offset..offset.saturating_add(4)) else {
            break;
        };
        let declared = u32::from_le_bytes(prefix.try_into().expect("four-byte prefix")) as usize;
        offset += 4;
        let Some(end) = offset.checked_add(declared) else {
            break;
        };
        let Some(transaction_bytes) = data.get(offset..end) else {
            break;
        };
        apply_if_decodable(transaction_bytes, &mut store);
        offset = end;
    }
});

fn apply_if_decodable(data: &[u8], store: &mut SemanticStore) {
    let Ok(transaction) = decode_transaction(data) else {
        return;
    };
    let before = take_snapshot(store);
    if store.apply_transaction_record(&transaction).is_err() {
        assert_eq!(
            before,
            take_snapshot(store),
            "a rejected transaction in a live sequence must roll back completely"
        );
    } else {
        let _ = take_snapshot(store);
    }
}
