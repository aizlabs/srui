//! PX-012 acceptance: filter by owner and by an explicit PID set (D1 §§6–8,
//! 12, 22, 29; D2 T9–T11, T16, T21).
//!
//! * The UID is the owner. "My processes", a typed UID and a typed account
//!   name each select by UID; a name shared by two UIDs keeps both, and a UID
//!   with no account is still selectable by number.
//! * An unreadable owner is never UID 0: a specific user filter hides it and
//!   says how many it hid.
//! * A PID set keeps the PIDs it names and states the ones no visible process
//!   has; an entry that is not a PID is named and keeps nothing.
//! * The user filter, the PID set and the text filter combine by conjunction,
//!   and an empty result says what each filter would keep on its own.
//! * Changing any filter never creates a process identity: every row keeps its
//!   item ID, hidden or shown, and clearing the filters restores the original
//!   rows exactly. A filter that hides the promoted search match lets the SDK's
//!   settlement clear it in the same commit.
//! * A delayed or hung account-name lookup never stalls publication.

use srui_process_explorer::owner::{
    AccountLookup, Accounts, OwnerChoice, MAX_OWNER_INPUT_BYTES, MAX_OWNER_STATUS_BYTES,
    MAX_PID_SET, MAX_PID_SET_INPUT_BYTES,
};
use srui_process_explorer::refresh::ProcessView;
use srui_process_explorer::source::*;
use srui_process_explorer::{
    install_filter_handlers, install_owner_handlers, install_search_handlers, start_from_source,
    FILTER_STATUS, MODEL, OWNER_ALL, OWNER_INPUT, OWNER_MINE, OWNER_STATUS, PID_SET_INPUT,
    SEARCH_STATUS, TABLE,
};
use srui_protocol::Event as WireEvent;
use srui_sdk::{ItemId, NodeId, Operation, Value, ENABLED, TEXT, VALUE};
use srui_semantic_tree::{EditSeq, Event as DomainEvent};
use srui_sessiond::{EventOutcome, Session};
use std::collections::BTreeSet;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const SOURCE: &str = "owner-fixture-v1";
const CLIENT: &[u8] = b"owner-client-instance";
/// The fixture daemon's own account.
const ME: u32 = 1000;
/// Two accounts that share one name.
const OPS_A: u32 = 1001;
const OPS_B: u32 = 1002;
/// A UID the fixture database has no entry for.
const UNNAMED: u32 = 4242;
/// How long a test waits for the fixture database's worker.
const SETTLE: Duration = Duration::from_secs(10);

fn record(pid: u32, name: &str, owner: Observed<u32>) -> ProcessRecord {
    ProcessRecord {
        key: ProcessKey {
            source: SourceId(SOURCE.into()),
            host: Observed::Known(HostId("fixture-host".into())),
            boot: Observed::Known(BootId("fixture-boot".into())),
            pid_namespace: Observed::Known(PidNamespaceId(4_026_531_836)),
            pid: Observed::Known(pid),
            creation: CreationToken::Opaque(format!("{name}-{pid}")),
        },
        display_name: DisplayName::from(name),
        resident: Observed::Known(4096),
        cpu: CpuUsage::WarmingUp,
        owner,
    }
}

/// Six processes: three named exactly `worker` under three different owners,
/// two of whose accounts share the name `ops`; one owned by a UID with no
/// account; and one whose owner could not be read.
fn owners() -> Vec<ProcessRecord> {
    vec![
        record(100, "worker", Observed::Known(ME)),
        record(101, "editor", Observed::Known(ME)),
        record(102, "worker", Observed::Known(OPS_A)),
        record(103, "worker", Observed::Known(OPS_B)),
        record(104, "daemon", Observed::Known(UNNAMED)),
        record(105, "secret", Observed::Missing(MissingReason::Denied)),
    ]
}

fn fixture_accounts() -> Accounts {
    Accounts::fixed(ME, &[(ME, "alice"), (OPS_A, "ops"), (OPS_B, "ops")])
}

struct Fixture {
    records: Vec<ProcessRecord>,
    accounts: Accounts,
}

impl Fixture {
    fn new(accounts: Accounts) -> Self {
        Self {
            records: owners(),
            accounts,
        }
    }
}

impl ProcessSource for Fixture {
    fn status_text(&self) -> &str {
        "owner fixture"
    }

    fn snapshot(&mut self) -> ProcessSnapshot {
        ProcessSnapshot {
            sampled_at: SnapshotTime(std::time::UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
            source: SourceId(SOURCE.into()),
            records: self.records.clone(),
            vanished: 0,
            capped: CappedRecords::none(),
            system: SystemSample::not_provided(),
            completeness: Completeness::Complete,
        }
    }

    fn accounts(&self) -> Accounts {
        self.accounts.clone()
    }
}

/// Every row the client holds, in published order: item ID and PID.
fn rows(session: &Session) -> Vec<(ItemId, u64)> {
    session.with_store(|store| {
        let model = store.get_model(MODEL).expect("the collection model exists");
        assert_eq!(model.item_count, model.items.len() as u64);
        model
            .items
            .values()
            .map(|item| {
                let Value::List(cells) = &item.value else {
                    panic!("a row is a list of cells")
                };
                let Value::UnsignedInt(pid) = cells[0] else {
                    panic!("the first cell is the PID")
                };
                (item.item_id, pid)
            })
            .collect()
    })
}

fn pids(session: &Session) -> Vec<u64> {
    rows(session).into_iter().map(|(_, pid)| pid).collect()
}

fn text(session: &Session, node: NodeId) -> String {
    session.with_store(
        |store| match store.get_node(node).and_then(|n| n.get_property(TEXT)) {
            Some(Value::String(text)) => text.clone(),
            other => panic!("node {node:?} has no text: {other:?}"),
        },
    )
}

fn value(session: &Session, node: NodeId) -> String {
    session.with_store(
        |store| match store.get_node(node).and_then(|n| n.get_property(VALUE)) {
            Some(Value::String(text)) => text.clone(),
            _ => String::new(),
        },
    )
}

fn enabled(session: &Session, node: NodeId) -> bool {
    session.with_store(|store| {
        store.get_node(node).and_then(|n| n.get_property(ENABLED)) == Some(&Value::Bool(true))
    })
}

/// The user-filter status the client holds, which must also be the view's
/// record of it.
fn status(session: &Session, view: &ProcessView) -> String {
    let held = text(session, OWNER_STATUS);
    assert_eq!(held, view.owner_status(), "the view records what committed");
    held
}

fn operations_since(session: &Session, revision: u64) -> Vec<Operation> {
    session
        .collect_replayed_transactions(revision)
        .expect("the journal holds this run")
        .into_iter()
        .flat_map(|transaction| transaction.operations)
        .map(|op| Operation::try_from(op).expect("a committed operation decodes"))
        .collect()
}

/// Every item ID any operation since `revision` inserted.
fn inserted_since(session: &Session, revision: u64) -> BTreeSet<ItemId> {
    operations_since(session, revision)
        .into_iter()
        .flat_map(|op| match op {
            Operation::ModelInsert { items, .. } => {
                items.into_iter().map(|item| item.item_id).collect()
            }
            _ => Vec::new(),
        })
        .collect()
}

/// A started view whose fixture database has answered for every owner.
fn started(accounts: Accounts) -> (Session, Fixture, ProcessView) {
    let session = Session::mint();
    let mut source = Fixture::new(accounts);
    let (view, _) = start_from_source(&session, &mut source).unwrap();
    (session, source, view)
}

/// Asks the cache about every fixture owner and waits for its answers, so a
/// test that is not about delay sees resolved names.
fn settle_names(view: &ProcessView) {
    for uid in [0, ME, OPS_A, OPS_B, UNNAMED] {
        let _ = view.owner_filter().names().name(uid);
    }
    assert!(view.owner_filter().names().wait_idle(SETTLE));
}

#[test]
fn my_processes_and_all_visible_users_change_rows_but_never_identities() {
    let (session, mut source, mut view) = started(fixture_accounts());
    settle_names(&view);
    let original = rows(&session);
    let start = session.current_revision();
    assert_eq!(pids(&session), [100, 101, 102, 103, 104, 105]);
    assert_eq!(
        status(&session, &view),
        "User: all users visible to this daemon's account · showing all 6 processes"
    );
    // "All visible users" is the choice, so only "Only my processes" is enabled.
    assert!(enabled(&session, OWNER_MINE) && !enabled(&session, OWNER_ALL));

    view.choose_owner(&session, OwnerChoice::Current).unwrap();
    assert_eq!(pids(&session), [100, 101]);
    let mine = status(&session, &view);
    assert_eq!(
        mine,
        "User: mine, UID 1000 (alice) · 2 of 6 processes match · 1 process with an \
         unreadable owner hidden"
    );
    assert!(!enabled(&session, OWNER_MINE) && enabled(&session, OWNER_ALL));
    // The filtered rows are the same items as before, in the same order.
    let kept: Vec<(ItemId, u64)> = original
        .iter()
        .filter(|(_, pid)| *pid == 100 || *pid == 101)
        .copied()
        .collect();
    assert_eq!(rows(&session), kept);
    // A refresh of the same host under the filter changes nothing at all.
    let quiet = view.refresh(&session, &mut source).unwrap();
    assert!(!quiet.published(), "{quiet:?}");

    view.choose_owner(&session, OwnerChoice::AllVisible)
        .unwrap();
    assert_eq!(
        rows(&session),
        original,
        "every row is back under its item ID"
    );
    assert!(enabled(&session, OWNER_MINE) && !enabled(&session, OWNER_ALL));
    let inserted = inserted_since(&session, start);
    let known: BTreeSet<ItemId> = original.iter().map(|(id, _)| *id).collect();
    assert!(
        inserted.is_subset(&known),
        "filtering re-inserted only existing items: {inserted:?}"
    );
}

#[test]
fn a_uid_a_name_two_uids_share_and_a_uid_with_no_account_select_by_uid() {
    let (session, _source, mut view) = started(fixture_accounts());
    settle_names(&view);
    let original = rows(&session);

    view.set_owner_input(&session, "1001").unwrap();
    assert_eq!(pids(&session), [102]);
    assert!(status(&session, &view).starts_with("User: UID 1001 (ops) · 1 of 6 processes match"));
    assert_eq!(value(&session, OWNER_INPUT), "1001");
    // Typing selects a user, so "All visible users" can return to all.
    assert!(enabled(&session, OWNER_MINE) && enabled(&session, OWNER_ALL));

    // Duplicate account names: both UIDs named `ops` are kept.
    view.set_owner_input(&session, "ops").unwrap();
    assert_eq!(pids(&session), [102, 103]);
    assert!(
        status(&session, &view).starts_with(
            "User: name \"ops\", UIDs 1001, 1002 (2 accounts share this name) · 2 of 6 \
             processes match"
        ),
        "{}",
        view.owner_status()
    );

    // A UID the database cannot name is still selectable by number.
    view.set_owner_input(&session, "4242").unwrap();
    assert_eq!(pids(&session), [104]);
    assert!(status(&session, &view).starts_with("User: UID 4242 (no account name) · 1 of 6"));

    // An unreadable owner is never UID 0.
    view.set_owner_input(&session, "0").unwrap();
    assert_eq!(pids(&session), Vec::<u64>::new());
    assert_eq!(
        status(&session, &view),
        "User: UID 0 (no account name) · no process of 6 matches every filter: the user \
         filter alone keeps 0 · 1 process with an unreadable owner hidden"
    );

    // No owner has the name: an explained empty result.
    view.set_owner_input(&session, "mallory").unwrap();
    assert_eq!(pids(&session), Vec::<u64>::new());
    assert!(status(&session, &view).starts_with(
        "User: name \"mallory\": no visible process owner has this account name · no process \
         of 6 matches every filter: the user filter alone keeps 0"
    ));

    // Digits beyond a UID are refused as one, not read as a name.
    view.set_owner_input(&session, "99999999999").unwrap();
    assert!(status(&session, &view)
        .starts_with("User: \"99999999999\" is not a UID (at most 4294967295)"));

    view.set_owner_input(&session, "").unwrap();
    assert_eq!(rows(&session), original);
    assert!(!enabled(&session, OWNER_ALL));
}

#[test]
fn a_pid_set_names_absent_pids_and_combines_with_the_user_and_text_filters() {
    let (session, _source, mut view) = started(fixture_accounts());
    settle_names(&view);
    let original = rows(&session);

    view.set_pid_set(&session, "100, 102 9999 9998 102")
        .unwrap();
    assert_eq!(pids(&session), [100, 102]);
    assert_eq!(
        status(&session, &view),
        "User: all users visible to this daemon's account · PID set: 4 PIDs · no visible \
         process has PIDs 9998, 9999 · 2 of 6 processes match"
    );
    assert!(
        text(&session, FILTER_STATUS).contains("2 of 6 rows shown by the user filter and PID set")
    );

    // Conjunction with the user filter.
    view.set_owner_input(&session, "ops").unwrap();
    assert_eq!(pids(&session), [102]);
    // And with the text filter: nothing matches all three, and each filter's
    // own count is stated.
    view.set_filter(&session, "editor").unwrap();
    assert_eq!(pids(&session), Vec::<u64>::new());
    let empty = status(&session, &view);
    assert!(
        empty.ends_with(
            "no process of 6 matches every filter: the user filter alone keeps 2, the PID set \
             alone keeps 2, the text filter alone keeps 1 · 1 process with an unreadable \
             owner hidden"
        ),
        "{empty}"
    );
    assert!(text(&session, FILTER_STATUS).contains("counted after the user filter and PID set"));

    // A set whose entries are not PIDs keeps nothing and says why.
    view.set_filter(&session, "").unwrap();
    view.set_owner_input(&session, "").unwrap();
    view.set_pid_set(&session, "abc").unwrap();
    assert_eq!(pids(&session), Vec::<u64>::new());
    assert!(status(&session, &view).contains("PID set: 0 PIDs · 1 entry is not a PID: \"abc\""));

    view.set_pid_set(&session, "").unwrap();
    assert_eq!(
        rows(&session),
        original,
        "every row is back under its item ID"
    );
    assert_eq!(
        text(&session, FILTER_STATUS),
        "Committed filter (gen 2): none · showing all 6 rows"
    );
}

#[test]
fn a_filter_that_hides_the_promoted_match_clears_it_in_the_same_commit() {
    let (session, _source, mut view) = started(fixture_accounts());
    settle_names(&view);
    view.set_search(&session, "worker").unwrap();
    let promoted = view
        .promoted_selection()
        .expect("the first worker is promoted");
    assert_eq!(rows(&session)[0], (promoted, 100));
    let before = session.current_revision();
    view.set_owner_input(&session, "ops").unwrap();
    assert_eq!(pids(&session), [102, 103]);
    assert_eq!(view.promoted_selection(), None);
    let transactions = session.collect_replayed_transactions(before).unwrap();
    let clearing: Vec<_> = transactions
        .iter()
        .filter(|transaction| {
            transaction.operations.iter().any(|op| {
                matches!(
                    Operation::try_from(op.clone()),
                    Ok(Operation::ClearProperty { id, property }) if id == TABLE && property == VALUE
                )
            })
        })
        .collect();
    assert_eq!(clearing.len(), 1, "settlement clears the promotion once");
    assert!(
        clearing[0].operations.iter().any(|op| matches!(
            Operation::try_from(op.clone()),
            Ok(Operation::ModelDelete { .. })
        )),
        "in the commit that removes the row"
    );
    // The search keeps the same target and says it is no longer shown.
    assert!(text(&session, SEARCH_STATUS).contains("is no longer shown"));
}

/// A database that answers only once the test opens its gate, and counts the
/// questions it was asked.
#[derive(Default)]
struct Gated {
    open: Mutex<bool>,
    opened: Condvar,
    asked: Mutex<Vec<u32>>,
}

impl Gated {
    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.opened.notify_all();
    }
}

impl AccountLookup for Gated {
    fn account_name(&self, uid: u32) -> Result<Option<Vec<u8>>, MissingReason> {
        self.asked.lock().unwrap().push(uid);
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.opened.wait(open).unwrap();
        }
        Ok((uid == ME).then(|| b"alice".to_vec()))
    }
}

/// Opens the gate when dropped, so a failing test never leaves the worker
/// blocked behind it.
struct Release(Arc<Gated>);

impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[test]
fn a_delayed_name_lookup_never_stalls_publication() {
    let gate = Arc::new(Gated::default());
    let _release = Release(Arc::clone(&gate));
    let accounts =
        Accounts::with_lookup(Observed::Known(ME), gate.clone() as Arc<dyn AccountLookup>)
            .with_timeout(Duration::from_millis(200));
    let session = Arc::new(Session::mint());
    let mut source = Fixture::new(accounts);
    let begun = Instant::now();
    let (view, _) = start_from_source(&session, &mut source).unwrap();
    let view = Arc::new(Mutex::new(view));
    install_filter_handlers(&session, &view);
    install_owner_handlers(&session, &view);
    // The worker is now blocked on the daemon's own name.
    let deadline = Instant::now() + SETTLE;
    while gate.asked.lock().unwrap().is_empty() {
        assert!(Instant::now() < deadline, "the worker was asked for a name");
        std::thread::sleep(Duration::from_millis(5));
    }

    // Every publication answers at once while the lookup hangs: a name filter
    // that cannot be resolved yet hides the unresolved owners and says so.
    let fast = Duration::from_secs(1);
    let edit = Instant::now();
    processed(
        session
            .process_event(&text_edit(1, OWNER_INPUT, "alice"))
            .unwrap(),
    );
    assert!(edit.elapsed() < fast, "an edit waited {:?}", edit.elapsed());
    assert_eq!(pids(&session), Vec::<u64>::new());
    let waiting = text(&session, OWNER_STATUS);
    assert!(
        waiting.starts_with(
            "User: name \"alice\": no resolved process owner has this account name yet; 4 \
             owners' names are not resolved yet, so their processes stay hidden until resolved"
        ),
        "{waiting}"
    );
    for _ in 0..20 {
        let tick = Instant::now();
        view.lock().unwrap().refresh(&session, &mut source).unwrap();
        assert!(
            tick.elapsed() < fast,
            "a refresh waited {:?}",
            tick.elapsed()
        );
    }
    // A UID still selects at once, whatever its name is doing.
    processed(
        session
            .process_event(&text_edit(2, OWNER_INPUT, "1000"))
            .unwrap(),
    );
    assert_eq!(pids(&session), [100, 101]);
    // Past the timeout the name is published as timed out, not as pending,
    // and never as a guess.
    std::thread::sleep(Duration::from_millis(300));
    processed(
        session
            .process_event(&activate(3, OWNER_MINE, session.current_revision()))
            .unwrap(),
    );
    assert_eq!(pids(&session), [100, 101]);
    assert!(
        text(&session, OWNER_STATUS).starts_with("User: mine, UID 1000 (name lookup timed out)")
    );
    assert_eq!(
        value(&session, OWNER_INPUT),
        "",
        "a button empties the user field"
    );
    assert!(
        begun.elapsed() < Duration::from_secs(8),
        "nothing waited for the database"
    );

    // The late answer is still recorded, and the next publication uses it.
    gate.release();
    assert!(view
        .lock()
        .unwrap()
        .owner_filter()
        .names()
        .wait_idle(SETTLE));
    view.lock().unwrap().refresh(&session, &mut source).unwrap();
    assert!(text(&session, OWNER_STATUS).starts_with("User: mine, UID 1000 (alice) · 2 of 6"));
    processed(
        session
            .process_event(&text_edit(4, OWNER_INPUT, "alice"))
            .unwrap(),
    );
    assert_eq!(pids(&session), [100, 101]);
}

#[test]
fn the_user_filter_controls_are_driven_by_standard_client_events() {
    let session = Arc::new(Session::mint());
    let mut source = Fixture::new(fixture_accounts());
    let (view, _) = start_from_source(&session, &mut source).unwrap();
    settle_names(&view);
    let view = Arc::new(Mutex::new(view));
    install_filter_handlers(&session, &view);
    install_search_handlers(&session, &view);
    install_owner_handlers(&session, &view);
    let original = rows(&session);
    // "All visible users" is already the choice: the button is disabled, and
    // event validation refuses a click.
    let refused = session
        .process_event(&activate(1, OWNER_ALL, session.current_revision()))
        .unwrap();
    assert!(
        matches!(refused, EventOutcome::Rejected { .. }),
        "{refused:?}"
    );
    processed(
        session
            .process_event(&activate(2, OWNER_MINE, session.current_revision()))
            .unwrap(),
    );
    assert_eq!(pids(&session), [100, 101]);
    processed(
        session
            .process_event(&text_edit(3, PID_SET_INPUT, "101 7777"))
            .unwrap(),
    );
    assert_eq!(pids(&session), [101]);
    assert_eq!(value(&session, PID_SET_INPUT), "101 7777");
    assert!(text(&session, OWNER_STATUS).contains("no visible process has PID 7777"));
    processed(
        session
            .process_event(&activate(4, OWNER_ALL, session.current_revision()))
            .unwrap(),
    );
    assert_eq!(pids(&session), [101]);
    processed(
        session
            .process_event(&text_edit(5, PID_SET_INPUT, ""))
            .unwrap(),
    );
    assert_eq!(rows(&session), original);
}

#[test]
fn the_user_filter_state_and_status_stay_inside_their_bounds() {
    let (session, _source, mut view) = started(fixture_accounts());
    settle_names(&view);
    let wide = "\u{20000}".repeat(MAX_OWNER_INPUT_BYTES);
    view.set_owner_input(&session, &wide).unwrap();
    assert!(value(&session, OWNER_INPUT).len() <= MAX_OWNER_INPUT_BYTES);
    assert!(view.owner_filter().input().len() <= MAX_OWNER_INPUT_BYTES);
    // Many absent PIDs, many entries that are not PIDs, and more PIDs than a
    // set holds.
    let many: String = (1..=400u32)
        .map(|pid| format!("{} x{pid} ", 1_000_000 + pid))
        .collect();
    view.set_pid_set(&session, &many).unwrap();
    assert!(value(&session, PID_SET_INPUT).len() <= MAX_PID_SET_INPUT_BYTES);
    assert!(view.owner_filter().pid_set().pids().len() <= MAX_PID_SET);
    let held = status(&session, &view);
    assert!(
        held.len() <= MAX_OWNER_STATUS_BYTES + '…'.len_utf8(),
        "{}",
        held.len()
    );
    assert!(held.contains("and ") && held.contains("more"), "{held}");
    let full: String = (1..=200u32).map(|pid| format!("{pid},")).collect();
    view.set_pid_set(&session, &full).unwrap();
    assert_eq!(view.owner_filter().pid_set().pids().len(), MAX_PID_SET);
    assert!(status(&session, &view).contains(&format!("bounded to {MAX_PID_SET} PIDs")));
}

#[test]
fn a_source_that_states_no_accounts_is_explained_rather_than_guessed() {
    let (session, _source, mut view) = started(Accounts::not_provided());
    view.choose_owner(&session, OwnerChoice::Current).unwrap();
    assert_eq!(pids(&session), Vec::<u64>::new());
    assert!(status(&session, &view)
        .starts_with("User: mine, but this source does not state the daemon's account"));
    view.set_owner_input(&session, "alice").unwrap();
    assert!(status(&session, &view)
        .starts_with("User: name \"alice\", but this source names no accounts: type a UID"));
    view.set_owner_input(&session, "1001").unwrap();
    assert_eq!(pids(&session), [102]);
    assert!(status(&session, &view).starts_with("User: UID 1001 · 1 of 6 processes match"));
}

fn text_edit(seq: u64, node: NodeId, text: &str) -> WireEvent {
    DomainEvent::text_edit(
        seq,
        format!("owner-edit-{seq}"),
        0u64,
        node.get(),
        text,
        EditSeq::new(seq).expect("positive edit_seq"),
    )
    .with_client_instance_id(CLIENT.to_vec())
    .to_wire()
}

fn activate(seq: u64, node: NodeId, observed: u64) -> WireEvent {
    DomainEvent::activate(seq, format!("owner-click-{seq}"), observed, node)
        .with_client_instance_id(CLIENT.to_vec())
        .to_wire()
}

fn processed(outcome: EventOutcome) {
    assert!(
        matches!(outcome, EventOutcome::Processed { .. }),
        "not processed: {outcome:?}"
    );
}
