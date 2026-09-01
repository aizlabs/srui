//! Process identity and minimal-diff coverage (§8, §12.1, §13, §23).

mod common;

use std::collections::HashSet;

use common::{base_fixture, base_processes, kinds, model_rows, replay_model, MIB, OTHER_UID, UID};
use srui_example_process_monitor::testing::{record, snapshot};
use srui_example_process_monitor::*;
use srui_sdk::{ItemId, Operation, Value};

fn seeded_state(processes: Vec<ProcessRecord>) -> MonitorState {
    let mut state = MonitorState::with_denylist(UID, HashSet::new());
    state.seed(&snapshot(25.0, processes));
    state
}

fn item_for_pid(state: &MonitorState, pid: u32) -> ItemId {
    state
        .visible()
        .iter()
        .find(|row| row.values.pid == u64::from(pid))
        .unwrap_or_else(|| panic!("pid {pid} is visible"))
        .item_id
}

// -----------------------------------------------------------------------------
// Identity
// -----------------------------------------------------------------------------

#[test]
fn retained_process_keeps_its_item_id_across_ticks() {
    let mut state = seeded_state(base_processes());
    let before = item_for_pid(&state, 20);

    let mut changed = base_processes();
    changed[1].cpu_percent = 44.0;
    let plan = state.plan_snapshot(&snapshot(30.0, changed));
    state.commit(plan);

    assert_eq!(item_for_pid(&state, 20), before);
}

#[test]
fn row_index_shifts_do_not_change_item_ids() {
    let mut state = seeded_state(base_processes());
    let before: Vec<(u64, ItemId)> = state
        .visible()
        .iter()
        .map(|row| (row.values.pid, row.item_id))
        .collect();

    // A new lowest PID shifts every existing row down by one index.
    let mut processes = base_processes();
    processes.push(record(5, 2_000, "newcomer", 0.5, 10 * MIB, Some(UID)));
    let plan = state.plan_snapshot(&snapshot(25.0, processes));
    state.commit(plan);

    assert_eq!(state.visible()[0].values.pid, 5);
    for (pid, item) in before {
        assert_eq!(
            item_for_pid(&state, pid as u32),
            item,
            "pid {pid} kept its id"
        );
    }
}

#[test]
fn filtering_out_and_back_in_preserves_the_item_id() {
    let mut state = seeded_state(base_processes());

    let plan = state.plan_visibility(true);
    state.commit(plan);
    let foreign = item_for_pid(&state, 40);

    let plan = state.plan_visibility(false);
    state.commit(plan);
    assert!(state.visible().iter().all(|row| row.values.pid != 40));

    let plan = state.plan_visibility(true);
    state.commit(plan);
    assert_eq!(item_for_pid(&state, 40), foreign);
}

#[test]
fn exited_process_produces_a_model_delete() {
    let state = seeded_state(base_processes());
    let gone = item_for_pid(&state, 30);

    let remaining: Vec<ProcessRecord> = base_processes()
        .into_iter()
        .filter(|process| process.key.pid != 30)
        .collect();
    let plan = state.plan_snapshot(&snapshot(25.0, remaining));

    assert_eq!(kinds(plan.operations()), vec!["MODEL_DELETE"]);
    match &plan.operations()[0] {
        Operation::ModelDelete { item_ids, .. } => assert_eq!(item_ids.as_slice(), &[gone]),
        other => panic!("expected MODEL_DELETE, got {other:?}"),
    }
}

#[test]
fn reused_pid_with_a_new_start_time_receives_a_new_item_id() {
    let mut state = seeded_state(base_processes());
    let original = item_for_pid(&state, 20);

    // pid 20 exits and the OS hands the same number to a different process.
    let mut processes = base_processes();
    processes[1] = record(20, 9_999, "impostor", 1.0, 20 * MIB, Some(UID));
    let plan = state.plan_snapshot(&snapshot(25.0, processes));
    let operation_kinds = kinds(plan.operations());
    state.commit(plan);

    let reused = item_for_pid(&state, 20);
    assert_ne!(reused, original);
    assert_eq!(state.key_for_item(original), None);
    assert_eq!(state.key_for_item(reused), Some(ProcessKey::new(20, 9_999)));
    assert_eq!(operation_kinds, vec!["MODEL_DELETE", "MODEL_INSERT"]);
}

// -----------------------------------------------------------------------------
// Diffing
// -----------------------------------------------------------------------------

#[test]
fn identical_snapshots_emit_no_operations() {
    let state = seeded_state(base_processes());
    let plan = state.plan_snapshot(&snapshot(25.0, base_processes()));
    assert!(
        plan.is_empty(),
        "unchanged state must not produce operations"
    );
}

#[test]
fn one_new_process_emits_only_a_model_insert() {
    let state = seeded_state(base_processes());

    let mut processes = base_processes();
    processes.push(record(25, 3_000, "delta", 5.0, 50 * MIB, Some(UID)));
    let plan = state.plan_snapshot(&snapshot(25.0, processes));

    assert_eq!(kinds(plan.operations()), vec!["MODEL_INSERT"]);
    match &plan.operations()[0] {
        Operation::ModelInsert { index, items, .. } => {
            // pid 25 sorts between 20 and 30.
            assert_eq!(index, &2);
            assert_eq!(items.len(), 1);
        }
        other => panic!("expected MODEL_INSERT, got {other:?}"),
    }
}

#[test]
fn a_changed_row_emits_only_a_model_update_for_that_row() {
    let state = seeded_state(base_processes());
    let changed_item = item_for_pid(&state, 20);

    let mut processes = base_processes();
    processes[1].cpu_percent = 42.5;
    processes[1].memory_bytes = 512 * MIB;
    let plan = state.plan_snapshot(&snapshot(25.0, processes));

    assert_eq!(kinds(plan.operations()), vec!["MODEL_UPDATE"]);
    match &plan.operations()[0] {
        Operation::ModelUpdate { index, items, .. } => {
            assert_eq!(index, &None, "updates address items by stable identity");
            assert_eq!(items.len(), 1, "unchanged retained rows are not resent");
            assert_eq!(items[0].item_id, changed_item);
            assert_eq!(
                items[0].value,
                Value::List(vec![
                    Value::UnsignedInt(20),
                    Value::String("beta".to_string()),
                    Value::Float64(42.5),
                    Value::UnsignedInt(512),
                ])
            );
        }
        other => panic!("expected MODEL_UPDATE, got {other:?}"),
    }
}

#[test]
fn sub_display_precision_cpu_noise_produces_no_operations() {
    let state = seeded_state(base_processes());

    let mut processes = base_processes();
    processes[0].cpu_percent = 1.0004;
    let plan = state.plan_snapshot(&snapshot(25.0, processes));

    assert!(plan.is_empty(), "quantized values suppress float noise");
}

#[test]
fn mixed_insert_update_delete_produces_the_correct_final_model() {
    let state = seeded_state(base_processes());
    let previous = state.visible().to_vec();

    let mut processes = base_processes();
    processes.retain(|process| process.key.pid != 10); // delete first row
    processes[0].cpu_percent = 77.0; // update pid 20
    processes.push(record(35, 4_000, "epsilon", 9.0, 90 * MIB, Some(UID))); // insert after 30
    let plan = state.plan_snapshot(&snapshot(25.0, processes));

    assert_eq!(
        kinds(plan.operations()),
        vec!["MODEL_DELETE", "MODEL_INSERT", "MODEL_UPDATE"]
    );

    let replayed = replay_model(&previous, plan.operations());
    let expected: Vec<(ItemId, Value)> = plan
        .visible()
        .iter()
        .map(|row| (row.item_id, row.values.to_value()))
        .collect();
    assert_eq!(replayed, expected);
}

#[test]
fn insertion_indices_stay_correct_after_deletions() {
    let state = seeded_state(base_processes());
    let previous = state.visible().to_vec();

    // Drop the two lowest PIDs and add two new ones on either side of the survivor.
    let processes = vec![
        record(15, 5_000, "fresh-low", 1.0, MIB, Some(UID)),
        record(30, 1_000, "gamma", 3.0, 300 * MIB, Some(UID)),
        record(45, 5_000, "fresh-high", 1.0, MIB, Some(UID)),
    ];
    let plan = state.plan_snapshot(&snapshot(25.0, processes));

    let replayed = replay_model(&previous, plan.operations());
    let expected: Vec<(ItemId, Value)> = plan
        .visible()
        .iter()
        .map(|row| (row.item_id, row.values.to_value()))
        .collect();
    assert_eq!(replayed, expected);
    assert_eq!(
        replayed
            .iter()
            .map(|(_, value)| value.clone())
            .collect::<Vec<_>>()
            .len(),
        3
    );
}

#[test]
fn a_reordered_retained_row_is_rebuilt_explicitly() {
    // The production sort key can never reorder retained rows; the diff still has to stay correct
    // if it ever encounters one, rather than emit an inconsistent model.
    let rows = |order: [u32; 3]| -> Vec<VisibleRow> {
        order
            .iter()
            .map(|pid| VisibleRow {
                item_id: ItemId::new(u64::from(*pid)),
                key: ProcessKey::new(*pid, 1),
                values: RowValues {
                    pid: u64::from(*pid),
                    name: format!("p{pid}"),
                    cpu_percent: 1.0,
                    memory_mib: 1,
                },
            })
            .collect()
    };

    let previous = rows([1, 2, 3]);
    let next = rows([3, 1, 2]);
    let operations = diff_visible_rows(PROCESS_MODEL_ID, &previous, &next);

    let replayed = replay_model(&previous, &operations);
    let expected: Vec<(ItemId, Value)> = next
        .iter()
        .map(|row| (row.item_id, row.values.to_value()))
        .collect();
    assert_eq!(replayed, expected);
}

// -----------------------------------------------------------------------------
// Transaction shape
// -----------------------------------------------------------------------------

#[test]
fn one_polling_sample_produces_exactly_one_transaction() {
    let fixture = base_fixture();
    let before = fixture.session.current_revision();

    let mut processes = base_processes();
    processes[0].cpu_percent = 60.0;
    processes.push(record(50, 6_000, "zeta", 1.0, MIB, Some(UID)));
    fixture.source.publish(snapshot(80.0, processes));

    fixture.monitor.tick().expect("tick commits");
    assert_eq!(fixture.session.current_revision(), before + 1);
}

#[test]
fn an_empty_diff_commits_no_transaction() {
    let fixture = base_fixture();
    let before = fixture.session.current_revision();

    let committed = fixture.monitor.tick().expect("tick runs");
    assert_eq!(committed, 0);
    assert_eq!(fixture.session.current_revision(), before);
}

#[test]
fn ordinary_ticks_never_create_nodes_models_or_reset_ranges() {
    let mut state = seeded_state(base_processes());

    let mutations: Vec<Vec<ProcessRecord>> = vec![
        {
            let mut processes = base_processes();
            processes[0].cpu_percent = 91.0;
            processes
        },
        {
            let mut processes = base_processes();
            processes.retain(|process| process.key.pid != 20);
            processes
        },
        {
            let mut processes = base_processes();
            processes.push(record(60, 7_000, "eta", 2.0, 2 * MIB, Some(OTHER_UID)));
            processes.push(record(61, 7_000, "theta", 2.0, 2 * MIB, Some(UID)));
            processes
        },
    ];

    for processes in mutations {
        let plan = state.plan_snapshot(&snapshot(40.0, processes));
        for kind in kinds(plan.operations()) {
            assert!(
                matches!(
                    kind,
                    "SET_PROPERTY" | "MODEL_INSERT" | "MODEL_UPDATE" | "MODEL_DELETE"
                ),
                "forbidden steady-state operation: {kind}"
            );
        }
        state.commit(plan);
    }
}

#[test]
fn progress_properties_are_only_resent_when_their_value_changed() {
    let state = seeded_state(base_processes());

    // Same CPU, same memory: nothing to send.
    assert!(state
        .plan_snapshot(&snapshot(25.0, base_processes()))
        .is_empty());

    // Changed CPU only: exactly the CPU value and its description.
    let plan = state.plan_snapshot(&snapshot(55.0, base_processes()));
    assert_eq!(
        kinds(plan.operations()),
        vec!["SET_PROPERTY", "SET_PROPERTY"]
    );
    for operation in plan.operations() {
        match operation {
            Operation::SetProperty { id, .. } => assert_eq!(id, &CPU_PROGRESS_ID),
            other => panic!("expected SET_PROPERTY, got {other:?}"),
        }
    }
}

#[test]
fn committed_ticks_keep_the_published_model_in_sync() {
    let fixture = base_fixture();

    let mut processes = base_processes();
    processes.retain(|process| process.key.pid != 10);
    processes[0].cpu_percent = 80.0;
    processes.push(record(70, 8_000, "iota", 3.0, 3 * MIB, Some(UID)));
    fixture.source.publish(snapshot(66.0, processes));
    fixture.monitor.tick().expect("tick commits");

    let expected: Vec<(ItemId, Value)> = fixture.monitor.with_state(|state| {
        state
            .visible()
            .iter()
            .map(|row| (row.item_id, row.values.to_value()))
            .collect()
    });
    assert_eq!(model_rows(&fixture.session), expected);
}
