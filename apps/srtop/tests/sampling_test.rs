//! PX-015 acceptance: a bounded sampling interval, a clearly labelled paused
//! view, and manual refresh (D1 §§12, 18, 20.4; D2 T25).
//!
//! * Pause freezes this app's published sample view at a defined revision:
//!   while the source keeps changing, no row, cell or summary figure changes,
//!   and the loop takes no sample at all (collection stops). A scan that was
//!   running when the view was paused is discarded when it returns.
//! * Session and control traffic keep flowing: the filter, the sort buttons
//!   and the inspector keep committing their own transactions, re-projecting
//!   the frozen rows, and the sampling status and freshness line say the view
//!   is paused and since when.
//! * Resume publishes one current consistent view through the ordinary diff:
//!   one sample, never a replay of the missed ones, every surviving row keeping
//!   its item ID.
//! * The interval is validated on the server; a refused value is explained and
//!   the previous interval kept, and a change restarts the loop's one ticker
//!   without a burst. Refresh now takes one coalesced sample and is disabled
//!   while paused. Every timer ends with the loop on shutdown.

use srui_process_explorer::catalogue::ProcessField;
use srui_process_explorer::inspector::{DetailRead, DetailReadsIdle, DetailSource};
use srui_process_explorer::refresh::{lock_view, poll, ProcessView, SharedView};
use srui_process_explorer::sampling::{
    PAUSED_FRESHNESS, PAUSE_LABEL, RESUME_LABEL, RESUMING_FRESHNESS,
};
use srui_process_explorer::source::*;
use srui_process_explorer::summary::{FRESHNESS_TEXT, LAYOUT};
use srui_process_explorer::{
    install_filter_handlers, install_inspector, install_sampling_handlers, install_sort_handlers,
    FILTER_INPUT, FILTER_STATUS, INSPECTOR_DETAILS, INTERVAL_APPLY, INTERVAL_INPUT, MODEL,
    SAMPLING_PAUSE, SAMPLING_REFRESH, SAMPLING_STATUS, SORT_NAME, TABLE,
};
use srui_protocol::Event as WireEvent;
use srui_sdk::{ItemId, NodeId, PropertyRef, TypeRef, Value, ENABLED, LABEL, TEXT, VALUE};
use srui_semantic_tree::{EditSeq, Event as DomainEvent};
use srui_sessiond::{EventOutcome, Session};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const SOURCE: &str = "sampling-fixture-v1";
const CLIENT: &[u8] = b"px015-sampling-client";
const WAIT: Duration = Duration::from_secs(10);

fn key(pid: u32) -> ProcessKey {
    ProcessKey {
        source: SourceId(SOURCE.into()),
        host: Observed::Known(HostId("fixture-host".into())),
        boot: Observed::Known(BootId("fixture-boot".into())),
        pid_namespace: Observed::Known(PidNamespaceId(4_026_531_836)),
        pid: Observed::Known(pid),
        creation: CreationToken::Opaque(format!("token-{pid}")),
    }
}

fn record(pid: u32, name: &str, resident: u64) -> ProcessRecord {
    ProcessRecord {
        lineage: srui_process_explorer::source::Lineage::NOT_COLLECTED,
        owner: OWNER_NOT_COLLECTED,
        key: key(pid),
        display_name: name.into(),
        resident: Observed::Known(resident),
        cpu: CpuUsage::WarmingUp,
    }
}

/// The records a test changes, as real workers would come and go.
#[derive(Clone, Default)]
struct Host(Arc<Mutex<Vec<ProcessRecord>>>);

impl Host {
    fn set(&self, records: Vec<ProcessRecord>) {
        *self.0.lock().unwrap() = records;
    }
}

/// Holds a scan inside the source until released, to have one running when
/// the view is paused.
#[derive(Clone, Default)]
struct Gate(Arc<(Mutex<bool>, Condvar)>);

impl Gate {
    fn close(&self) {
        *self.0 .0.lock().unwrap() = true;
    }

    fn open(&self) {
        *self.0 .0.lock().unwrap() = false;
        self.0 .1.notify_all();
    }

    fn pass(&self) {
        let (closed, changed) = &*self.0;
        let mut closed = closed.lock().unwrap();
        while *closed {
            closed = changed.wait(closed).unwrap();
        }
    }
}

/// A source over [`Host`] that reports every scan it starts.
struct Live {
    host: Host,
    gate: Gate,
    scans: mpsc::UnboundedSender<()>,
    second: u64,
}

impl ProcessSource for Live {
    fn status_text(&self) -> &str {
        FAKE_STATUS_TEXT
    }

    fn snapshot(&mut self) -> ProcessSnapshot {
        let _ = self.scans.send(());
        // The host as it is when the scan starts, however long it is held.
        let records = self.host.0.lock().unwrap().clone();
        self.gate.pass();
        self.second += 1;
        ProcessSnapshot {
            source: SourceId(SOURCE.into()),
            sampled_at: SnapshotTime(
                SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + self.second),
            ),
            records,
            vanished: 0,
            capped: CappedRecords::none(),
            completeness: Completeness::Complete,
            system: SystemSample::not_provided(),
        }
    }

    fn details(&self) -> Option<Arc<dyn DetailSource>> {
        Some(Arc::new(
            srui_process_explorer::inspector::FixtureDetails::new(SOURCE, Vec::new()),
        ))
    }
}

struct Harness {
    session: Arc<Session>,
    view: SharedView,
    host: Host,
    gate: Gate,
    scans: mpsc::UnboundedReceiver<()>,
    source: Option<Live>,
    idle: Option<DetailReadsIdle>,
    seq: AtomicU64,
}

fn three() -> Vec<ProcessRecord> {
    vec![
        record(101, "alpha", 1024),
        record(102, "beta", 2048),
        record(103, "gamma", 4096),
    ]
}

/// A detail source whose every read waits for one release, and says it
/// started. Its first read answers "permission denied", every later one
/// "unavailable", so a test can tell which read a published line came from.
struct GatedDetails {
    reads: Mutex<u64>,
    entered: Mutex<std::sync::mpsc::Sender<u64>>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
}

impl DetailSource for GatedDetails {
    fn read_details(&self, _target: &ProcessKey) -> DetailRead {
        let read = {
            let mut reads = self.reads.lock().unwrap();
            *reads += 1;
            *reads
        };
        let _ = self.entered.lock().unwrap().send(read);
        let _ = self
            .release
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(30));
        if read == 1 {
            DetailRead::Unreadable(MissingReason::Denied)
        } else {
            DetailRead::Unreadable(MissingReason::Unavailable)
        }
    }
}

/// A [`GatedDetails`], the receiver of its read numbers, and its release.
fn gated_details() -> (
    Arc<dyn DetailSource>,
    std::sync::mpsc::Receiver<u64>,
    std::sync::mpsc::Sender<()>,
) {
    let (entered, reads) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let details = GatedDetails {
        reads: Mutex::new(0),
        entered: Mutex::new(entered),
        release: Mutex::new(released),
    };
    (Arc::new(details), reads, release)
}

fn start(interval: Duration) -> Harness {
    start_with(interval, None)
}

/// [`start`], with the inspector reading `details` instead of the source's.
fn start_with(interval: Duration, details: Option<Arc<dyn DetailSource>>) -> Harness {
    let session = Arc::new(Session::mint());
    let host = Host::default();
    host.set(three());
    let gate = Gate::default();
    let (sender, scans) = mpsc::unbounded_channel();
    let mut source = Live {
        host: host.clone(),
        gate: gate.clone(),
        scans: sender,
        second: 0,
    };
    // As `main` does.
    let (view, _) = ProcessView::start_with_interval(&session, &mut source, interval).unwrap();
    let view: SharedView = Arc::new(Mutex::new(view));
    install_sort_handlers(&session, &view);
    install_filter_handlers(&session, &view);
    install_sampling_handlers(&session, &view);
    let idle = install_inspector(&session, &view, details.or_else(|| source.details()));
    let mut harness = Harness {
        session,
        view,
        host,
        gate,
        scans,
        source: Some(source),
        idle,
        seq: AtomicU64::new(0),
    };
    // The first scan, published by `start`.
    harness.drain();
    harness
}

impl Harness {
    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn activate(&self, node: NodeId) -> EventOutcome {
        let seq = self.next_seq();
        let event: WireEvent = DomainEvent::activate(
            seq,
            format!("px015-click-{seq}"),
            self.session.current_revision(),
            node,
        )
        .with_client_instance_id(CLIENT.to_vec())
        .to_wire();
        self.session.process_event(&event).expect("event admitted")
    }

    fn click(&self, node: NodeId) {
        let outcome = self.activate(node);
        assert!(
            matches!(outcome, EventOutcome::Processed { .. }),
            "not processed: {outcome:?}"
        );
    }

    fn type_into(&self, node: NodeId, text: &str) {
        let seq = self.next_seq();
        let event = DomainEvent::text_edit(
            seq,
            format!("px015-edit-{seq}"),
            0u64,
            node.get(),
            text,
            EditSeq::new(seq).expect("positive edit_seq"),
        )
        .with_client_instance_id(CLIENT.to_vec())
        .to_wire();
        let outcome = self.session.process_event(&event).expect("event admitted");
        assert!(
            matches!(outcome, EventOutcome::Processed { .. }),
            "not processed: {outcome:?}"
        );
    }

    fn select(&self, item: ItemId) {
        let seq = self.next_seq();
        let event: WireEvent = DomainEvent::new(
            None,
            seq,
            format!("px015-select-{seq}"),
            self.session.current_revision(),
            TABLE,
            TypeRef::EVENT_SELECTION_CHANGED,
            vec![(PropertyRef::VALUE, Value::ItemId(item))],
        )
        .with_client_instance_id(CLIENT.to_vec())
        .to_wire();
        let outcome = self.session.process_event(&event).expect("event admitted");
        assert!(matches!(outcome, EventOutcome::Processed { .. }));
    }

    /// Takes one sample by hand, as the loop would, under the view's epoch.
    fn sample(&mut self) {
        let source = self.source.as_mut().expect("the source is not polled");
        let snapshot = source.snapshot();
        let mut view = lock_view(&self.view);
        let epoch = view.sampling().epoch();
        view.apply_sampled(&self.session, epoch, FAKE_STATUS_TEXT, &snapshot)
            .unwrap();
    }

    /// Hands the source to the production loop.
    fn poll(&mut self, shutdown: &CancellationToken) -> tokio::task::JoinHandle<()> {
        let source = self.source.take().expect("polled once");
        let interval = lock_view(&self.view).sampling().interval();
        tokio::spawn(poll(
            Arc::clone(&self.view),
            source,
            Arc::clone(&self.session),
            interval,
            shutdown.child_token(),
        ))
    }

    /// How many scans began since the last call.
    fn drain(&mut self) -> usize {
        let mut count = 0;
        while self.scans.try_recv().is_ok() {
            count += 1;
        }
        count
    }

    async fn next_scan(&mut self) {
        tokio::time::timeout(WAIT, self.scans.recv())
            .await
            .expect("the loop samples")
            .expect("the source is alive");
    }

    fn text(&self, node: NodeId) -> String {
        self.session.with_store(|store| {
            match store.get_node(node).and_then(|n| n.get_property(TEXT)) {
                Some(Value::String(text)) => text.clone(),
                other => panic!("node {node:?} has no text: {other:?}"),
            }
        })
    }

    fn property(&self, node: NodeId, property: PropertyRef) -> Option<Value> {
        self.session.with_store(|store| {
            store
                .get_node(node)
                .unwrap()
                .get_property(property)
                .cloned()
        })
    }

    /// Every published row: item ID and cells, in published order.
    fn rows(&self) -> Vec<(ItemId, Value)> {
        self.session.with_store(|store| {
            store
                .get_model(MODEL)
                .unwrap()
                .items
                .values()
                .map(|item| (item.item_id, item.value.clone()))
                .collect()
        })
    }

    fn names(&self) -> Vec<String> {
        self.rows()
            .into_iter()
            .map(|(_, value)| match value {
                Value::List(cells) => match &cells[ProcessField::Name.index()] {
                    Value::String(name) => name.clone(),
                    other => panic!("a name cell is text: {other:?}"),
                },
                other => panic!("a row is a list: {other:?}"),
            })
            .collect()
    }

    fn identities(&self) -> HashMap<String, ItemId> {
        self.names()
            .into_iter()
            .zip(self.rows().into_iter().map(|(id, _)| id))
            .collect()
    }

    /// Every summary text and property except the freshness line's text,
    /// which is the one line a pause relabels.
    fn figures(&self) -> Vec<(NodeId, PropertyRef, Value)> {
        self.session.with_store(|store| {
            let mut figures = Vec::new();
            for (id, _, _) in LAYOUT {
                let node = store.get_node(id).unwrap();
                for (property, value) in &node.properties {
                    if !(id == FRESHNESS_TEXT && *property == TEXT) {
                        figures.push((id, *property, value.clone()));
                    }
                }
            }
            figures.sort_by_key(|(id, property, _)| (*id, *property));
            figures
        })
    }
}

#[test]
fn pause_freezes_rows_and_figures_while_the_host_changes_and_controls_keep_working() {
    let mut harness = start(Duration::from_secs(1));
    let ids = harness.identities();
    assert_eq!(harness.names(), ["alpha", "beta", "gamma"]);
    assert_eq!(
        harness.text(SAMPLING_STATUS),
        "Live · sampling every 1000 ms"
    );
    assert_eq!(
        harness.property(SAMPLING_PAUSE, LABEL),
        Some(Value::String(PAUSE_LABEL.into()))
    );
    let freshness = harness.text(FRESHNESS_TEXT);
    assert!(
        freshness.starts_with("Last successful sample: "),
        "{freshness}"
    );

    let before = harness.session.current_revision();
    harness.click(SAMPLING_PAUSE);
    let frozen = lock_view(&harness.view)
        .frozen()
        .expect("the view is paused");
    // The rows are frozen as the revision current when the pause was applied
    // held them; the pause's own commit changes no row.
    assert_eq!(frozen.revision, before);
    assert_eq!(harness.session.current_revision(), before + 1);
    let status = harness.text(SAMPLING_STATUS);
    assert!(status.starts_with("Paused at "), "{status}");
    assert!(
        status.contains(&format!(
            "rows as of revision {before} · collection stopped"
        )),
        "{status}"
    );
    assert_eq!(
        harness.property(SAMPLING_PAUSE, LABEL),
        Some(Value::String(RESUME_LABEL.into()))
    );
    assert_eq!(
        harness.property(SAMPLING_REFRESH, ENABLED),
        Some(Value::Bool(false))
    );
    assert_eq!(
        harness.text(FRESHNESS_TEXT),
        format!("{PAUSED_FRESHNESS}{freshness}"),
        "the freshness line says the data is paused, not live"
    );
    let rows = harness.rows();
    let figures = harness.figures();

    // The host changes: a process ends, one appears, one grows. Samples that
    // reach the paused view all the same change nothing at all.
    harness.host.set(vec![
        record(101, "alpha", 999_999),
        record(103, "gamma", 4096),
        record(104, "delta", 8192),
    ]);
    let revision = harness.session.current_revision();
    for _ in 0..3 {
        harness.sample();
    }
    // A refresh called on the view directly is refused just the same.
    {
        let source = harness.source.as_mut().unwrap();
        let snapshot = source.snapshot();
        let refused = lock_view(&harness.view)
            .apply(&harness.session, FAKE_STATUS_TEXT, &snapshot)
            .unwrap();
        assert!(!refused.published());
    }
    assert_eq!(harness.rows(), rows);
    assert_eq!(harness.figures(), figures);
    assert_eq!(harness.session.current_revision(), revision);
    assert_eq!(lock_view(&harness.view).sampling().discarded(), 4);

    // Refresh now is disabled while paused: event validation refuses it.
    assert!(
        matches!(
            harness.activate(SAMPLING_REFRESH),
            EventOutcome::Rejected { .. }
        ),
        "a disabled Refresh now is refused"
    );

    // Controls still work, on the frozen rows: the filter commits its own
    // state and re-projects them...
    harness.type_into(FILTER_INPUT, "beta");
    assert_eq!(
        harness.names(),
        ["beta"],
        "beta ended, but the view is frozen"
    );
    assert!(harness
        .text(FILTER_STATUS)
        .contains("\"beta\" · 1 of 3 rows matched"));
    // ...and clearing it restores exactly the frozen rows.
    harness.type_into(FILTER_INPUT, "");
    assert_eq!(harness.rows(), rows);
    // A sort reorders the frozen rows, keeping every item ID.
    harness.click(SORT_NAME);
    harness.click(SORT_NAME);
    assert_eq!(harness.names(), ["gamma", "beta", "alpha"]);
    assert_eq!(harness.identities(), ids);
    // The freshness line still says paused after those re-projections.
    assert!(harness.text(FRESHNESS_TEXT).starts_with(PAUSED_FRESHNESS));
    assert!(
        harness.session.current_revision() > revision,
        "controls committed"
    );

    // Resume: one current sample, through the ordinary diff.
    harness.click(SAMPLING_PAUSE);
    assert!(lock_view(&harness.view).frozen().is_none());
    assert_eq!(
        harness.text(SAMPLING_STATUS),
        "Resuming · sampling now, then every 1000 ms"
    );
    assert!(harness.text(FRESHNESS_TEXT).starts_with(RESUMING_FRESHNESS));
    assert_eq!(
        harness.property(SAMPLING_REFRESH, ENABLED),
        Some(Value::Bool(true))
    );
    harness.sample();
    // The current host, in the active sort (name, descending).
    assert_eq!(harness.names(), ["gamma", "delta", "alpha"]);
    let now = harness.identities();
    assert_eq!(
        now["alpha"], ids["alpha"],
        "a surviving row keeps its item ID"
    );
    assert_eq!(now["gamma"], ids["gamma"]);
    assert!(!ids.values().any(|id| *id == now["delta"]));
    assert_eq!(
        harness.text(SAMPLING_STATUS),
        "Live · sampling every 1000 ms"
    );
    let live = harness.text(FRESHNESS_TEXT);
    assert!(live.starts_with("Last successful sample: "), "{live}");
    assert_eq!(
        harness.property(SAMPLING_PAUSE, LABEL),
        Some(Value::String(PAUSE_LABEL.into()))
    );
}

#[test]
fn a_paused_inspector_reads_nothing_live_and_resume_reads_again() {
    let mut harness = start(Duration::from_secs(1));
    let ids = harness.identities();
    harness.click(SAMPLING_PAUSE);
    harness.select(ids["beta"]);
    let idle = harness.idle.clone().expect("a detail worker");
    assert!(idle.wait_idle(WAIT));
    let details = harness.text(INSPECTOR_DETAILS);
    assert_eq!(
        details,
        "State, parent and start time: not read while the view is paused; Resume view reads them"
    );
    harness.click(SAMPLING_PAUSE);
    assert!(idle.wait_idle(WAIT));
    // The fixture has no details for this key, so the live read says so.
    assert_eq!(
        harness.text(INSPECTOR_DETAILS),
        "State, parent and start time: unavailable"
    );
    harness.sample();
    assert!(idle.wait_idle(WAIT));
}

#[test]
fn invalid_intervals_are_refused_with_a_reason_and_the_interval_kept() {
    let harness = start(Duration::from_millis(1000));
    let not_a_number = "not a whole number of milliseconds; enter 50 to 300000 ms";
    let outside = "outside 50 to 300000 ms";
    for (text, reason) in [
        ("abc", not_a_number),
        ("-5", not_a_number),
        ("+5", not_a_number),
        ("1.5", not_a_number),
        ("2s", not_a_number),
        ("0", outside),
        ("49", outside),
        ("300001", outside),
        ("99999999999999999999999", outside),
        ("", "no interval entered; enter 50 to 300000 ms"),
    ] {
        harness.type_into(INTERVAL_INPUT, text);
        harness.click(INTERVAL_APPLY);
        assert_eq!(
            harness.text(SAMPLING_STATUS),
            format!(
                "Live · sampling every 1000 ms · interval {text:?} refused: {reason}; keeping 1000 ms"
            ),
            "{text:?}"
        );
        assert_eq!(
            lock_view(&harness.view).sampling().interval(),
            Duration::from_millis(1000)
        );
    }
    // Editing the field clears the refusal of the text it replaced.
    harness.type_into(INTERVAL_INPUT, "12");
    assert_eq!(
        harness.text(SAMPLING_STATUS),
        "Live · sampling every 1000 ms"
    );
    // A text longer than the server keeps is cut, said so, and refused.
    harness.type_into(INTERVAL_INPUT, &"5".repeat(100));
    assert_eq!(
        harness.property(INTERVAL_INPUT, VALUE),
        Some(Value::String("5".repeat(32)))
    );
    harness.click(INTERVAL_APPLY);
    assert!(harness
        .text(SAMPLING_STATUS)
        .contains("refused: longer than 32 bytes"));
    // An accepted interval replaces the refusal.
    harness.type_into(INTERVAL_INPUT, "250");
    harness.click(INTERVAL_APPLY);
    assert_eq!(
        harness.text(SAMPLING_STATUS),
        "Live · sampling every 250 ms"
    );
    assert_eq!(
        lock_view(&harness.view).sampling().interval(),
        Duration::from_millis(250)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_loop_takes_no_sample_while_paused_and_exactly_one_on_resume() {
    let interval = Duration::from_millis(300);
    let mut harness = start(interval);
    let shutdown = CancellationToken::new();
    let polling = harness.poll(&shutdown);
    harness.next_scan().await;
    harness.click(SAMPLING_PAUSE);
    // A scan already running when the pause landed is discarded; let it end.
    tokio::time::sleep(Duration::from_millis(100)).await;
    harness.drain();
    let rows = harness.rows();
    let figures = harness.figures();
    harness.host.set(vec![record(104, "delta", 8192)]);
    // Five intervals paused: the loop polls no ticker and takes no sample.
    tokio::time::sleep(interval * 5).await;
    assert_eq!(harness.drain(), 0, "collection stops while paused");
    assert_eq!(harness.rows(), rows);
    assert_eq!(harness.figures(), figures);
    // Control traffic still flows while paused.
    harness.type_into(FILTER_INPUT, "alpha");
    assert_eq!(harness.names(), ["alpha"]);
    harness.type_into(FILTER_INPUT, "");

    harness.click(SAMPLING_PAUSE);
    harness.next_scan().await;
    // Half an interval later: that one sample, and no replay of the five
    // missed ticks.
    tokio::time::sleep(interval / 2).await;
    assert_eq!(harness.drain(), 0, "one sample on resume, not a burst");
    let converged = tokio::time::timeout(WAIT, async {
        while harness.names() != ["delta"] {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(converged.is_ok(), "resume converges on the current host");
    assert_eq!(
        harness.text(SAMPLING_STATUS),
        "Live · sampling every 300 ms"
    );
    shutdown.cancel();
    tokio::time::timeout(WAIT, polling)
        .await
        .expect("the loop stops")
        .expect("the loop does not panic");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scan_running_when_the_view_is_paused_never_changes_the_frozen_rows() {
    let mut harness = start(Duration::from_millis(50));
    let shutdown = CancellationToken::new();
    harness.gate.close();
    let polling = harness.poll(&shutdown);
    // The loop's scan is held inside the source.
    harness.next_scan().await;
    harness.click(SAMPLING_PAUSE);
    let rows = harness.rows();
    harness.host.set(vec![record(104, "delta", 8192)]);
    let revision = harness.session.current_revision();
    harness.gate.open();
    let discarded = tokio::time::timeout(WAIT, async {
        while lock_view(&harness.view).sampling().discarded() == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert!(
        discarded.is_ok(),
        "the running scan returned and was discarded"
    );
    assert_eq!(harness.rows(), rows);
    assert_eq!(harness.session.current_revision(), revision);

    // Paused and resumed while a scan of the old host is held: that scan is
    // from an earlier epoch and never stands in for the one resume asks for.
    harness.gate.close();
    harness.click(SAMPLING_PAUSE);
    harness.next_scan().await;
    harness.host.set(vec![record(105, "epsilon", 8192)]);
    harness.click(SAMPLING_PAUSE);
    harness.click(SAMPLING_PAUSE);
    harness.gate.open();
    let converged = tokio::time::timeout(WAIT, async {
        while harness.names() != ["epsilon"] {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert!(converged.is_ok(), "the resume's own sample is published");
    assert_eq!(
        lock_view(&harness.view).sampling().discarded(),
        2,
        "the scan from before the pause and resume was discarded"
    );
    shutdown.cancel();
    tokio::time::timeout(WAIT, polling).await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refresh_now_presses_coalesce_into_one_bounded_sample() {
    // The slowest interval, so no periodic sample falls inside the test.
    let mut harness = start(Duration::from_secs(300));
    let shutdown = CancellationToken::new();
    let polling = harness.poll(&shutdown);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(harness.drain(), 0);
    harness.host.set(vec![record(104, "delta", 8192)]);
    // Five presses the loop has not taken yet are one request.
    {
        let mut view = lock_view(&harness.view);
        for _ in 0..5 {
            assert!(view.refresh_now());
        }
    }
    harness.next_scan().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(harness.drain(), 0, "five presses, one sample");
    assert_eq!(harness.names(), ["delta"]);
    // The same through the button.
    harness.click(SAMPLING_REFRESH);
    harness.next_scan().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(harness.drain(), 0);
    shutdown.cancel();
    tokio::time::timeout(WAIT, polling).await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_interval_change_restarts_the_one_ticker_without_a_burst() {
    let mut harness = start(Duration::from_millis(50));
    let shutdown = CancellationToken::new();
    let polling = harness.poll(&shutdown);
    // Sampling at 50 ms.
    for _ in 0..3 {
        harness.next_scan().await;
    }
    harness.type_into(INTERVAL_INPUT, "400");
    harness.click(INTERVAL_APPLY);
    assert_eq!(
        harness.text(SAMPLING_STATUS),
        "Live · sampling every 400 ms"
    );
    // At most a scan already running when the change landed.
    tokio::time::sleep(Duration::from_millis(30)).await;
    harness.drain();
    // No burst: nothing before the first new interval has passed.
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(
        harness.drain(),
        0,
        "the new interval starts from the change"
    );
    // Then one sample per 400 ms; a second 50 ms ticker would show ~24.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let taken = harness.drain();
    assert!(
        (2..=4).contains(&taken),
        "{taken} samples in ~1.45 s at 400 ms"
    );
    // Paused, then shut down: the loop ends with its ticker.
    harness.click(SAMPLING_PAUSE);
    shutdown.cancel();
    tokio::time::timeout(WAIT, polling)
        .await
        .expect("the loop stops while paused")
        .expect("the loop does not panic");
    // Nothing the loop started outlives it: the view is held by the harness
    // and the handlers only.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(harness.drain(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_loop_ends_on_shutdown_in_every_sampling_state_and_releases_the_view() {
    for state in ["live", "paused", "resuming", "retimed"] {
        let mut harness = start(Duration::from_millis(50));
        let shutdown = CancellationToken::new();
        let polling = harness.poll(&shutdown);
        harness.next_scan().await;
        match state {
            "paused" => harness.click(SAMPLING_PAUSE),
            "resuming" => {
                harness.click(SAMPLING_PAUSE);
                harness.click(SAMPLING_PAUSE);
            }
            "retimed" => {
                harness.type_into(INTERVAL_INPUT, "300000");
                harness.click(INTERVAL_APPLY);
            }
            _ => {}
        }
        shutdown.cancel();
        tokio::time::timeout(WAIT, polling)
            .await
            .unwrap_or_else(|_| panic!("the loop stops when {state}"))
            .expect("the loop does not panic");
        let weak = Arc::downgrade(&harness.view);
        let Harness { session, view, .. } = harness;
        // The handlers hold the view; the session owns them. Once both go,
        // the view and its inspector worker go too: no task kept it.
        drop(view);
        drop(session);
        assert!(
            weak.upgrade().is_none(),
            "nothing outlives the view ({state})"
        );
    }
}

const DETAILS_LOADING: &str = "State, parent and start time: loading";
const DETAILS_DEFERRED: &str =
    "State, parent and start time: not read while the view is paused; Resume view reads them";
const DETAILS_FIRST_READ: &str = "State, parent and start time: permission denied";
const DETAILS_LATER_READ: &str = "State, parent and start time: unavailable";

/// PR #95 review, F1: a detail read asked for before Pause view and returning
/// after Resume view is from an earlier generation, and is never shown; the
/// read the resume asked for is.
#[test]
fn a_detail_read_from_before_a_pause_and_resume_is_never_shown() {
    let (details, entered, release) = gated_details();
    let harness = start_with(Duration::from_secs(1), Some(details));
    let ids = harness.identities();
    harness.select(ids["beta"]);
    assert_eq!(
        entered.recv_timeout(WAIT),
        Ok(1),
        "the first read is running"
    );
    let before = lock_view(&harness.view).inspector().stale_results();
    harness.click(SAMPLING_PAUSE);
    harness.click(SAMPLING_PAUSE);
    assert_eq!(harness.text(INSPECTOR_DETAILS), DETAILS_LOADING);
    // The first read returns; the worker delivers it before it starts the
    // resume's read, so once that one has started the first is settled.
    release.send(()).unwrap();
    assert_eq!(entered.recv_timeout(WAIT), Ok(2), "the resume asked again");
    assert_eq!(
        harness.text(INSPECTOR_DETAILS),
        DETAILS_LOADING,
        "the pre-pause read was dropped, not published into the resumed view"
    );
    assert_eq!(
        lock_view(&harness.view).inspector().stale_results(),
        before + 1
    );
    release.send(()).unwrap();
    let idle = harness.idle.clone().expect("a detail worker");
    assert!(idle.wait_idle(WAIT));
    assert_eq!(harness.text(INSPECTOR_DETAILS), DETAILS_LATER_READ);
    assert_ne!(harness.text(INSPECTOR_DETAILS), DETAILS_FIRST_READ);
}

/// A detail read in flight across Pause view alone is dropped when it
/// returns: the paused inspector keeps saying nothing was read.
#[test]
fn a_detail_read_in_flight_across_a_pause_is_dropped() {
    let (details, entered, release) = gated_details();
    let harness = start_with(Duration::from_secs(1), Some(details));
    let ids = harness.identities();
    harness.select(ids["beta"]);
    assert_eq!(entered.recv_timeout(WAIT), Ok(1));
    let before = lock_view(&harness.view).inspector().stale_results();
    harness.click(SAMPLING_PAUSE);
    assert_eq!(harness.text(INSPECTOR_DETAILS), DETAILS_DEFERRED);
    release.send(()).unwrap();
    let idle = harness.idle.clone().expect("a detail worker");
    assert!(idle.wait_idle(WAIT));
    assert_eq!(harness.text(INSPECTOR_DETAILS), DETAILS_DEFERRED);
    assert_eq!(
        lock_view(&harness.view).inspector().stale_results(),
        before + 1
    );
    // Resume reads again, and that read is shown.
    harness.click(SAMPLING_PAUSE);
    assert_eq!(entered.recv_timeout(WAIT), Ok(2));
    release.send(()).unwrap();
    assert!(idle.wait_idle(WAIT));
    assert_eq!(harness.text(INSPECTOR_DETAILS), DETAILS_LATER_READ);
}

/// PR #95 review, F2: an interval change during a scan that outlasts the old
/// interval leaves the old ticker's tick and the wake both ready. Whichever the
/// loop takes, no scan starts before one new interval. Each round makes both
/// ready; code that honoured the retired tick would start an extra scan in
/// about half of the rounds, so eight clean rounds leave it a 1-in-256 chance.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tick_of_the_retired_ticker_never_starts_an_extra_scan() {
    let mut harness = start(Duration::from_millis(50));
    let shutdown = CancellationToken::new();
    let polling = harness.poll(&shutdown);
    for round in 0..8 {
        harness.next_scan().await;
        // Hold the next scan inside the source.
        harness.gate.close();
        tokio::time::sleep(Duration::from_millis(120)).await;
        harness.drain();
        harness.type_into(INTERVAL_INPUT, "300000");
        harness.click(INTERVAL_APPLY);
        // Well past the old interval: its tick is due, and the wake is stored.
        tokio::time::sleep(Duration::from_millis(150)).await;
        harness.gate.open();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            harness.drain(),
            0,
            "round {round}: a retired tick started a scan after the interval became 300000 ms"
        );
        // Back to 50 ms for the next round.
        harness.type_into(INTERVAL_INPUT, "50");
        harness.click(INTERVAL_APPLY);
    }
    shutdown.cancel();
    tokio::time::timeout(WAIT, polling).await.unwrap().unwrap();
}

/// Refresh now restarts the ticker: the next periodic sample is one interval
/// after the manual one, not the old schedule's next tick.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refresh_now_restarts_the_ticker() {
    let interval = Duration::from_millis(600);
    let mut harness = start(interval);
    let shutdown = CancellationToken::new();
    let polling = harness.poll(&shutdown);
    // A periodic scan, then a manual one half an interval later.
    harness.next_scan().await;
    tokio::time::sleep(interval / 2).await;
    assert_eq!(harness.drain(), 0);
    harness.click(SAMPLING_REFRESH);
    harness.next_scan().await;
    let manual = std::time::Instant::now();
    // The old schedule would tick half an interval after the manual scan.
    tokio::time::sleep(interval * 3 / 4).await;
    assert_eq!(harness.drain(), 0, "the old schedule's tick was not kept");
    harness.next_scan().await;
    assert!(
        manual.elapsed() >= interval * 3 / 4,
        "the next periodic scan follows one interval after the manual one"
    );
    shutdown.cancel();
    tokio::time::timeout(WAIT, polling).await.unwrap().unwrap();
}
