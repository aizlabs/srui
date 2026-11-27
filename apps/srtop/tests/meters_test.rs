//! PX-070 acceptance: meter layout and non-graph display modes (D1 §§7.4,
//! 10–12; D2 T16, T28).
//!
//! * Per-sample updates are scalar: a refresh sets or clears properties on
//!   the meter nodes that exist and never creates, deletes or moves a node.
//! * A layout change is structural exactly once: the click's one revision
//!   holds the deletions, moves and creations, and no figure a client
//!   already holds; an unchanged layout commits nothing, and a disabled
//!   control is refused by event validation.
//! * A hidden meter, or a meter shown as text, has no node to update; a
//!   meter shown again arrives with the current figures.
//! * Unavailable, denied, warming-up and stale figures never become a zero
//!   in any presentation: no bar is drawn and the line says why.
//! * A host with thousands of logical CPUs publishes the same bounded
//!   summary, and the row ceiling does not depend on the CPU count or the
//!   layout.
//! * The layout is a saved semantic preference: a saved view restores it
//!   after a restart, and an older settings file migrates.
//! * Nothing the server sends names a position, a size or a colour.
//!
//! Clicks travel the runtime's event path (`Session::process_event`) and
//! every revision is applied by a client replica.

use srui_process_explorer::meters::{Meter, MeterLayout, Presentation, METER_BUTTONS};
use srui_process_explorer::refresh::{lock_view, ProcessView, SharedView};
use srui_process_explorer::saved_views::SavedViews;
use srui_process_explorer::settings_file::SettingsStore;
use srui_process_explorer::source::*;
use srui_process_explorer::summary::{
    self, CPU_BAR, CPU_ROW, CPU_TEXT, FRESHNESS_TEXT, MEMORY_BAR, MEMORY_TEXT, METER_GROUPS,
    METER_GROUP_2, METER_HIDDEN, PROCESSES_TEXT, SUMMARY, SWAP_BAR, SWAP_TEXT,
};
use srui_process_explorer::{
    install_meter_handlers, start_from_source, COLUMN, METERS_HEADER, METERS_RESTORE,
    METERS_STATUS, METER_EARLIER_CPU, METER_LATER_PROCESSES, METER_MODE_CPU, METER_TOGGLE_SWAP,
    SHELL_NODE_COUNT,
};
use srui_protocol::DEFAULT_MAX_FRAME_SIZE;
use srui_sdk::{EnumToken, NodeId, Operation, PropertyRef, Value, Visibility, TEXT, VALUE};
use srui_semantic_tree::{Event as SemanticEvent, SemanticStore};
use srui_sessiond::{EventOutcome, Session};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

const SOURCE: &str = "px070-fixture-v1";

/// A host whose figures move on every sample, so every refresh has a meter
/// to update.
struct Moving {
    step: u64,
    cpus: Option<u32>,
    records: usize,
    /// Every name at the widest a row can publish, so a few tens of
    /// thousands of rows overflow one snapshot frame.
    wide: bool,
}

impl Moving {
    fn new(records: usize, cpus: Option<u32>) -> Self {
        Self {
            step: 0,
            cpus,
            records,
            wide: false,
        }
    }
}

fn key(pid: u32) -> ProcessKey {
    ProcessKey {
        source: SourceId(SOURCE.into()),
        host: Observed::Known(HostId("px070-host".into())),
        boot: Observed::Known(BootId("px070-boot".into())),
        pid_namespace: Observed::Known(PidNamespaceId(4_026_531_836)),
        pid: Observed::Known(pid),
        creation: CreationToken::Opaque(format!("px070-{pid}")),
        thread: None,
    }
}

fn record(pid: u32, wide: bool) -> ProcessRecord {
    let name = if wide {
        "\u{20000}".repeat(MAX_DISPLAY_NAME_CHARS)
    } else {
        format!("worker-{pid}")
    };
    ProcessRecord {
        kind: KIND_NOT_COLLECTED,
        lineage: Lineage::NOT_COLLECTED,
        owner: OWNER_NOT_COLLECTED,
        key: key(pid),
        display_name: name.as_str().into(),
        resident: Observed::Known(4096),
        cpu: CpuUsage::Missing(MissingReason::Unavailable),
    }
}

fn system(step: u64, cpus: Option<u32>) -> SystemSample {
    SystemSample {
        cpu: SystemCpu::Measured(SystemCpuInterval {
            busy: 100 + step,
            total: 400,
            cpus,
        }),
        memory: Ok(MemoryFigures {
            total: 8 << 30,
            available: (6 << 30) - (step << 20),
        }),
        swap: Ok(SwapFigures {
            total: 2 << 30,
            free: (1 << 30) + (step << 20),
        }),
        uptime: Ok(86_400 + step * 60),
        load: Ok(LoadAverages {
            one: 100 + step,
            five: 90,
            fifteen: 80,
        }),
    }
}

impl ProcessSource for Moving {
    fn status_text(&self) -> &str {
        FAKE_STATUS_TEXT
    }

    fn snapshot(&mut self) -> ProcessSnapshot {
        self.step += 1;
        ProcessSnapshot {
            source: SourceId(SOURCE.into()),
            sampled_at: SnapshotTime(
                SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + self.step),
            ),
            records: (1..=self.records as u32)
                .map(|pid| record(pid, self.wide))
                .collect(),
            vanished: 0,
            threads: ThreadScan::NotProvided,
            capped: CappedRecords::none(),
            completeness: Completeness::Complete,
            system: system(self.step, self.cpus),
        }
    }
}

/// A started view, its handlers installed, and a client replica that has
/// applied every committed revision.
struct Fixture {
    session: Session,
    view: SharedView,
    source: Moving,
    replica: SemanticStore,
    seq: u64,
}

impl Fixture {
    fn start(records: usize, cpus: Option<u32>) -> Self {
        let session = Session::mint();
        let mut source = Moving::new(records, cpus);
        let (view, _) = start_from_source(&session, &mut source).expect("the fixture publishes");
        let view = Arc::new(Mutex::new(view));
        install_meter_handlers(&session, &view);
        let mut fixture = Self {
            session,
            view,
            source,
            replica: SemanticStore::new(),
            seq: 0,
        };
        fixture.sync();
        fixture
    }

    fn revision(&self) -> u64 {
        self.session.current_revision()
    }

    /// Applies every revision the replica lacks, as a client would.
    fn sync(&mut self) {
        for transaction in self
            .session
            .collect_replayed_transactions(self.replica.revision().get())
            .expect("the journal holds this run")
        {
            let revision = transaction.new_revision;
            self.replica
                .apply_wire_transaction(transaction)
                .unwrap_or_else(|error| panic!("a client refuses revision {revision}: {error}"));
        }
        assert_eq!(self.replica.revision().get(), self.revision());
    }

    fn refresh(&mut self) {
        lock_view(&self.view)
            .refresh(&self.session, &mut self.source)
            .expect("the refresh publishes");
        self.sync();
    }

    /// A client's click on `node`, and what the session made of it.
    fn click(&mut self, node: NodeId) -> EventOutcome {
        self.seq += 1;
        let event = srui_protocol::Event::from(
            &SemanticEvent::activate(
                self.seq,
                format!("meter-click-{}", self.seq),
                self.revision(),
                node,
            )
            .with_client_instance_id("px070-meters-test"),
        );
        let outcome = self
            .session
            .process_event(&event)
            .expect("the session admits the event");
        self.sync();
        outcome
    }

    /// A click that must be applied and published in exactly one revision.
    fn applied(&mut self, node: NodeId) -> Vec<Operation> {
        let before = self.revision();
        let outcome = self.click(node);
        assert!(
            matches!(outcome, EventOutcome::Processed { revision_after_effect, .. }
                if revision_after_effect == before + 1),
            "the click on {node:?} was not one revision: {outcome:?}"
        );
        operations_since(&self.session, before)
    }

    fn layout(&self) -> MeterLayout {
        lock_view(&self.view).meter_layout().clone()
    }

    /// The server's and the replica's summary structure are the layout's:
    /// every summary node exists, each in the container the layout puts it.
    fn assert_structure(&self) {
        let layout = self.layout();
        let check = |store: &SemanticStore| {
            assert_eq!(store.node_count(), SHELL_NODE_COUNT);
            for (id, node_type, parent) in layout.nodes() {
                let node = store
                    .get_node(id)
                    .unwrap_or_else(|| panic!("{id:?} missing"));
                assert_eq!(node.node_type, node_type, "{id:?}");
                assert_eq!(node.parent_id, Some(parent), "{id:?}");
            }
            for (container, children) in layout.arrangement() {
                assert_eq!(
                    store.children_of(container),
                    Some(children.as_slice()),
                    "{container:?}"
                );
            }
            assert_eq!(
                store.children_of(SUMMARY),
                Some([METER_GROUPS, FRESHNESS_TEXT, METER_HIDDEN].as_slice())
            );
        };
        self.session.with_store(check);
        check(&self.replica);
    }
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

fn is_structural(operation: &Operation) -> bool {
    matches!(
        operation,
        Operation::CreateNode { .. }
            | Operation::DeleteNode { .. }
            | Operation::MoveNode { .. }
            | Operation::ReorderChildren { .. }
    )
}

/// The nodes a run of operations sets a property on.
fn set_on(operations: &[Operation]) -> Vec<(NodeId, PropertyRef)> {
    operations
        .iter()
        .filter_map(|operation| match operation {
            Operation::SetProperty { id, property, .. }
            | Operation::ClearProperty { id, property } => Some((*id, *property)),
            _ => None,
        })
        .collect()
}

fn text(session: &Session, node: NodeId) -> Option<String> {
    session.with_store(|store| match store.get_node(node)?.get_property(TEXT) {
        Some(Value::String(text)) => Some(text.clone()),
        _ => None,
    })
}

/// Per-sample updates are scalar on every layout, and each layout change is
/// structural exactly once and carries no figure.
#[test]
fn per_sample_updates_are_scalar_and_each_layout_change_is_structural_exactly_once() {
    let mut fixture = Fixture::start(3, Some(4));
    fixture.assert_structure();
    let assert_samples_are_scalar = |fixture: &mut Fixture, label: &str| {
        let before = fixture.revision();
        for _ in 0..3 {
            fixture.refresh();
        }
        let operations = operations_since(&fixture.session, before);
        assert!(
            !operations.iter().any(is_structural),
            "{label}: a sample changed structure: {operations:?}"
        );
        assert!(
            set_on(&operations).contains(&(FRESHNESS_TEXT, TEXT)),
            "{label}: the samples were published"
        );
        fixture.assert_structure();
        operations
    };
    let operations = assert_samples_are_scalar(&mut fixture, "default");
    assert!(set_on(&operations).contains(&(CPU_TEXT, TEXT)));
    assert!(set_on(&operations).contains(&(CPU_BAR, VALUE)));

    // Processes into the second group: one MOVE_NODE, the second group shown,
    // the controls relabelled; no figure.
    let change = fixture.applied(METER_LATER_PROCESSES);
    let structural: Vec<&Operation> = change.iter().filter(|op| is_structural(op)).collect();
    assert_eq!(
        structural,
        vec![&Operation::move_node(
            PROCESSES_TEXT,
            Some(METER_GROUP_2),
            Some(0)
        )]
    );
    for (node, property) in set_on(&change) {
        assert!(
            node == METER_GROUP_2
                || node == METERS_STATUS
                || METER_BUTTONS.iter().any(|(button, _)| *button == node),
            "a layout change set {property:?} on {node:?}"
        );
    }
    fixture.assert_structure();
    assert_samples_are_scalar(&mut fixture, "processes in group 2");

    // CPU as text: its bar moves into the collapsed container, and no
    // sample touches it there.
    let change = fixture.applied(METER_MODE_CPU);
    assert_eq!(
        change
            .iter()
            .filter(|op| is_structural(op))
            .collect::<Vec<_>>(),
        vec![&Operation::move_node(CPU_BAR, Some(METER_HIDDEN), Some(0))]
    );
    assert!(set_on(&change).iter().all(|(node, _)| *node != CPU_BAR));
    let operations = assert_samples_are_scalar(&mut fixture, "CPU as text");
    assert!(set_on(&operations).iter().all(|(node, _)| *node != CPU_BAR));
    assert!(
        set_on(&operations).contains(&(CPU_TEXT, TEXT)),
        "the line is the fallback"
    );

    // And back to a bar: before its line again, brought up to the current
    // value in the same revision.
    let change = fixture.applied(METER_MODE_CPU);
    assert_eq!(
        change
            .iter()
            .filter(|op| is_structural(op))
            .collect::<Vec<_>>(),
        vec![&Operation::move_node(CPU_BAR, Some(CPU_ROW), Some(0))]
    );
    assert!(set_on(&change).contains(&(CPU_BAR, VALUE)), "{change:?}");
    fixture.assert_structure();

    // Hide swap: it moves into the collapsed container, and no sample
    // touches it there.
    let change = fixture.applied(METER_TOGGLE_SWAP);
    assert_eq!(
        change
            .iter()
            .filter(|op| is_structural(op))
            .collect::<Vec<_>>(),
        vec![&Operation::move_node(
            summary::SWAP_ROW,
            Some(METER_HIDDEN),
            Some(0)
        )]
    );
    let operations = assert_samples_are_scalar(&mut fixture, "swap hidden");
    assert!(set_on(&operations)
        .iter()
        .all(|(node, _)| ![summary::SWAP_ROW, SWAP_BAR, SWAP_TEXT].contains(node)));
    // Hide every meter: the row of groups collapses; nothing moves.
    let change = fixture.applied(METERS_HEADER);
    assert!(!change.iter().any(is_structural), "{change:?}");
    assert!(set_on(&change).contains(&(METER_GROUPS, PropertyRef::VISIBILITY)));
    fixture.assert_structure();
    let operations = assert_samples_are_scalar(&mut fixture, "meters hidden");
    assert!(
        set_on(&operations)
            .iter()
            .all(|(node, _)| *node == FRESHNESS_TEXT
                || !summary::LAYOUT.iter().any(|(id, ..)| id == node)),
        "a hidden meter was updated: {operations:?}"
    );

    // Shown again: brought up to the current figures in the same revision,
    // so the next sample has nothing to correct.
    fixture.applied(METERS_HEADER);
    fixture.assert_structure();
    let memory = text(&fixture.session, MEMORY_TEXT);
    assert!(memory.is_some());
    let before = fixture.revision();
    lock_view(&fixture.view)
        .refresh(
            &fixture.session,
            &mut Moving {
                step: fixture.source.step - 1,
                ..Moving::new(3, Some(4))
            },
        )
        .unwrap();
    fixture.sync();
    assert!(
        set_on(&operations_since(&fixture.session, before))
            .iter()
            .all(|(node, _)| *node != MEMORY_TEXT),
        "a meter shown again already held the current figures"
    );

    // Restore the default: structure again, exactly once.
    let change = fixture.applied(METERS_RESTORE);
    assert!(change
        .iter()
        .filter(|op| is_structural(op))
        .all(|op| matches!(op, Operation::MoveNode { .. })));
    assert!(change.iter().any(is_structural));
    assert!(fixture.layout().is_default());
    fixture.assert_structure();
    assert_samples_are_scalar(&mut fixture, "restored");
}

/// The same layout commits nothing, and a control that could change
/// nothing is disabled and refused by event validation.
#[test]
fn an_unchanged_layout_commits_nothing_and_a_disabled_control_is_refused() {
    let mut fixture = Fixture::start(3, Some(4));
    let before = fixture.revision();
    let layout = fixture.layout();
    assert!(!lock_view(&fixture.view)
        .set_meter_layout(&fixture.session, layout)
        .unwrap());
    assert_eq!(fixture.revision(), before);
    // CPU is first: it cannot move earlier, and restoring the default
    // layout changes nothing.
    for node in [METER_EARLIER_CPU, METERS_RESTORE] {
        let outcome = fixture.click(node);
        assert!(
            matches!(outcome, EventOutcome::Rejected { .. }),
            "{node:?}: {outcome:?}"
        );
        assert_eq!(fixture.revision(), before);
    }
}

/// No value that is unavailable, denied, warming up or stale is drawn or
/// worded as a zero, in either presentation.
#[test]
fn unavailable_and_stale_figures_never_become_zero_in_any_presentation() {
    let states = [
        SystemSample::not_provided(),
        SystemSample::missing(FigureGap::Unread(MissingReason::Denied)),
        SystemSample::missing(FigureGap::Unread(MissingReason::Unavailable)),
        SystemSample::missing(FigureGap::Unusable),
        SystemSample {
            cpu: SystemCpu::WarmingUp,
            ..SystemSample::not_provided()
        },
        SystemSample {
            cpu: SystemCpu::Interrupted,
            ..SystemSample::not_provided()
        },
    ];
    for system in states {
        for text_only in [false, true] {
            let session = Session::mint();
            let mut source = Fixed(system);
            let (mut view, _) = start_from_source(&session, &mut source).unwrap();
            if text_only {
                let mut layout = view.meter_layout().clone();
                for meter in [Meter::Cpu, Meter::Memory, Meter::Swap] {
                    layout = layout.switching(meter);
                }
                assert!(view.set_meter_layout(&session, layout).unwrap());
            }
            view.refresh(&session, &mut source).unwrap();
            session.with_store(|store| {
                for bar in [CPU_BAR, MEMORY_BAR, SWAP_BAR] {
                    let node = store.get_node(bar).unwrap();
                    assert!(!node.has_property(VALUE), "{system:?}: {bar:?} has a value");
                    if text_only {
                        assert_eq!(node.parent_id, Some(METER_HIDDEN), "{bar:?}");
                    } else {
                        assert_eq!(
                            node.get_property(PropertyRef::VISIBILITY),
                            Some(&Value::from(EnumToken::from(Visibility::Hidden)))
                        );
                    }
                }
            });
            for line in [CPU_TEXT, MEMORY_TEXT, SWAP_TEXT] {
                let text = text(&session, line).unwrap();
                let (_, value) = text
                    .split_once("): ")
                    .or_else(|| text.split_once(": "))
                    .unwrap();
                assert!(
                    !value.chars().any(|c| c.is_ascii_digit()),
                    "{system:?} text_only={text_only}: {text}"
                );
            }
        }
    }

    // Stale: a scan that fails after a sample keeps the last figures in text
    // mode, never zeros them, and the freshness line warns.
    let session = Session::mint();
    let mut source = Moving::new(3, Some(4));
    let (mut view, _) = start_from_source(&session, &mut source).unwrap();
    let mut layout = view.meter_layout().clone();
    layout = layout.switching(Meter::Cpu);
    view.set_meter_layout(&session, layout).unwrap();
    let cpu = text(&session, CPU_TEXT).unwrap();
    let mut failing = Failing;
    view.refresh(&session, &mut failing).unwrap();
    assert_eq!(
        text(&session, CPU_TEXT).unwrap(),
        cpu,
        "the last figure is kept"
    );
    assert!(text(&session, FRESHNESS_TEXT)
        .unwrap()
        .starts_with("Collector error"));
}

/// A source whose every snapshot carries `system`.
struct Fixed(SystemSample);

impl ProcessSource for Fixed {
    fn status_text(&self) -> &str {
        FAKE_STATUS_TEXT
    }

    fn snapshot(&mut self) -> ProcessSnapshot {
        let mut snapshot = Moving::new(3, None).snapshot();
        snapshot.system = self.0;
        snapshot
    }
}

/// A source whose process list cannot be listed.
struct Failing;

impl ProcessSource for Failing {
    fn status_text(&self) -> &str {
        FAKE_STATUS_TEXT
    }

    fn snapshot(&mut self) -> ProcessSnapshot {
        let mut snapshot = Moving::new(0, Some(4)).snapshot();
        snapshot.completeness = Completeness::from_scan(
            SkippedRecords::unenumerable(),
            vec![EnumerationIssue {
                scope: IssueScope::Root,
                reason: MissingReason::Denied,
                detail: "/proc: Permission denied (os error 13)".into(),
            }],
        );
        snapshot
    }
}

/// A host with up to 65,536 logical CPUs publishes the same summary nodes,
/// a bounded CPU line, and the same row ceiling, in every layout.
#[test]
fn a_high_core_count_host_stays_bounded_in_every_layout() {
    // More rows than one snapshot frame holds, so the ceiling binds.
    const RECORDS: usize = 34_000;
    let mut ceilings = Vec::new();
    for cpus in [Some(4), Some(256), Some(1024), Some(65_536), None] {
        let session = Session::mint();
        let mut source = Moving {
            wide: true,
            ..Moving::new(RECORDS, cpus)
        };
        let (mut view, _) = start_from_source(&session, &mut source).unwrap();
        let rows = view.row_count();
        assert!(rows < RECORDS, "the ceiling must bind: {rows}");
        ceilings.push(rows);
        let cpu = text(&session, CPU_TEXT).unwrap();
        if let Some(count) = cpus {
            assert!(cpu.contains(&format!("all {count} logical CPUs")), "{cpu}");
        }
        assert!(cpu.len() <= summary::MAX_SUMMARY_TEXT_BYTES);
        assert_eq!(session.node_count(), SHELL_NODE_COUNT, "{cpus:?}");
        // Every layout keeps the same rows and at most the default's nodes.
        let mut layout = view.meter_layout().clone();
        for step in [
            MeterLayout::switching as fn(&MeterLayout, Meter) -> MeterLayout,
            MeterLayout::moving_later,
            MeterLayout::toggling,
        ] {
            layout = step(&layout, Meter::Cpu);
            view.set_meter_layout(&session, layout.clone()).unwrap();
            view.refresh(&session, &mut source).unwrap();
            assert_eq!(
                view.row_count(),
                rows,
                "{cpus:?}: a layout moved the ceiling"
            );
            assert!(session.node_count() <= SHELL_NODE_COUNT);
        }
        // A fresh client still attaches with one frame.
        let hello = srui_protocol::ClientHello {
            core_version: "0.5.0".into(),
            profiles: vec!["org.srui.standard-widgets/1".into()],
            client_instance_id: vec![7],
            ..Default::default()
        };
        let snapshot = session
            .bootstrap_fresh_client(&hello)
            .unwrap()
            .snapshot
            .unwrap();
        let bytes = srui_protocol::framed_payload_len(&srui_protocol::SruiMessage {
            msg: Some(srui_protocol::srui_message::Msg::Transaction(snapshot)),
        });
        assert!(bytes <= DEFAULT_MAX_FRAME_SIZE, "{cpus:?}: {bytes}");
        println!(
            "PX-070 high-core-count fixture: cpus={cpus:?} rows={rows} nodes={} frame={bytes}",
            session.node_count()
        );
    }
    assert!(
        ceilings.windows(2).all(|pair| pair[0] == pair[1]),
        "the row ceiling depends on the CPU count: {ceilings:?}"
    );
}

/// Nothing a layout publishes is geometry or appearance: no node carries a
/// size, position or colour property, in any layout the controls reach.
#[test]
fn no_layout_sends_geometry_or_appearance() {
    let mut fixture = Fixture::start(3, Some(4));
    for node in [
        METER_LATER_PROCESSES,
        METER_MODE_CPU,
        METER_TOGGLE_SWAP,
        METERS_HEADER,
    ] {
        fixture.applied(node);
        fixture.refresh();
    }
    for operation in operations_since(&fixture.session, 0) {
        let properties: Vec<PropertyRef> = match &operation {
            Operation::CreateNode { properties, .. } => {
                properties.iter().map(|(property, _)| *property).collect()
            }
            Operation::SetProperty { property, .. } => vec![*property],
            _ => Vec::new(),
        };
        for property in properties {
            assert!(
                ![
                    PropertyRef::MINIMUM_SIZE,
                    PropertyRef::MAXIMUM_SIZE,
                    PropertyRef::PREFERRED_SIZE,
                ]
                .contains(&property),
                "{operation:?}"
            );
        }
    }
}

/// A fresh private directory, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "px070-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn restart(store: &SettingsStore) -> (Session, ProcessView, SavedViews) {
    let session = Session::mint();
    let mut source = Moving::new(3, Some(4));
    let (mut view, _) = start_from_source(&session, &mut source).unwrap();
    let mut views = SavedViews::open(store.clone(), SOURCE);
    views.create_controls(&session, COLUMN).unwrap();
    views.apply_active(&session, &mut view).unwrap();
    (session, view, views)
}

/// A view saved with a meter layout restores it after a restart, as
/// structure; a version 2 file migrates to the default layout and is kept as
/// the backup when the next save writes version 3.
#[test]
fn a_saved_view_restores_its_meter_layout_after_a_restart() {
    let dir = TempDir::new("saved");
    let store = SettingsStore::new(dir.0.join("srtop").join("settings"));
    let (session, mut view, mut views) = restart(&store);
    let layout = view
        .meter_layout()
        .moving_later(Meter::Processes)
        .switching(Meter::Memory)
        .toggling(Meter::Uptime);
    view.set_meter_layout(&session, layout.clone()).unwrap();
    views.set_name_input(&session, "Meters").unwrap();
    views.create(&session, &view).unwrap();
    assert_eq!(views.settings().views.len(), 2, "{}", views.status());
    let saved = std::fs::read_to_string(store.path()).unwrap();
    assert!(saved.starts_with("srtop-settings 3\n"), "{saved}");
    assert!(
        saved.contains("meter-group system.process_count:text\n"),
        "{saved}"
    );

    let (session, view, _) = restart(&store);
    assert_eq!(view.meter_layout(), &layout);
    session.with_store(|store| {
        let parent = |node| store.get_node(node).unwrap().parent_id;
        assert_eq!(parent(MEMORY_BAR), Some(METER_HIDDEN), "memory is text");
        assert_eq!(
            parent(summary::UPTIME_TEXT),
            Some(METER_HIDDEN),
            "uptime is hidden"
        );
        assert_eq!(
            store.children_of(METER_GROUP_2),
            Some([PROCESSES_TEXT].as_slice())
        );
    });
    assert_eq!(layout.presentation(Meter::Memory), Some(Presentation::Text));

    // A version 2 file: the default meters, and a save keeps it as the
    // backup.
    let dir = TempDir::new("v2");
    let store = SettingsStore::new(dir.0.join("settings"));
    let body = format!(
        "srtop-settings 2\napp srtop\nsource {SOURCE}\nactive 0\nview Old\n\
         columns process.pid,process.name\nsort none\nfilter -\nowner all\ntree off\n\
         user-threads off\nkernel-threads on\nend-view -\n"
    );
    let v2 = format!(
        "{body}checksum {:016x}\n",
        srui_process_explorer::settings::fnv1a64(body.as_bytes())
    );
    std::fs::write(store.path(), &v2).unwrap();
    let (session, mut view, mut views) = restart(&store);
    assert!(view.meter_layout().is_default());
    assert!(views.status().contains("version 2"), "{}", views.status());
    view.set_meter_layout(&session, view.meter_layout().toggling_header())
        .unwrap();
    views.set_name_input(&session, "Hidden").unwrap();
    views.create(&session, &view).unwrap();
    assert!(std::fs::read_to_string(store.path())
        .unwrap()
        .starts_with("srtop-settings 3\n"));
    assert_eq!(std::fs::read_to_string(store.backup_path()).unwrap(), v2);
    let (_, view, _) = restart(&store);
    assert!(!view.meter_layout().header());
}
