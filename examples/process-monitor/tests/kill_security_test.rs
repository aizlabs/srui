//! Kill authorization and safety coverage (§7.7, §27).

mod common;

use common::{base_processes, fixture, Fixture, MIB, UID};
use srui_example_process_monitor::testing::{
    activate_event, record, selection_event, snapshot, TEST_CLIENT_INSTANCE_ID,
};
use srui_example_process_monitor::*;
use srui_sdk::{ItemId, Text};

/// The kill-status text the client currently sees.
fn kill_status(fixture: &Fixture) -> Option<String> {
    fixture.session.with_store(|store| {
        Text::from_store(store, KILL_STATUS_ID)?
            .text(store)
            .map(str::to_string)
    })
}

/// Resolves the kill through the same client instance the event builders stamp, because a
/// selection is only ever actionable by the client that made it (§27).
fn kill(fixture: &Fixture) -> KillOutcome {
    fixture
        .monitor
        .kill_selected_for_client(Some(TEST_CLIENT_INSTANCE_ID.as_bytes()))
}

fn select(fixture: &Fixture, pid: u32) -> ItemId {
    let item = fixture.monitor.with_state(|state| {
        state
            .visible()
            .iter()
            .find(|row| row.values.pid == u64::from(pid))
            .unwrap_or_else(|| panic!("pid {pid} visible"))
            .item_id
    });
    let event = selection_event(1, fixture.session.current_revision(), item);
    fixture
        .session
        .process_event(&event)
        .expect("selection accepted");
    item
}

#[test]
fn no_selection_performs_no_termination() {
    let fixture = common::base_fixture();
    assert_eq!(kill(&fixture), KillOutcome::NoSelection);
    assert!(fixture.terminator.calls().is_empty());
}

#[test]
fn a_stale_selection_performs_no_termination() {
    let fixture = common::base_fixture();
    select(&fixture, 20);

    // The selected process exits: the next tick drops it from authoritative state.
    let remaining: Vec<ProcessRecord> = base_processes()
        .into_iter()
        .filter(|process| process.key.pid != 20)
        .collect();
    fixture.source.publish(snapshot(25.0, remaining));
    fixture.monitor.tick().expect("tick");

    assert_eq!(kill(&fixture), KillOutcome::NoSelection);
    assert!(fixture.terminator.calls().is_empty());
}

#[test]
fn pid_1_is_denied() {
    let fixture = fixture(snapshot(
        10.0,
        vec![
            record(1, 1, "init", 0.1, MIB, Some(UID)),
            record(20, 1_000, "beta", 2.0, 2 * MIB, Some(UID)),
        ],
    ));
    select(&fixture, 1);

    assert_eq!(kill(&fixture), KillOutcome::Denied(1));
    assert!(fixture.terminator.calls().is_empty());
}

#[test]
fn the_monitors_own_pid_is_denied() {
    let own = std::process::id();
    let fixture = fixture(snapshot(
        10.0,
        vec![record(own, 1, "process-monitor", 0.5, MIB, Some(UID))],
    ));
    select(&fixture, own);

    assert_eq!(kill(&fixture), KillOutcome::Denied(own));
    assert!(fixture.terminator.calls().is_empty());
}

#[test]
fn a_start_time_mismatch_is_refused_as_stale() {
    let fixture = common::base_fixture();
    select(&fixture, 20);

    // The PID was recycled between the sample and the activation.
    fixture.source.set_live_start_time(20, Some(999_999));

    assert_eq!(kill(&fixture), KillOutcome::StaleIdentity(20));
    assert!(fixture.terminator.calls().is_empty());
}

#[test]
fn a_vanished_pid_is_refused_as_stale() {
    let fixture = common::base_fixture();
    select(&fixture, 20);
    fixture.source.set_live_start_time(20, None);

    assert_eq!(kill(&fixture), KillOutcome::StaleIdentity(20));
    assert!(fixture.terminator.calls().is_empty());
}

#[test]
fn a_valid_selection_signals_the_authoritative_numeric_pid_exactly_once() {
    let fixture = common::base_fixture();
    select(&fixture, 30);

    assert_eq!(kill(&fixture), KillOutcome::Terminated(30));
    assert_eq!(fixture.terminator.calls(), vec![30]);
}

#[test]
fn client_visible_row_text_is_never_used_to_resolve_the_target() {
    // Row text that would be catastrophic if it were parsed as a PID.
    let fixture = fixture(snapshot(
        10.0,
        vec![
            record(10, 1, "1", 1.0, MIB, Some(UID)),
            record(4_242, 1, "init", 1.0, MIB, Some(UID)),
        ],
    ));
    let item = select(&fixture, 4_242);

    // The item id is not the pid either: identity resolves through server-owned state only.
    assert_ne!(item.get(), 4_242);
    assert_eq!(kill(&fixture), KillOutcome::Terminated(4_242));
    assert_eq!(fixture.terminator.calls(), vec![4_242]);
}

#[test]
fn an_os_refusal_is_reported_without_crashing() {
    let session = std::sync::Arc::new(srui_sessiond::Session::with_capabilities(
        "kill-failure",
        srui_sdk::ServerCapabilities::standard_widgets(),
    ));
    let source = srui_example_process_monitor::testing::FakeProcessSource::new(snapshot(
        5.0,
        vec![record(20, 1_000, "beta", 1.0, MIB, Some(UID))],
    ));
    let terminator = srui_example_process_monitor::testing::RecordingTerminator::with_result(Err(
        TerminateError::PermissionDenied,
    ));
    let monitor = Monitor::start(session.clone(), source.boxed(), terminator.boxed(), UID)
        .expect("monitor starts");

    let item = monitor.with_state(|state| state.visible()[0].item_id);
    let event = selection_event(1, session.current_revision(), item);
    session.process_event(&event).expect("selection accepted");

    assert_eq!(
        monitor.kill_selected_for_client(Some(TEST_CLIENT_INSTANCE_ID.as_bytes())),
        KillOutcome::Failed(20, TerminateError::PermissionDenied)
    );
    assert_eq!(terminator.calls(), vec![20]);
}

#[test]
fn a_malformed_activation_does_not_crash_or_terminate() {
    let fixture = common::base_fixture();
    let revision = fixture.session.current_revision();

    // ACTIVATE with no selection recorded, delivered through the real event path.
    let event = activate_event(1, revision, KILL_BUTTON_ID);
    fixture
        .session
        .process_event(&event)
        .expect("event accepted");

    assert!(fixture.terminator.calls().is_empty());
    // The only state change is the refusal reported back to the client.
    assert_eq!(fixture.session.current_revision(), revision + 1);
}

#[test]
fn every_kill_outcome_is_reported_to_the_client_as_semantic_state() {
    let fixture = common::base_fixture();

    let refusal = activate_event(1, fixture.session.current_revision(), KILL_BUTTON_ID);
    fixture
        .session
        .process_event(&refusal)
        .expect("event accepted");
    assert_eq!(
        kill_status(&fixture),
        Some("Select a process first".to_string())
    );

    let item = fixture.monitor.with_state(|state| {
        state
            .visible()
            .iter()
            .find(|row| row.values.pid == 30)
            .unwrap()
            .item_id
    });
    fixture
        .session
        .process_event(&selection_event(
            2,
            fixture.session.current_revision(),
            item,
        ))
        .expect("selection accepted");

    let success = activate_event(3, fixture.session.current_revision(), KILL_BUTTON_ID);
    fixture
        .session
        .process_event(&success)
        .expect("event accepted");
    assert_eq!(fixture.terminator.calls(), vec![30]);
    assert_eq!(
        kill_status(&fixture),
        Some("SIGTERM delivered to PID 30".to_string())
    );
}

#[test]
fn the_default_denylist_covers_pid_0_pid_1_and_this_process() {
    let fixture = common::base_fixture();
    fixture.monitor.with_state(|state| {
        assert!(state.denylist().contains(&0));
        assert!(state.denylist().contains(&1));
        assert!(state.denylist().contains(&std::process::id()));
    });
}

#[test]
fn pid_0_is_denied_because_it_would_signal_a_process_group() {
    // macOS reports a real PID 0 (`kernel_task`), so this row can genuinely be selected.
    let fixture = fixture(snapshot(
        10.0,
        vec![
            record(0, 1, "kernel_task", 1.0, MIB, Some(UID)),
            record(20, 1_000, "beta", 2.0, 2 * MIB, Some(UID)),
        ],
    ));
    select(&fixture, 0);

    assert_eq!(kill(&fixture), KillOutcome::Denied(0));
    assert!(fixture.terminator.calls().is_empty());
}

#[test]
fn kill_activation_is_scoped_to_the_activating_client() {
    let fixture = common::base_fixture();
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

    // Client A selects PID 20.
    let mut event_a = selection_event(1, revision, item_20);
    event_a.client_instance_id = b"client-a".to_vec();
    fixture
        .session
        .process_event(&event_a)
        .expect("client A selection accepted");

    // Client B selects PID 30.
    let mut event_b = selection_event(2, revision, item_30);
    event_b.client_instance_id = b"client-b".to_vec();
    fixture
        .session
        .process_event(&event_b)
        .expect("client B selection accepted");

    // Client A activates Kill Selected: must kill PID 20, NOT Client B's selection (PID 30).
    let mut kill_a = activate_event(3, revision, KILL_BUTTON_ID);
    kill_a.client_instance_id = b"client-a".to_vec();
    fixture
        .session
        .process_event(&kill_a)
        .expect("client A kill accepted");

    assert_eq!(fixture.terminator.calls(), vec![20]);

    // Client B activates Kill Selected: must kill PID 30.
    let mut kill_b = activate_event(4, revision, KILL_BUTTON_ID);
    kill_b.client_instance_id = b"client-b".to_vec();
    fixture
        .session
        .process_event(&kill_b)
        .expect("client B kill accepted");

    assert_eq!(fixture.terminator.calls(), vec![20, 30]);
}

#[test]
fn client_b_without_selection_cannot_kill_client_a_selection() {
    let fixture = common::base_fixture();
    let revision = fixture.session.current_revision();

    let item_20 = fixture.monitor.with_state(|state| {
        state
            .visible()
            .iter()
            .find(|row| row.values.pid == 20)
            .unwrap()
            .item_id
    });

    // Client A selects PID 20.
    let mut event_a = selection_event(1, revision, item_20);
    event_a.client_instance_id = b"client-a".to_vec();
    fixture
        .session
        .process_event(&event_a)
        .expect("client A selection accepted");

    // Client B has never made any selection, but sends ACTIVATE on Kill Selected.
    let mut kill_b = activate_event(2, revision, KILL_BUTTON_ID);
    kill_b.client_instance_id = b"client-b".to_vec();
    fixture
        .session
        .process_event(&kill_b)
        .expect("client B kill event processed");

    // Must NOT kill Client A's selection (PID 20) or any other process.
    assert!(fixture.terminator.calls().is_empty());
    assert_eq!(
        fixture.monitor.kill_selected_for_client(Some(b"client-b")),
        KillOutcome::NoSelection
    );
}

#[test]
fn an_unidentified_client_cannot_kill_another_clients_selection() {
    let fixture = common::base_fixture();
    let revision = fixture.session.current_revision();

    let item_20 = fixture.monitor.with_state(|state| {
        state
            .visible()
            .iter()
            .find(|row| row.values.pid == 20)
            .unwrap()
            .item_id
    });

    // Client A selects PID 20, and is the only client in the session holding a selection.
    let mut event_a = selection_event(1, revision, item_20);
    event_a.client_instance_id = b"client-a".to_vec();
    fixture
        .session
        .process_event(&event_a)
        .expect("client A selection accepted");

    // A client that handshook without a client instance id activates "Kill Selected". Nothing
    // identifies it as the owner of A's selection, so nothing is signalled.
    let mut kill_anonymous = activate_event(2, revision, KILL_BUTTON_ID);
    kill_anonymous.client_instance_id = Vec::new();
    fixture
        .session
        .process_event(&kill_anonymous)
        .expect("anonymous kill event processed");

    assert!(fixture.terminator.calls().is_empty());
    assert_eq!(
        fixture
            .monitor
            .with_state(|state| state.selected_item_for_client(b"client-a")),
        Some(item_20)
    );
}

#[test]
fn an_unidentified_client_cannot_record_a_selection() {
    let fixture = common::base_fixture();
    let revision = fixture.session.current_revision();

    let item_20 = fixture.monitor.with_state(|state| {
        state
            .visible()
            .iter()
            .find(|row| row.values.pid == 20)
            .unwrap()
            .item_id
    });

    let mut event = selection_event(1, revision, item_20);
    event.client_instance_id = Vec::new();
    fixture
        .session
        .process_event(&event)
        .expect("anonymous selection event processed");

    // Refused outright rather than parked in a bucket a later activation could resolve.
    assert_eq!(
        fixture.monitor.with_state(MonitorState::selected_item),
        None
    );
    assert_eq!(kill(&fixture), KillOutcome::NoSelection);
}
