//! Kill authorization and safety coverage (§7.7, §27).

mod common;

use common::{base_processes, fixture, Fixture, MIB, UID};
use srui_example_process_monitor::testing::{activate_event, record, selection_event, snapshot};
use srui_example_process_monitor::*;
use srui_sdk::ItemId;

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
    assert_eq!(fixture.monitor.kill_selected(), KillOutcome::NoSelection);
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

    assert_eq!(fixture.monitor.kill_selected(), KillOutcome::NoSelection);
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

    assert_eq!(fixture.monitor.kill_selected(), KillOutcome::Denied(1));
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

    assert_eq!(fixture.monitor.kill_selected(), KillOutcome::Denied(own));
    assert!(fixture.terminator.calls().is_empty());
}

#[test]
fn a_start_time_mismatch_is_refused_as_stale() {
    let fixture = common::base_fixture();
    select(&fixture, 20);

    // The PID was recycled between the sample and the activation.
    fixture.source.set_live_start_time(20, Some(999_999));

    assert_eq!(
        fixture.monitor.kill_selected(),
        KillOutcome::StaleIdentity(20)
    );
    assert!(fixture.terminator.calls().is_empty());
}

#[test]
fn a_vanished_pid_is_refused_as_stale() {
    let fixture = common::base_fixture();
    select(&fixture, 20);
    fixture.source.set_live_start_time(20, None);

    assert_eq!(
        fixture.monitor.kill_selected(),
        KillOutcome::StaleIdentity(20)
    );
    assert!(fixture.terminator.calls().is_empty());
}

#[test]
fn a_valid_selection_signals_the_authoritative_numeric_pid_exactly_once() {
    let fixture = common::base_fixture();
    select(&fixture, 30);

    assert_eq!(fixture.monitor.kill_selected(), KillOutcome::Terminated(30));
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
    assert_eq!(
        fixture.monitor.kill_selected(),
        KillOutcome::Terminated(4_242)
    );
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
    let monitor = Monitor::start(
        session.clone(),
        source.boxed(),
        terminator.boxed(),
        Some(UID),
    )
    .expect("monitor starts");

    let item = monitor.with_state(|state| state.visible()[0].item_id);
    let event = selection_event(1, session.current_revision(), item);
    session.process_event(&event).expect("selection accepted");

    assert_eq!(
        monitor.kill_selected(),
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
    assert_eq!(fixture.session.current_revision(), revision);
}

#[test]
fn the_default_denylist_covers_pid_1_and_this_process() {
    let fixture = common::base_fixture();
    fixture.monitor.with_state(|state| {
        assert!(state.denylist().contains(&1));
        assert!(state.denylist().contains(&std::process::id()));
    });
}
