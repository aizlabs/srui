//! Shared deterministic fixtures for the process-monitor tests.
//!
//! No test in this crate ever enumerates or signals a real host process.

#![allow(dead_code)]

use std::sync::Arc;

use srui_example_process_monitor::testing::{
    record, snapshot, FakeProcessSource, RecordingTerminator,
};
use srui_example_process_monitor::{Monitor, ProcessRecord, ProcessSnapshot};
use srui_sdk::ServerCapabilities;
use srui_sessiond::Session;

/// Effective user id the monitor runs as in tests.
pub const UID: u32 = 501;
/// A different, foreign user id.
pub const OTHER_UID: u32 = 0;
/// One mebibyte in bytes.
pub const MIB: u64 = 1024 * 1024;

/// Three same-user processes plus one foreign-user process.
pub fn base_processes() -> Vec<ProcessRecord> {
    vec![
        record(10, 1_000, "alpha", 1.0, 100 * MIB, Some(UID)),
        record(20, 1_000, "beta", 2.0, 200 * MIB, Some(UID)),
        record(30, 1_000, "gamma", 3.0, 300 * MIB, Some(UID)),
        record(40, 1_000, "root-daemon", 4.0, 400 * MIB, Some(OTHER_UID)),
    ]
}

/// A snapshot with 25% CPU and the standard fixture processes.
pub fn base_snapshot() -> ProcessSnapshot {
    snapshot(25.0, base_processes())
}

/// A started monitor plus handles on its injected effects.
pub struct Fixture {
    pub monitor: Arc<Monitor>,
    pub source: FakeProcessSource,
    pub terminator: RecordingTerminator,
    pub session: Arc<Session>,
}

/// Starts a monitor over the given snapshot.
pub fn fixture(initial: ProcessSnapshot) -> Fixture {
    let session = Arc::new(Session::with_capabilities(
        "process-monitor-test",
        ServerCapabilities::standard_widgets(),
    ));
    let source = FakeProcessSource::new(initial);
    let terminator = RecordingTerminator::default();
    let monitor = Monitor::start(session.clone(), source.boxed(), terminator.boxed(), UID)
        .expect("monitor starts");
    Fixture {
        monitor,
        source,
        terminator,
        session,
    }
}

/// Starts a monitor over [`base_snapshot`].
pub fn base_fixture() -> Fixture {
    fixture(base_snapshot())
}

/// Cached model rows in index order, as the client would render them.
pub fn model_rows(session: &Session) -> Vec<(srui_sdk::ItemId, srui_sdk::Value)> {
    session.with_store(|store| {
        let model = store
            .get_model(srui_example_process_monitor::PROCESS_MODEL_ID)
            .expect("process model");
        (0..model.item_count())
            .map(|index| {
                let item = model.get_item_by_index(index).expect("cached item");
                (item.item_id, item.value.clone())
            })
            .collect()
    })
}

/// Applies `operations` to a standalone model seeded with `prev`, returning the resulting rows.
pub fn replay_model(
    prev: &[srui_example_process_monitor::VisibleRow],
    operations: &[srui_sdk::Operation],
) -> Vec<(srui_sdk::ItemId, srui_sdk::Value)> {
    use srui_example_process_monitor::PROCESS_MODEL_ID;
    use srui_sdk::{Operation, SemanticStore, TypeRef};

    let mut store = SemanticStore::new();
    store
        .create_model(PROCESS_MODEL_ID, TypeRef::TABLE, 0)
        .expect("create model");
    let mut offset: u64 = 0;
    for chunk in prev.chunks(srui_example_process_monitor::MAX_ITEMS_PER_MODEL_OP) {
        let chunk_len = chunk.len() as u64;
        Operation::model_insert(
            PROCESS_MODEL_ID,
            offset,
            chunk.iter().map(|row| row.to_model_item()),
        )
        .apply(&mut store)
        .expect("seed model");
        offset += chunk_len;
    }
    for operation in operations {
        operation.apply(&mut store).expect("apply operation");
    }

    let model = store.get_model(PROCESS_MODEL_ID).expect("model");
    (0..model.item_count())
        .map(|index| {
            let item = model.get_item_by_index(index).expect("cached item");
            (item.item_id, item.value.clone())
        })
        .collect()
}

/// Short operation kind names, for asserting the operation mix of a tick.
pub fn kinds(operations: &[srui_sdk::Operation]) -> Vec<&'static str> {
    use srui_sdk::Operation;

    operations
        .iter()
        .map(|operation| match operation {
            Operation::CreateNode { .. } => "CREATE_NODE",
            Operation::DeleteNode { .. } => "DELETE_NODE",
            Operation::SetProperty { .. } => "SET_PROPERTY",
            Operation::ClearProperty { .. } => "CLEAR_PROPERTY",
            Operation::MoveNode { .. } => "MOVE_NODE",
            Operation::ReorderChildren { .. } => "REORDER_CHILDREN",
            Operation::BatchPropertySet { .. } => "BATCH_PROPERTY_SET",
            Operation::CreateModel { .. } => "CREATE_MODEL",
            Operation::ModelInsert { .. } => "MODEL_INSERT",
            Operation::ModelDelete { .. } => "MODEL_DELETE",
            Operation::ModelUpdate { .. } => "MODEL_UPDATE",
            Operation::ModelResetRange { .. } => "MODEL_RESET_RANGE",
        })
        .collect()
}
