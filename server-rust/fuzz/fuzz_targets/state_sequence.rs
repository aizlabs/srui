//! Stateful transaction-sequence fuzzing for rollback and graph invariants (§12.1, §13, §18.1, §26).
//!
//! Drives a store and a journal through the same prepared-commit sequence a session uses, and
//! asserts after every input that they still agree: a rejected transaction changes neither, and an
//! accepted one advances both to the same revision.

#![no_main]

mod store_snapshot;

use libfuzzer_sys::fuzz_target;
use srui_journal::TransactionJournal;
use srui_semantic_tree::{decode_transaction, AuthoritativeCommit, SemanticStore};
use store_snapshot::take_snapshot;

const MAX_INPUT_BYTES: usize = 256 * 1024;
const MAX_TRANSACTIONS: usize = 64;

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_BYTES {
        return;
    }

    let mut store = SemanticStore::new();
    let mut journal = TransactionJournal::default();
    // Replaying the same decoded transaction guarantees a rejection against populated state
    // whenever the first application commits, exercising rollback beyond an empty store.
    apply_if_decodable(data, &mut store, &mut journal);
    apply_if_decodable(data, &mut store, &mut journal);

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
        apply_if_decodable(transaction_bytes, &mut store, &mut journal);
        offset = end;
    }
});

fn apply_if_decodable(data: &[u8], store: &mut SemanticStore, journal: &mut TransactionJournal) {
    let Ok(transaction) = decode_transaction(data) else {
        return;
    };

    let before = take_snapshot(store);
    let journal_before = (journal.len(), journal.latest_revision());

    let rejected = || {
        assert_eq!(
            before,
            take_snapshot(store),
            "a rejected transaction in a live sequence must roll back completely"
        );
        assert_eq!(
            (journal.len(), journal.latest_revision()),
            journal_before,
            "a rejected transaction must not touch the journal"
        );
    };

    // Only an authoritative commit may advance the pair, and everything that can fail is decided
    // before either mutates — the same order a session commits in (§12.1, §18.1).
    let Ok(commit) = AuthoritativeCommit::try_from(transaction) else {
        rejected();
        return;
    };
    let Ok(staged) = store.prepare_commit(&commit) else {
        rejected();
        return;
    };
    let Ok(permit) = journal.prepare(&commit) else {
        rejected();
        return;
    };

    store.commit_prepared(staged);
    journal.append(permit);

    assert_eq!(
        store.revision().get(),
        journal.latest_revision(),
        "store and journal must advance together"
    );
}
