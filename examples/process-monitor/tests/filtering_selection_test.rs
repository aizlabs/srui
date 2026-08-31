//! Authoritative filtering and selection coverage (§7.6, §7.7, §27).

mod common;

use common::{base_fixture, base_processes, model_rows, MIB, OTHER_UID, UID};
use srui_example_process_monitor::testing::{
    argumentless_event, record, selection_event, snapshot, toggle_event,
};
use srui_example_process_monitor::*;
use srui_sdk::{ItemId, Toggle, Value, SELECTION_CHANGED, VALUE, VALUE_CHANGED};

fn visible_pids(monitor: &Monitor) -> Vec<u64> {
    monitor.with_state(|state| state.visible().iter().map(|row| row.values.pid).collect())
}

fn test_client_selection(monitor: &Monitor) -> Option<ItemId> {
    monitor.with_state(|state| state.selected_item_for_client(b"process-monitor-test"))
}

#[test]
fn show_all_false_includes_only_the_effective_users_processes() {
    let fixture = base_fixture();
    assert_eq!(visible_pids(&fixture.monitor), vec![10, 20, 30]);
    assert!(!fixture.monitor.with_state(MonitorState::show_all));
}

#[test]
fn show_all_true_includes_every_enumerated_process() {
    let fixture = base_fixture();
    let revision = fixture.session.current_revision();

    let event = toggle_event(1, revision, Value::Bool(true));
    fixture
        .session
        .process_event(&event)
        .expect("event accepted");

    assert_eq!(visible_pids(&fixture.monitor), vec![10, 20, 30, 40]);
    assert!(fixture.monitor.with_state(MonitorState::show_all));
    // The authoritative toggle value is confirmed back to the client in the same transaction.
    fixture.session.with_store(|store| {
        assert_eq!(Toggle::new(SHOW_ALL_ID).value(store), Some(true));
    });
    assert_eq!(fixture.session.current_revision(), revision + 1);
    assert_eq!(model_rows(&fixture.session).len(), 4);
}

#[test]
fn show_all_membership_is_recomputed_from_the_servers_latest_enumeration() {
    let fixture = base_fixture();

    // A foreign-user process appears while the filter still hides it.
    let mut processes = base_processes();
    processes.push(record(41, 1_000, "root-helper", 1.0, MIB, Some(OTHER_UID)));
    fixture.source.publish(snapshot(25.0, processes));
    fixture.monitor.tick().expect("tick");
    assert_eq!(visible_pids(&fixture.monitor), vec![10, 20, 30]);

    let event = toggle_event(1, fixture.session.current_revision(), Value::Bool(true));
    fixture
        .session
        .process_event(&event)
        .expect("event accepted");
    assert_eq!(visible_pids(&fixture.monitor), vec![10, 20, 30, 40, 41]);
}

#[test]
fn a_malformed_toggle_value_is_ignored_without_crashing() {
    let fixture = base_fixture();
    let revision = fixture.session.current_revision();

    let bogus = toggle_event(1, revision, Value::String("yes".to_string()));
    fixture
        .session
        .process_event(&bogus)
        .expect("event accepted");
    let bare = argumentless_event(2, revision, SHOW_ALL_ID, VALUE_CHANGED);
    fixture
        .session
        .process_event(&bare)
        .expect("event accepted");

    assert!(!fixture.monitor.with_state(MonitorState::show_all));
    assert_eq!(fixture.session.current_revision(), revision);
}

#[test]
fn a_known_selection_records_only_the_item_id() {
    let fixture = base_fixture();
    let item = fixture
        .monitor
        .with_state(|state| state.visible()[1].item_id);

    let event = selection_event(1, fixture.session.current_revision(), item);
    fixture
        .session
        .process_event(&event)
        .expect("event accepted");

    assert_eq!(
        test_client_selection(&fixture.monitor),
        Some(item)
    );
}

#[test]
fn an_unknown_selection_is_ignored() {
    let fixture = base_fixture();
    let revision = fixture.session.current_revision();

    let event = selection_event(1, revision, ItemId::new(9_999));
    fixture
        .session
        .process_event(&event)
        .expect("event accepted");

    assert_eq!(
        test_client_selection(&fixture.monitor),
        None
    );
    assert_eq!(fixture.session.current_revision(), revision);
}

#[test]
fn a_malformed_selection_argument_is_ignored() {
    let fixture = base_fixture();
    let revision = fixture.session.current_revision();

    let bare = argumentless_event(1, revision, PROCESS_TABLE_ID, SELECTION_CHANGED);
    fixture
        .session
        .process_event(&bare)
        .expect("event accepted");

    assert_eq!(
        test_client_selection(&fixture.monitor),
        None
    );
}

#[test]
fn selection_is_cleared_when_the_selected_process_exits() {
    let fixture = base_fixture();
    let item = fixture
        .monitor
        .with_state(|state| state.visible()[1].item_id);
    let event = selection_event(1, fixture.session.current_revision(), item);
    fixture
        .session
        .process_event(&event)
        .expect("event accepted");

    let remaining: Vec<ProcessRecord> = base_processes()
        .into_iter()
        .filter(|process| process.key.pid != 20)
        .collect();
    fixture.source.publish(snapshot(25.0, remaining));
    fixture.monitor.tick().expect("tick");

    assert_eq!(
        test_client_selection(&fixture.monitor),
        None
    );
}

#[test]
fn selection_is_cleared_when_the_selected_process_is_filtered_out() {
    let fixture = base_fixture();
    let revision = fixture.session.current_revision();
    fixture
        .session
        .process_event(&toggle_event(1, revision, Value::Bool(true)))
        .expect("event accepted");

    let foreign = fixture.monitor.with_state(|state| {
        state
            .visible()
            .iter()
            .find(|row| row.values.pid == 40)
            .expect("foreign row visible")
            .item_id
    });
    let revision = fixture.session.current_revision();
    fixture
        .session
        .process_event(&selection_event(2, revision, foreign))
        .expect("event accepted");
    assert_eq!(
        test_client_selection(&fixture.monitor),
        Some(foreign)
    );

    let revision = fixture.session.current_revision();
    fixture
        .session
        .process_event(&toggle_event(3, revision, Value::Bool(false)))
        .expect("event accepted");

    assert_eq!(
        test_client_selection(&fixture.monitor),
        None
    );
    fixture.session.with_store(|store| {
        assert_eq!(
            Toggle::new(SHOW_ALL_ID).value(store),
            Some(false),
            "show_all toggle property must update to false on the client"
        );
    });
}

#[test]
fn processes_without_a_resolvable_owner_are_excluded_until_show_all() {
    let fixture = common::fixture(snapshot(
        10.0,
        vec![
            record(11, 1, "owned", 1.0, MIB, Some(UID)),
            record(12, 1, "unowned", 1.0, MIB, None),
        ],
    ));
    // Filtered mode means exactly "owned by the effective user": unproven ownership is excluded.
    assert_eq!(visible_pids(&fixture.monitor), vec![11]);

    let event = toggle_event(1, fixture.session.current_revision(), Value::Bool(true));
    fixture
        .session
        .process_event(&event)
        .expect("event accepted");
    assert_eq!(visible_pids(&fixture.monitor), vec![11, 12]);
}

#[test]
fn multi_client_selections_are_isolated_and_cleared_independently() {
    let fixture = base_fixture();
    let revision = fixture.session.current_revision();

    let item_20 = fixture.monitor.with_state(|state| {
        state
            .visible()
            .iter()
            .find(|row| row.values.pid == 20)
            .unwrap()
            .item_id
    });
    let item_30 = fixture.monitor.with_state(|state| {
        state
            .visible()
            .iter()
            .find(|row| row.values.pid == 30)
            .unwrap()
            .item_id
    });

    // Client A selects PID 20
    let mut event_a = selection_event(1, revision, item_20);
    event_a.client_instance_id = b"client-a".to_vec();
    fixture
        .session
        .process_event(&event_a)
        .expect("client A selection accepted");

    // Client B selects PID 30
    let mut event_b = selection_event(2, revision, item_30);
    event_b.client_instance_id = b"client-b".to_vec();
    fixture
        .session
        .process_event(&event_b)
        .expect("client B selection accepted");

    assert_eq!(
        fixture
            .monitor
            .with_state(|s| s.selected_item_for_client(b"client-a")),
        Some(item_20)
    );
    assert_eq!(
        fixture
            .monitor
            .with_state(|s| s.selected_item_for_client(b"client-b")),
        Some(item_30)
    );

    // PID 20 exits
    let remaining: Vec<ProcessRecord> = base_processes()
        .into_iter()
        .filter(|process| process.key.pid != 20)
        .collect();
    fixture.source.publish(snapshot(25.0, remaining));
    fixture.monitor.tick().expect("tick");

    // Client A's selection is cleared because PID 20 exited, but Client B's selection of PID 30 remains intact!
    assert_eq!(
        fixture
            .monitor
            .with_state(|s| s.selected_item_for_client(b"client-a")),
        None
    );
    assert_eq!(
        fixture
            .monitor
            .with_state(|s| s.selected_item_for_client(b"client-b")),
        Some(item_30)
    );
}

#[test]
fn show_all_toggle_reaffirms_state_when_commit_fails() {
    let fixture = base_fixture();
    let initial_show_all = fixture.monitor.with_state(MonitorState::show_all);
    assert!(!initial_show_all);

    // If an invalid toggle event with non-boolean payload arrives, the toggle value remains false.
    let bogus_event = toggle_event(
        1,
        fixture.session.current_revision(),
        Value::String("invalid".into()),
    );
    fixture
        .session
        .process_event(&bogus_event)
        .expect("processed");

    assert!(!fixture.monitor.with_state(MonitorState::show_all));
    fixture.session.with_store(|store| {
        assert_eq!(
            store.get_node(SHOW_ALL_ID).unwrap().get_property(VALUE),
            Some(&Value::Bool(false))
        );
    });
}
