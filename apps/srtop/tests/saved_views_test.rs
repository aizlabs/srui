//! PX-069 acceptance: named saved views and the versioned settings store
//! (D1 §§6.3, 7.3, 26–27; D2 T32).
//!
//! * A restarted srtop recovers its saved views from the settings file and
//!   applies the active one: columns, sort, filters, view and thread
//!   controls.
//! * Truncated, oversized, unknown-version, foreign, other-source and
//!   partially written files fail safely: a valid backup is recovered and is
//!   never replaced by a damaged file, and a file this build must not replace
//!   is never overwritten. A save interrupted at any step leaves the settings
//!   file whole, old or new.
//! * Version 1 files are migrated; bounds are enforced on read and on every
//!   change.
//! * Renaming or deleting a view changes no process identity, selection or
//!   pending action, and executes nothing; no intent, confirmation or process
//!   identity is ever written to the settings file.
//!
//! Every file lives in a fresh temporary directory; no test names the real
//! user's settings. Clicks travel the runtime's event path
//! (`Session::process_event`) and every commit is applied by a client replica.

use srui_process_explorer::actions::{
    lock_actions, ActionGate, ActionMode, Authorized, DryRunExecutor, Execution, ProcessActions,
    SharedActions,
};
use srui_process_explorer::catalogue::{ColumnLayout, ProcessField, SortDirection};
use srui_process_explorer::owner::Accounts;
use srui_process_explorer::refresh::{lock_view, SharedView, ViewMode};
use srui_process_explorer::saved_views::{lock_saved_views, SavedViews, SharedSavedViews};
use srui_process_explorer::settings::{
    self, fnv1a64, Problem, SavedOwner, SavedView, Settings, ViewName, CURRENT_VERSION,
    MAX_SETTINGS_FILE_BYTES, MAX_VIEWS,
};
use srui_process_explorer::settings_file::{Origin, ReadOnly, SaveError, SaveStep, SettingsStore};
use srui_process_explorer::sort::SortOrder;
use srui_process_explorer::source::*;
use srui_process_explorer::threads::ThreadSettings;
use srui_process_explorer::{
    install_actions, install_column_handlers, install_filter_handlers, install_inspector,
    install_saved_views, install_sort_handlers, start_from_source, ACTION_PREPARE, MODEL,
    SAVED_VIEWS_RESTORE, SAVED_VIEWS_ROW, SAVED_VIEWS_STATUS, SAVED_VIEW_CREATE, SAVED_VIEW_DELETE,
    SAVED_VIEW_NAME_INPUT, SAVED_VIEW_RENAME, SAVED_VIEW_SLOTS, TABLE,
};
use srui_sdk::{ItemId, NodeId, PropertyRef, TypeRef, Value, ACTIVATE, COLUMNS, ENABLED, LABEL};
use srui_semantic_tree::{EditSeq, Event as DomainEvent, SemanticStore};
use srui_sessiond::{EventOutcome, Session};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

const SOURCE: &str = "px069-fixture-v1";
const CLIENT: &[u8] = b"px069-client";
const ME: u32 = 1000;

/// A fresh private directory under the system's temporary directory,
/// removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "px069-{label}-{}-{nanos}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }

    /// The settings path inside a not yet existing subdirectory, so the
    /// store's own directory creation is exercised.
    fn settings(&self) -> PathBuf {
        self.0.join("srtop").join("settings")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn key(pid: u32, token: &str) -> ProcessKey {
    ProcessKey {
        source: SourceId(SOURCE.into()),
        host: Observed::Known(HostId("px069-host".into())),
        boot: Observed::Known(BootId("px069-boot".into())),
        pid_namespace: Observed::Known(PidNamespaceId(7)),
        pid: Observed::Known(pid),
        creation: CreationToken::Opaque(token.into()),
        thread: None,
    }
}

fn record(pid: u32, name: &str, resident: u64, ticks: u64) -> ProcessRecord {
    ProcessRecord {
        kind: KIND_NOT_COLLECTED,
        lineage: Lineage::new(1, 100),
        owner: Observed::Known(ME),
        key: key(pid, name),
        display_name: DisplayName::from(name),
        resident: Observed::Known(resident),
        cpu: CpuUsage::Measured(CpuInterval {
            ticks,
            ticks_per_second: 100,
            elapsed: Duration::from_secs(1),
        }),
    }
}

/// `init` (1), `shell` (4101), `daemon` (4102) and `worker` (4103).
struct Fixture;

impl ProcessSource for Fixture {
    fn status_text(&self) -> &str {
        "Read-only · PX-069 fixture"
    }

    fn accounts(&self) -> Accounts {
        Accounts::fixed(ME, &[(ME, "alice")])
    }

    fn snapshot(&mut self) -> ProcessSnapshot {
        ProcessSnapshot {
            source: SourceId(SOURCE.into()),
            sampled_at: SnapshotTime(SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
            records: vec![
                record(1, "init", 1024, 5),
                record(4101, "shell", 9, 50),
                record(4102, "daemon", 2048, 20),
                record(4103, "worker", 4096, 70),
            ],
            vanished: 0,
            threads: ThreadScan::NotProvided,
            capped: CappedRecords::none(),
            completeness: Completeness::Complete,
            system: SystemSample::not_provided(),
        }
    }
}

/// An executor that records every intent that reaches it.
#[derive(Default)]
struct Recording {
    reached: Mutex<usize>,
    dry_run: DryRunExecutor,
}

impl ProcessActions for Recording {
    fn supports(&self, operation: srui_process_explorer::actions::ActionOperation) -> bool {
        self.dry_run.supports(operation)
    }

    fn execute(&self, action: &Authorized<'_>) -> Execution {
        *self.reached.lock().unwrap() += 1;
        self.dry_run.execute(action)
    }
}

/// One running srtop: the session, its process view and saved views (and
/// action gate, when started with one), and a client replica.
struct Explorer {
    session: Arc<Session>,
    view: SharedView,
    saved: SharedSavedViews,
    gate: Option<SharedActions>,
    executor: Arc<Recording>,
    replica: SemanticStore,
    seq: u64,
}

impl Explorer {
    /// Starts as `main` does: handlers, then the saved views (which apply the
    /// active view), then the actions when asked for.
    fn start(store: SettingsStore, actions: bool) -> Self {
        let session = Arc::new(Session::mint());
        let mut source = Fixture;
        let (view, snapshot) = start_from_source(&session, &mut source).unwrap();
        let view: SharedView = Arc::new(Mutex::new(view));
        install_sort_handlers(&session, &view);
        install_filter_handlers(&session, &view);
        install_column_handlers(&session, &view);
        install_inspector(&session, &view, None);
        let saved: SharedSavedViews =
            Arc::new(Mutex::new(SavedViews::open(store, &snapshot.source.0)));
        install_saved_views(&session, &view, &saved).unwrap();
        let executor = Arc::new(Recording::default());
        let gate = actions.then(|| {
            let gate: SharedActions = Arc::new(Mutex::new(ActionGate::new(
                ActionMode::DryRun,
                Arc::clone(&executor) as Arc<dyn ProcessActions>,
            )));
            install_actions(&session, &view, &gate).unwrap();
            gate
        });
        let mut explorer = Self {
            session,
            view,
            saved,
            gate,
            executor,
            replica: SemanticStore::new(),
            seq: 0,
        };
        explorer.sync();
        explorer
    }

    fn sync(&mut self) {
        for transaction in self
            .session
            .collect_replayed_transactions(self.replica.revision().get())
            .expect("the journal holds this run")
        {
            self.replica.apply_wire_transaction(transaction).unwrap();
        }
        assert_eq!(
            self.replica.revision().get(),
            self.session.current_revision()
        );
    }

    fn deliver(&mut self, wire: &srui_protocol::Event) -> EventOutcome {
        let outcome = self.session.process_event(wire).expect("admitted");
        self.sync();
        outcome
    }

    fn event(
        &mut self,
        node: NodeId,
        event: TypeRef,
        arguments: Vec<(PropertyRef, Value)>,
    ) -> srui_protocol::Event {
        self.seq += 1;
        DomainEvent::new(
            None,
            self.seq,
            format!("px069-event-{}", self.seq),
            self.session.current_revision(),
            node,
            event,
            arguments,
        )
        .with_client_instance_id(CLIENT.to_vec())
        .to_wire()
    }

    fn click(&mut self, node: NodeId) -> EventOutcome {
        let wire = self.event(node, ACTIVATE, Vec::new());
        self.deliver(&wire)
    }

    fn processed(&mut self, node: NodeId) {
        let outcome = self.click(node);
        assert!(
            matches!(outcome, EventOutcome::Processed { .. }),
            "{node:?}: {outcome:?}"
        );
    }

    fn type_name(&mut self, text: &str) {
        self.seq += 1;
        let wire = DomainEvent::text_edit(
            self.seq,
            format!("px069-edit-{}", self.seq),
            self.session.current_revision(),
            SAVED_VIEW_NAME_INPUT.get(),
            text,
            EditSeq::new(self.seq).unwrap(),
        )
        .with_client_instance_id(CLIENT.to_vec())
        .to_wire();
        assert!(matches!(
            self.deliver(&wire),
            EventOutcome::Processed { .. }
        ));
    }

    fn select(&mut self, pid: u64) {
        let item = self.item_of(pid);
        let wire = self.event(
            TABLE,
            TypeRef::EVENT_SELECTION_CHANGED,
            vec![(PropertyRef::VALUE, Value::ItemId(item))],
        );
        assert!(matches!(
            self.deliver(&wire),
            EventOutcome::Processed { .. }
        ));
    }

    fn property(&self, node: NodeId, property: PropertyRef) -> Option<Value> {
        self.replica
            .get_node(node)
            .and_then(|node| node.get_property(property))
            .cloned()
    }

    fn label(&self, node: NodeId) -> String {
        match self.property(node, LABEL) {
            Some(Value::String(text)) => text,
            other => panic!("no label on {node:?}: {other:?}"),
        }
    }

    fn status(&self) -> String {
        match self.property(SAVED_VIEWS_STATUS, PropertyRef::TEXT) {
            Some(Value::String(text)) => text,
            other => panic!("no views status: {other:?}"),
        }
    }

    fn enabled(&self, node: NodeId) -> bool {
        self.property(node, ENABLED) != Some(Value::Bool(false))
    }

    fn headings(&self) -> Vec<String> {
        match self.property(TABLE, COLUMNS) {
            Some(Value::List(headings)) => headings
                .into_iter()
                .map(|heading| match heading {
                    Value::String(text) => text,
                    other => panic!("heading {other:?}"),
                })
                .collect(),
            other => panic!("headings: {other:?}"),
        }
    }

    fn item_ids(&self) -> Vec<ItemId> {
        let model = self.replica.get_model(MODEL).unwrap();
        (0..model.item_count)
            .map(|index| model.get_item_by_index(index).unwrap().item_id)
            .collect()
    }

    fn item_of(&self, pid: u64) -> ItemId {
        let at = self
            .headings()
            .iter()
            .position(|heading| heading == ProcessField::Pid.heading())
            .unwrap_or_else(|| panic!("a PID column: {:?}", self.headings()));
        let model = self.replica.get_model(MODEL).unwrap();
        (0..model.item_count)
            .map(|index| model.get_item_by_index(index).unwrap())
            .find(|item| match &item.value {
                Value::List(cells) => match &cells[at] {
                    Value::UnsignedInt(shown) => *shown == pid,
                    Value::String(shown) => shown == &pid.to_string(),
                    _ => false,
                },
                _ => false,
            })
            .unwrap_or_else(|| panic!("PID {pid} is shown"))
            .item_id
    }

    fn settings(&self) -> Settings {
        lock_saved_views(&self.saved).settings().clone()
    }
}

fn presentation(explorer: &Explorer) -> (Vec<ProcessField>, Option<SortOrder>, String, ViewMode) {
    let view = lock_view(&explorer.view);
    (
        view.columns(),
        view.sort(),
        view.committed_filter().raw().to_string(),
        view.view_mode(),
    )
}

fn view(name: &str, columns: &[ProcessField]) -> SavedView {
    SavedView {
        name: ViewName::parse(name).unwrap(),
        columns: ColumnLayout::new(columns).unwrap(),
        sort: Some(SortOrder {
            field: ProcessField::Cpu,
            direction: SortDirection::Descending,
        }),
        filter: "worker".into(),
        owner: SavedOwner::Mine,
        tree: false,
        threads: ThreadSettings::default(),
    }
}

fn settings_with(views: &[&str]) -> Settings {
    let mut settings = Settings::defaults(SOURCE);
    for name in views {
        settings
            .views
            .push(view(name, &[ProcessField::Name, ProcessField::Cpu]));
    }
    settings.active = Some(settings.views.len() - 1);
    settings
}

/// A settings file with `body` (everything before the checksum line) and a
/// valid checksum.
fn signed(body: &str) -> String {
    format!("{body}checksum {:016x}\n", fnv1a64(body.as_bytes()))
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap()
}

// ---------------------------------------------------------------------------
// Restart recovery and the switcher
// ---------------------------------------------------------------------------

#[test]
fn a_restarted_srtop_recovers_its_views_and_applies_the_active_one() {
    let dir = TempDir::new("restart");
    let path = dir.settings();
    let mut first = Explorer::start(SettingsStore::new(&path), false);
    // Nothing is written until a view changes.
    assert!(!path.exists(), "loading wrote a settings file");
    assert_eq!(first.settings(), Settings::defaults(SOURCE));
    assert_eq!(first.label(SAVED_VIEW_SLOTS[0]), "View: Default (active)");
    assert!(
        !first.enabled(SAVED_VIEW_SLOTS[1]),
        "an empty slot is disabled"
    );
    assert!(
        !first.enabled(SAVED_VIEW_DELETE),
        "the last view cannot be deleted"
    );
    assert!(
        !first.enabled(SAVED_VIEWS_RESTORE),
        "the defaults are in use"
    );

    // Arrange the list: CPU and Name only, by CPU descending, filtered to
    // workers, as a user's clicks and typing would, then save it.
    {
        let mut view = lock_view(&first.view);
        view.set_columns(&first.session, &[ProcessField::Cpu, ProcessField::Name])
            .unwrap();
        view.set_sort(
            &first.session,
            SortOrder {
                field: ProcessField::Cpu,
                direction: SortDirection::Descending,
            },
        )
        .unwrap();
        view.set_filter(&first.session, "worker|shell").unwrap();
    }
    first.sync();
    let arranged = presentation(&first);
    first.type_name("  Busy workers  ");
    first.processed(SAVED_VIEW_CREATE);
    assert_eq!(first.label(SAVED_VIEW_SLOTS[0]), "View: Default");
    assert_eq!(
        first.label(SAVED_VIEW_SLOTS[1]),
        "View: Busy workers (active)"
    );
    assert!(first.enabled(SAVED_VIEW_SLOTS[1]));
    assert!(
        first.status().contains("Saved view “Busy workers”"),
        "{}",
        first.status()
    );
    assert_eq!(
        first.property(SAVED_VIEW_NAME_INPUT, PropertyRef::VALUE),
        Some(Value::String(String::new())),
        "the name field is cleared once the view is saved"
    );
    let saved = read(&path);
    assert!(saved.starts_with(format!("srtop-settings {CURRENT_VERSION}\n").as_bytes()));

    // Switch back to the default view: the default layout and order.
    first.processed(SAVED_VIEW_SLOTS[0]);
    assert_eq!(
        presentation(&first).0,
        ColumnLayout::default().fields().to_vec()
    );
    assert_eq!(presentation(&first).2, "");
    // And back again: exactly the arranged list.
    first.processed(SAVED_VIEW_SLOTS[1]);
    assert_eq!(presentation(&first), arranged);
    drop(first);

    // A restart reads the file and applies the active view at once.
    let second = Explorer::start(SettingsStore::new(&path), false);
    assert_eq!(presentation(&second), arranged);
    assert_eq!(
        second.label(SAVED_VIEW_SLOTS[1]),
        "View: Busy workers (active)"
    );
    assert_eq!(
        second.headings(),
        vec![
            ProcessField::Cpu.heading().to_string(),
            ProcessField::Name.heading().to_string(),
            srui_process_explorer::tags::TAG_HEADING.to_string()
        ]
    );
    assert_eq!(
        second.item_ids().len(),
        2,
        "the saved filter keeps two rows"
    );
    assert_eq!(second.settings().views.len(), 2);
}

#[test]
fn an_empty_slot_is_refused_by_event_validation_and_a_slot_applies_only_what_the_server_holds() {
    let dir = TempDir::new("slots");
    SettingsStore::new(dir.settings())
        .save(&settings_with(&["Mine"]))
        .unwrap();
    let mut explorer = Explorer::start(SettingsStore::new(dir.settings()), false);
    assert_eq!(
        presentation(&explorer).0,
        vec![ProcessField::Name, ProcessField::Cpu]
    );
    let before = std::fs::read(dir.settings()).unwrap();
    let outcome = explorer.click(SAVED_VIEW_SLOTS[5]);
    assert!(
        matches!(
            outcome,
            EventOutcome::Rejected {
                error: srui_semantic_tree::EventValidationError::NodeDisabled(_),
                ..
            }
        ),
        "{outcome:?}"
    );
    assert_eq!(std::fs::read(dir.settings()).unwrap(), before);
    // Even a click that reached the handler of an empty slot applies nothing.
    lock_saved_views(&explorer.saved)
        .apply_slot(&explorer.session, &mut lock_view(&explorer.view), 5)
        .unwrap();
    assert_eq!(std::fs::read(dir.settings()).unwrap(), before);
    assert_eq!(
        presentation(&explorer).0,
        vec![ProcessField::Name, ProcessField::Cpu]
    );
}

#[test]
fn restore_defaults_saves_and_applies_the_default_view_and_keeps_the_previous_file_as_backup() {
    let dir = TempDir::new("restore");
    let store = SettingsStore::new(dir.settings());
    store.save(&settings_with(&["A", "B"])).unwrap();
    let previous = read(store.path());
    let mut explorer = Explorer::start(store.clone(), false);
    assert!(explorer.enabled(SAVED_VIEWS_RESTORE));
    explorer.processed(SAVED_VIEWS_RESTORE);
    assert_eq!(explorer.settings(), Settings::defaults(SOURCE));
    assert_eq!(
        settings::parse(&read(store.path())).unwrap().settings,
        Settings::defaults(SOURCE)
    );
    assert_eq!(read(&store.backup_path()), previous);
    assert_eq!(
        presentation(&explorer).0,
        ColumnLayout::default().fields().to_vec()
    );
    assert!(!explorer.enabled(SAVED_VIEWS_RESTORE));
    assert!(!explorer.enabled(SAVED_VIEW_SLOTS[1]));
}

// ---------------------------------------------------------------------------
// Files that fail safely
// ---------------------------------------------------------------------------

/// A store holding `new` with `old` as its backup.
fn two_saves(dir: &TempDir) -> (SettingsStore, Settings, Settings) {
    let store = SettingsStore::new(dir.settings());
    let old = settings_with(&["Old"]);
    let new = settings_with(&["New"]);
    store.save(&old).unwrap();
    store.save(&new).unwrap();
    assert_eq!(
        settings::parse(&read(&store.backup_path()))
            .unwrap()
            .settings,
        old
    );
    (store, old, new)
}

#[test]
fn a_truncated_or_partly_written_file_falls_back_to_the_backup_which_it_never_replaces() {
    let dir = TempDir::new("truncated");
    let (store, old, new) = two_saves(&dir);
    let whole = read(store.path());
    let backup = read(&store.backup_path());
    for cut in [0, 1, whole.len() / 3, whole.len() / 2, whole.len() - 1] {
        std::fs::write(store.path(), &whole[..cut]).unwrap();
        let loaded = store.load(SOURCE);
        assert!(
            matches!(loaded.origin, Origin::Backup { .. }),
            "cut at {cut}: {:?}",
            loaded.origin
        );
        assert_eq!(
            loaded.settings, old,
            "cut at {cut}: the backup is recovered"
        );
        assert_eq!(loaded.read_only, None);
    }
    // A save over a damaged file replaces it and keeps the valid backup.
    let third = settings_with(&["Third"]);
    store.save(&third).unwrap();
    assert_eq!(
        read(&store.backup_path()),
        backup,
        "the damaged file became the backup"
    );
    assert_eq!(store.load(SOURCE).settings, third);
    // The damaged file is not the only way: one flipped byte is refused too.
    let mut damaged = read(store.path());
    let middle = damaged.len() / 2;
    damaged[middle] ^= 0x01;
    std::fs::write(store.path(), &damaged).unwrap();
    let loaded = store.load(SOURCE);
    assert_eq!(
        loaded.origin,
        Origin::Backup {
            primary: Problem::Checksum
        }
    );
    let _ = new;
}

#[test]
fn an_oversized_file_is_refused_as_oversized_without_losing_the_backup() {
    let dir = TempDir::new("oversized");
    let (store, old, _) = two_saves(&dir);
    let backup = read(&store.backup_path());
    // A well-formed head, then far more than the bound.
    let mut big = read(store.path());
    big.resize(MAX_SETTINGS_FILE_BYTES + 4096, b'#');
    std::fs::write(store.path(), &big).unwrap();
    let loaded = store.load(SOURCE);
    assert_eq!(
        loaded.origin,
        Origin::Backup {
            primary: Problem::Oversized
        }
    );
    assert_eq!(loaded.settings, old);
    store.save(&settings_with(&["After"])).unwrap();
    assert_eq!(read(&store.backup_path()), backup);
    // And one byte past the bound, once the head says it is this build's.
    let mut past = b"srtop-settings 2\napp srtop\n".to_vec();
    past.resize(MAX_SETTINGS_FILE_BYTES + 1, b'x');
    assert_eq!(settings::parse(&past), Err(Problem::Oversized));
    // An oversized file nobody may read could be anything, a newer srtop's
    // among them: the store is read-only and never replaces it.
    std::fs::write(store.path(), &big).unwrap();
    std::fs::set_permissions(store.path(), std::fs::Permissions::from_mode(0o000)).unwrap();
    assert!(matches!(
        store.load(SOURCE).read_only,
        Some(ReadOnly::Unreadable(_))
    ));
    assert!(matches!(
        store.save(&settings_with(&["Later"])),
        Err(SaveError::Refused(ReadOnly::Unreadable(_)))
    ));
    std::fs::set_permissions(store.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(read(store.path()), big);
}

/// PX-069 verification round 1, F1: the version and the application are read
/// from the head before any bound, so a newer srtop's or another
/// application's file is never replaced however large or long it is, at the
/// settings path or at the backup's.
#[test]
fn an_oversized_or_long_newer_or_foreign_file_is_never_overwritten() {
    let dir = TempDir::new("bignewer");
    let (store, old, new) = two_saves(&dir);
    let backup = read(&store.backup_path());
    let oversized = |head: &str| {
        let mut bytes = head.as_bytes().to_vec();
        bytes.resize(MAX_SETTINGS_FILE_BYTES + 10_000, b'#');
        bytes.push(b'\n');
        bytes
    };
    let newer = oversized(&format!(
        "srtop-settings {}\napp srtop\n",
        CURRENT_VERSION + 1
    ));
    let foreign = oversized("srtop-settings 2\napp other\n");
    let mut long = format!("srtop-settings {}\n", CURRENT_VERSION + 1);
    for line in 0..settings::MAX_LINES + 50 {
        long.push_str(&format!("k{line} v\n"));
    }
    for (name, bytes, reason) in [
        (
            "oversized newer",
            newer.clone(),
            ReadOnly::NewerVersion(CURRENT_VERSION + 1),
        ),
        ("oversized foreign", foreign, ReadOnly::ForeignApp),
        (
            "long newer",
            long.into_bytes(),
            ReadOnly::NewerVersion(CURRENT_VERSION + 1),
        ),
    ] {
        std::fs::write(store.path(), &bytes).unwrap();
        let loaded = store.load(SOURCE);
        assert_eq!(loaded.read_only, Some(reason.clone()), "{name}");
        // The valid backup's views are shown, read-only (V2-1).
        assert_eq!(loaded.settings, old, "{name}");
        assert_eq!(
            store.save(&settings_with(&["Third"])),
            Err(SaveError::Refused(reason)),
            "{name}"
        );
        assert_eq!(read(store.path()), bytes, "{name}: the file was replaced");
        assert_eq!(
            read(&store.backup_path()),
            backup,
            "{name}: the backup was replaced"
        );
    }

    // The backup too: with a valid settings file, a save would replace the
    // backup, so a newer srtop's backup makes the store read-only.
    std::fs::write(store.path(), new.encode().unwrap()).unwrap();
    std::fs::write(store.backup_path(), &newer).unwrap();
    let loaded = store.load(SOURCE);
    assert_eq!(
        loaded.settings, new,
        "the settings file itself is still shown"
    );
    let protected = ReadOnly::Backup(Box::new(ReadOnly::NewerVersion(CURRENT_VERSION + 1)));
    assert_eq!(loaded.read_only, Some(protected.clone()));
    assert_eq!(
        store.save(&settings_with(&["Third"])),
        Err(SaveError::Refused(protected))
    );
    assert_eq!(read(&store.backup_path()), newer);
    // With a damaged settings file the backup is not replaced, so the save
    // goes ahead and the newer backup stays.
    std::fs::write(store.path(), b"srtop-settings 2\napp srtop\ncut").unwrap();
    assert_eq!(store.load(SOURCE).read_only, None);
    store.save(&settings_with(&["Third"])).unwrap();
    assert_eq!(read(&store.backup_path()), newer);
    assert_eq!(store.load(SOURCE).settings, settings_with(&["Third"]));
}

/// PX-069 verification round 1, F2: an explicit `--settings-path` naming some
/// other file never destroys it; an empty file or the start of the magic is
/// a cut-short settings file and may be replaced.
#[test]
fn an_unrelated_file_at_the_settings_path_is_never_replaced() {
    let dir = TempDir::new("unrelated");
    let path = dir.0.join("notes.txt");
    for content in [
        &b"my important notes\n"[..],
        b"#!/bin/sh\n",
        b"srtop-settingsX 2\n",
        b"\xff\xfe",
    ] {
        std::fs::write(&path, content).unwrap();
        let store = SettingsStore::new(&path);
        let loaded = store.load(SOURCE);
        assert_eq!(loaded.read_only, Some(ReadOnly::NotSettings), "{content:?}");
        assert_eq!(
            store.save(&settings_with(&["A"])),
            Err(SaveError::Refused(ReadOnly::NotSettings))
        );
        assert_eq!(read(&path), content);
        assert!(!store.backup_path().exists());
    }
    for damaged in [&b""[..], b"srtop-set", b"srtop-settings "] {
        std::fs::write(&path, damaged).unwrap();
        let store = SettingsStore::new(&path);
        assert_eq!(store.load(SOURCE).read_only, None, "{damaged:?}");
        store.save(&settings_with(&["A"])).unwrap();
        assert_eq!(store.load(SOURCE).settings, settings_with(&["A"]));
        assert!(
            !store.backup_path().exists(),
            "a damaged file became the backup"
        );
        std::fs::remove_file(&path).unwrap();
    }
}

/// PX-069 verification round 2, V2-1: damage to the settings file's head
/// (zero-filled, a flipped magic byte, an unknown first line) keeps the file
/// unreplaced, shows the valid backup read-only and says so with both paths;
/// moving the backup over the file, or deleting only the file, recovers it.
#[test]
fn head_damage_shows_the_valid_backup_read_only_and_names_both_files() {
    let dir = TempDir::new("head");
    let (store, old, new) = two_saves(&dir);
    let whole = read(store.path());
    let backup = read(&store.backup_path());
    let mut cases: Vec<(&str, Vec<u8>, ReadOnly)> = vec![
        ("zero-filled", vec![0; whole.len()], ReadOnly::NotSettings),
        (
            "first 64 bytes zeroed",
            {
                let mut bytes = whole.clone();
                bytes[..64].fill(0);
                bytes
            },
            ReadOnly::NotSettings,
        ),
        (
            "CRLF first line",
            {
                let mut bytes = b"srtop-settings 2\r\n".to_vec();
                bytes.extend_from_slice(&whole[17..]);
                bytes
            },
            ReadOnly::UnknownFormat,
        ),
    ];
    for at in [0, 5, 13] {
        let mut flipped = whole.clone();
        flipped[at] ^= 0x01;
        cases.push(("flipped magic", flipped, ReadOnly::NotSettings));
    }
    for (name, bytes, reason) in cases {
        std::fs::write(store.path(), &bytes).unwrap();
        let loaded = store.load(SOURCE);
        assert_eq!(loaded.read_only, Some(reason.clone()), "{name}");
        assert!(
            matches!(loaded.origin, Origin::Backup { .. }),
            "{name}: {:?}",
            loaded.origin
        );
        assert_eq!(loaded.settings, old, "{name}: the backup is shown");
        let status = SavedViews::open(store.clone(), SOURCE).status();
        for expected in [
            format!(
                "the settings file {} {}",
                store.path().display(),
                reason.predicate()
            ),
            format!("its backup {} is shown", store.backup_path().display()),
        ] {
            assert!(status.contains(&expected), "{name}: {status}");
        }
        assert!(
            !status.contains("default view is shown"),
            "{name}: {status}"
        );
        assert_eq!(
            store.save(&settings_with(&["Third"])),
            Err(SaveError::Refused(reason)),
            "{name}"
        );
        assert_eq!(
            read(store.path()),
            bytes,
            "{name}: the damaged file was replaced"
        );
        assert_eq!(
            read(&store.backup_path()),
            backup,
            "{name}: the backup was replaced"
        );
    }
    // Through the controls: the backup's active view is applied, changes are
    // disabled, Restore default views among them (it would replace the file).
    let explorer = Explorer::start(store.clone(), false);
    assert_eq!(explorer.settings(), old);
    assert_eq!(
        presentation(&explorer).0,
        vec![ProcessField::Name, ProcessField::Cpu]
    );
    assert!(!explorer.enabled(SAVED_VIEWS_RESTORE));
    drop(explorer);

    // Recovery 1: move the backup over the settings file.
    std::fs::rename(store.backup_path(), store.path()).unwrap();
    let loaded = store.load(SOURCE);
    assert_eq!((loaded.read_only, loaded.settings), (None, old.clone()));
    // Recovery 2: delete only the settings file; the backup is shown, and the
    // next save creates a new settings file without touching the backup.
    store.save(&new).unwrap();
    std::fs::write(store.path(), vec![0; 32]).unwrap();
    std::fs::remove_file(store.path()).unwrap();
    let loaded = store.load(SOURCE);
    assert_eq!(loaded.origin, Origin::BackupOnly);
    assert_eq!((loaded.read_only, loaded.settings), (None, old.clone()));
    let status = SavedViews::open(store.clone(), SOURCE).status();
    assert!(
        status.contains(&format!(
            "There is no settings file {}; its backup {} was loaded",
            store.path().display(),
            store.backup_path().display()
        )),
        "{status}"
    );
    let backup = read(&store.backup_path());
    store.save(&settings_with(&["Recovered"])).unwrap();
    assert_eq!(store.load(SOURCE).settings, settings_with(&["Recovered"]));
    assert_eq!(read(&store.backup_path()), backup);
}

/// PX-069 verification round 2, V2-2: every read-only status says which file
/// is protected and what is shown.
#[test]
fn each_read_only_status_names_the_protected_file_and_what_is_shown() {
    let dir = TempDir::new("status");
    let (store, _, new) = two_saves(&dir);
    let (path, bak) = (
        store.path().display().to_string(),
        store.backup_path().display().to_string(),
    );
    // A valid settings file and a protected backup: the file's views are
    // shown and applied; the backup is the one named.
    for (backup_bytes, reason) in [
        (
            format!("srtop-settings {}\n", CURRENT_VERSION + 1).into_bytes(),
            ReadOnly::NewerVersion(CURRENT_VERSION + 1),
        ),
        (
            b"srtop-settings 2\napp other\nx\n".to_vec(),
            ReadOnly::ForeignApp,
        ),
        (b"my notes\n".to_vec(), ReadOnly::NotSettings),
    ] {
        std::fs::write(store.backup_path(), &backup_bytes).unwrap();
        let views = SavedViews::open(store.clone(), SOURCE);
        assert_eq!(views.settings(), &new);
        assert_eq!(
            views.read_only(),
            Some(&ReadOnly::Backup(Box::new(reason.clone())))
        );
        let status = views.status();
        assert!(
            status.contains(&format!("Not saving: the backup {bak} {}; it is left unchanged. The views of {path} are shown and can be switched", reason.predicate())),
            "{status}"
        );
        assert!(!status.contains("default view"), "{status}");
        assert!(
            !status.contains("the settings file"),
            "the settings file is fine: {status}"
        );
        let explorer = Explorer::start(store.clone(), false);
        assert_eq!(
            presentation(&explorer).0,
            vec![ProcessField::Name, ProcessField::Cpu],
            "the active view is applied"
        );
    }
    // A protected settings file and no usable backup: the defaults.
    std::fs::remove_file(store.backup_path()).unwrap();
    std::fs::write(
        store.path(),
        format!("srtop-settings {}\n", CURRENT_VERSION + 1),
    )
    .unwrap();
    let status = SavedViews::open(store.clone(), SOURCE).status();
    assert!(
        status.contains(&format!("Not saving: the settings file {path} was written by a newer srtop (version {}); it is left unchanged and the default view is shown", CURRENT_VERSION + 1)),
        "{status}"
    );
    // A settings file of another source: the defaults, the file named.
    let theirs = Settings {
        source: "procfs:/proc".into(),
        ..settings_with(&["Live"])
    };
    std::fs::write(store.path(), theirs.encode().unwrap()).unwrap();
    let status = SavedViews::open(store.clone(), SOURCE).status();
    assert!(
        status.contains(&format!("Not saving: the settings file {path} was saved for another process source; it is left unchanged and the default view is shown")),
        "{status}"
    );
}

/// PX-069 verification round 2, V2-4: the first line's shape is frozen, so a
/// first line that starts like a settings file's but has another shape is
/// never replaced.
#[test]
fn an_unknown_first_line_shape_is_never_replaced() {
    let dir = TempDir::new("shape");
    let (store, old, _) = two_saves(&dir);
    for (head, reason) in [
        ("srtop-settings 3\r\n", ReadOnly::NewerVersion(3)),
        ("srtop-settings 3 \n", ReadOnly::NewerVersion(3)),
        ("srtop-settings 3.0\n", ReadOnly::NewerVersion(3)),
        ("srtop-settings 2\r\n", ReadOnly::UnknownFormat),
        ("srtop-settings 2.1\n", ReadOnly::UnknownFormat),
        ("srtop-settings 1234567890\n", ReadOnly::UnknownFormat),
    ] {
        let bytes = format!("{head}app srtop\nfuture stuff\n");
        std::fs::write(store.path(), &bytes).unwrap();
        let loaded = store.load(SOURCE);
        assert_eq!(loaded.read_only, Some(reason.clone()), "{head:?}");
        assert_eq!(loaded.settings, old, "{head:?}");
        assert_eq!(
            store.save(&settings_with(&["X"])),
            Err(SaveError::Refused(reason)),
            "{head:?}"
        );
        assert_eq!(read(store.path()), bytes.as_bytes(), "{head:?}");
    }
}

/// PX-069 verification round 1, F5: a directory flush that fails after the
/// rename leaves the new file in place, and the views say they were saved,
/// with the warning.
#[test]
fn a_failed_directory_flush_after_the_rename_keeps_the_new_views_and_says_so() {
    let dir = TempDir::new("flush");
    let store = SettingsStore::new(dir.settings());
    store.save(&settings_with(&["Old"])).unwrap();
    let mut explorer = Explorer::start(store.clone().interrupted_at(SaveStep::FlushFailed), false);
    explorer.type_name("Flushed");
    explorer.processed(SAVED_VIEW_CREATE);
    let names: Vec<String> = explorer
        .settings()
        .views
        .iter()
        .map(|view| view.name.to_string())
        .collect();
    assert_eq!(names, ["Default", "Old", "Flushed"]);
    assert_eq!(store.load(SOURCE).settings, explorer.settings());
    assert_eq!(
        explorer.label(SAVED_VIEW_SLOTS[2]),
        "View: Flushed (active)"
    );
    assert!(
        explorer.status().contains(
            "Saved view “Flushed”, saved, but the settings directory could not be flushed"
        ),
        "{}",
        explorer.status()
    );
}

/// From verification round 1's probes: links planted at the backup's and at
/// this process's temporary name are replaced or removed as links, never
/// followed; their targets are untouched.
#[test]
fn links_at_the_backup_or_temporary_name_are_never_followed() {
    let dir = TempDir::new("links");
    let store = SettingsStore::new(dir.settings());
    store.save(&settings_with(&["A"])).unwrap();
    let target = dir.0.join("bak-target");
    std::fs::write(&target, b"BAKTARGET").unwrap();
    std::os::unix::fs::symlink(&target, store.backup_path()).unwrap();
    store.save(&settings_with(&["B"])).unwrap();
    assert_eq!(read(&target), b"BAKTARGET");
    assert!(std::fs::symlink_metadata(store.backup_path())
        .unwrap()
        .is_file());
    let target = dir.0.join("tmp-target");
    std::fs::write(&target, b"TMPTARGET").unwrap();
    let temporary = dir
        .settings()
        .with_file_name(format!("settings.tmp.{}", std::process::id()));
    std::os::unix::fs::symlink(&target, &temporary).unwrap();
    store.save(&settings_with(&["C"])).unwrap();
    assert_eq!(read(&target), b"TMPTARGET");
    assert_eq!(store.load(SOURCE).settings, settings_with(&["C"]));
}

#[test]
fn a_newer_versions_file_is_read_only_shows_the_backup_and_is_never_overwritten() {
    let dir = TempDir::new("newer");
    let (store, old, _) = two_saves(&dir);
    let backup = read(&store.backup_path());
    let future = format!(
        "srtop-settings {}\nwhatever a later srtop writes\n",
        CURRENT_VERSION + 1
    );
    std::fs::write(store.path(), &future).unwrap();
    let loaded = store.load(SOURCE);
    assert_eq!(
        loaded.read_only,
        Some(ReadOnly::NewerVersion(CURRENT_VERSION + 1))
    );
    assert_eq!(loaded.settings, old, "the valid backup is shown, read-only");
    assert_eq!(
        store.save(&settings_with(&["Mine"])),
        Err(SaveError::Refused(ReadOnly::NewerVersion(
            CURRENT_VERSION + 1
        )))
    );
    assert_eq!(
        read(store.path()),
        future.as_bytes(),
        "the newer file is untouched"
    );
    assert_eq!(read(&store.backup_path()), backup);

    // Through the controls: switching works, every change is disabled.
    let mut explorer = Explorer::start(store.clone(), false);
    for node in [
        SAVED_VIEW_CREATE,
        SAVED_VIEW_RENAME,
        SAVED_VIEW_DELETE,
        SAVED_VIEWS_RESTORE,
        SAVED_VIEW_NAME_INPUT,
    ] {
        assert!(
            !explorer.enabled(node),
            "{node:?} is enabled on a read-only store"
        );
    }
    assert!(
        explorer.status().contains("newer srtop"),
        "{}",
        explorer.status()
    );
    explorer.processed(SAVED_VIEW_SLOTS[0]);
    // A handler reached all the same still writes nothing.
    {
        let mut views = lock_saved_views(&explorer.saved);
        views.set_name_input(&explorer.session, "Forced").unwrap();
        views
            .create(&explorer.session, &lock_view(&explorer.view))
            .unwrap();
        views
            .restore_defaults(&explorer.session, &mut lock_view(&explorer.view))
            .unwrap();
    }
    assert_eq!(read(store.path()), future.as_bytes());
    assert_eq!(read(&store.backup_path()), backup);
}

#[test]
fn unknown_old_versions_foreign_files_other_sources_and_non_files_fail_safely() {
    let dir = TempDir::new("foreign");
    let (store, old, _) = two_saves(&dir);
    // Version 0 is no version: damaged, so the backup is recovered.
    std::fs::write(store.path(), "srtop-settings 0\n").unwrap();
    assert_eq!(
        store.load(SOURCE).origin,
        Origin::Backup {
            primary: Problem::BadVersion
        }
    );
    // Another application's file is never replaced.
    let foreign = signed("srtop-settings 2\napp other\n");
    std::fs::write(store.path(), &foreign).unwrap();
    assert_eq!(store.load(SOURCE).read_only, Some(ReadOnly::ForeignApp));
    assert!(store.save(&old).is_err());
    assert_eq!(read(store.path()), foreign.as_bytes());
    // Another source's views are neither applied nor replaced.
    let theirs = Settings {
        source: "procfs:/proc".into(),
        ..settings_with(&["Live"])
    };
    std::fs::write(store.path(), theirs.encode().unwrap()).unwrap();
    let loaded = store.load(SOURCE);
    assert_eq!(loaded.read_only, Some(ReadOnly::OtherSource));
    assert_eq!(loaded.settings, Settings::defaults(SOURCE));
    assert_eq!(
        store.save(&old),
        Err(SaveError::Refused(ReadOnly::OtherSource))
    );
    // A symbolic link is never followed, even to a valid file.
    std::fs::remove_file(store.path()).unwrap();
    std::os::unix::fs::symlink(store.backup_path(), store.path()).unwrap();
    assert_eq!(store.load(SOURCE).read_only, Some(ReadOnly::NotAFile));
    assert_eq!(
        store.save(&old),
        Err(SaveError::Refused(ReadOnly::NotAFile))
    );
    assert!(std::fs::symlink_metadata(store.path())
        .unwrap()
        .file_type()
        .is_symlink());
    // A directory where the file should be.
    std::fs::remove_file(store.path()).unwrap();
    std::fs::create_dir(store.path()).unwrap();
    assert_eq!(store.load(SOURCE).read_only, Some(ReadOnly::NotAFile));
    assert!(store.save(&old).is_err());
    assert!(store.path().is_dir());
}

#[test]
fn a_save_interrupted_at_any_step_leaves_the_settings_file_whole_old_or_new() {
    for step in [
        SaveStep::BackupReplaced,
        SaveStep::HalfWritten,
        SaveStep::Written,
        SaveStep::Renamed,
    ] {
        let dir = TempDir::new("interrupted");
        let (store, old, new) = two_saves(&dir);
        let next = settings_with(&["Next"]);
        assert_eq!(
            store.clone().interrupted_at(step).save(&next),
            Err(SaveError::Interrupted(step))
        );
        let primary = settings::parse(&read(store.path()))
            .unwrap_or_else(|problem| panic!("{step:?}: the settings file is {problem}"))
            .settings;
        assert!(
            primary == new || primary == next,
            "{step:?}: the settings file is neither the old nor the new one"
        );
        let backup = settings::parse(&read(&store.backup_path()))
            .unwrap_or_else(|problem| panic!("{step:?}: the backup is {problem}"))
            .settings;
        assert!(backup == old || backup == new, "{step:?}");
        let loaded = store.load(SOURCE);
        assert!(loaded.settings == new || loaded.settings == next);
        // The next save completes and sweeps nothing it should not.
        store.save(&next).unwrap();
        assert_eq!(store.load(SOURCE).settings, next);
        assert_eq!(
            settings::parse(&read(&store.backup_path()))
                .unwrap()
                .settings,
            {
                if primary == next {
                    next.clone()
                } else {
                    new.clone()
                }
            }
        );
    }
}

#[test]
fn a_save_renames_a_fresh_private_file_into_place_and_keeps_exactly_one_backup() {
    let dir = TempDir::new("atomic");
    let store = SettingsStore::new(dir.settings());
    store.save(&settings_with(&["One"])).unwrap();
    let directory = store.path().parent().unwrap().to_path_buf();
    assert_eq!(
        std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(store.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(!store.backup_path().exists(), "nothing to back up yet");
    // Hold the first file open by another name: a rename leaves it whole.
    let held = dir.0.join("held");
    std::fs::hard_link(store.path(), &held).unwrap();
    let first = read(store.path());
    let first_inode = std::fs::metadata(store.path()).unwrap().ino();
    store.save(&settings_with(&["Two"])).unwrap();
    assert_ne!(
        std::fs::metadata(store.path()).unwrap().ino(),
        first_inode,
        "the settings file was rewritten in place, not replaced"
    );
    assert_eq!(read(&held), first, "the replaced file was modified");
    assert_eq!(read(&store.backup_path()), first);
    store.save(&settings_with(&["Three"])).unwrap();
    let mut names: Vec<String> = std::fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["settings", "settings.bak"],
        "one file and one backup"
    );
    // A temporary a crashed save left behind is swept once it is stale, and
    // never read.
    let leftover = directory.join("settings.tmp.99999999");
    std::fs::write(&leftover, b"srtop-settings 2\npartial").unwrap();
    let stale = SystemTime::now() - Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(&leftover)
        .unwrap()
        .set_modified(stale)
        .unwrap();
    let fresh = directory.join("settings.tmp.99999998");
    std::fs::write(&fresh, b"in progress").unwrap();
    assert_eq!(store.load(SOURCE).settings, settings_with(&["Three"]));
    store.save(&settings_with(&["Four"])).unwrap();
    assert!(!leftover.exists(), "a stale temporary file was kept");
    assert!(
        fresh.exists(),
        "a recent temporary file may be another save's"
    );
}

// ---------------------------------------------------------------------------
// Migration and bounds
// ---------------------------------------------------------------------------

#[test]
fn a_version_1_file_is_migrated_and_written_back_as_the_current_version() {
    let dir = TempDir::new("migrate");
    let store = SettingsStore::new(dir.settings());
    std::fs::create_dir_all(store.path().parent().unwrap()).unwrap();
    let v1 = signed(&format!(
        "srtop-settings 1\napp srtop\nsource {SOURCE}\n\
         view Memory\ncolumns process.resident_memory,process.name\nsort process.resident_memory descending\nend-view -\n\
         view Plain\ncolumns process.pid\nsort none\nend-view -\n"
    ));
    let resident = ProcessField::Resident;
    assert!(v1.contains(settings::field_key(resident)), "{v1}");
    std::fs::write(store.path(), &v1).unwrap();
    let loaded = store.load(SOURCE);
    assert_eq!(
        loaded.origin,
        Origin::Primary {
            migrated_from: Some(1)
        }
    );
    let views = &loaded.settings.views;
    assert_eq!(views.len(), 2);
    assert_eq!(views[0].name.as_str(), "Memory");
    assert_eq!(views[0].columns.fields(), &[resident, ProcessField::Name]);
    assert_eq!(
        views[0].sort,
        Some(SortOrder {
            field: resident,
            direction: SortDirection::Descending
        })
    );
    // What version 1 did not record takes its default.
    assert_eq!(views[0].filter, "");
    assert_eq!(views[0].owner, SavedOwner::AllVisible);
    assert!(!views[0].tree);
    assert_eq!(views[0].threads, ThreadSettings::default());
    assert_eq!(loaded.settings.active, Some(0));
    // A restart shows it, and the next change writes version 2, keeping the
    // version 1 file as the backup.
    let mut explorer = Explorer::start(store.clone(), false);
    assert_eq!(
        presentation(&explorer).0,
        vec![resident, ProcessField::Name]
    );
    assert!(
        explorer.status().contains("version 1"),
        "{}",
        explorer.status()
    );
    assert_eq!(read(store.path()), v1.as_bytes(), "reading never writes");
    explorer.processed(SAVED_VIEW_SLOTS[1]);
    assert!(read(store.path()).starts_with(b"srtop-settings 2\n"));
    assert_eq!(read(&store.backup_path()), v1.as_bytes());
    assert_eq!(store.load(SOURCE).settings.active, Some(1));
}

#[test]
fn bounds_are_enforced_on_every_read_and_every_change() {
    let dir = TempDir::new("bounds");
    let store = SettingsStore::new(dir.settings());
    // One view too many, in an otherwise valid file.
    let mut body = format!("srtop-settings 2\napp srtop\nsource {SOURCE}\nactive 0\n");
    for index in 0..=MAX_VIEWS {
        body.push_str(&format!(
            "view V{index}\ncolumns process.pid\nsort none\nfilter \nowner all\ntree off\n\
             user-threads off\nkernel-threads on\nend-view -\n"
        ));
    }
    assert_eq!(
        settings::parse(signed(&body).as_bytes()),
        Err(Problem::Bound("views"))
    );
    // The same file with exactly the bound is read.
    let at_bound = body.split("view V8\n").next().unwrap().to_string();
    assert_eq!(
        settings::parse(signed(&at_bound).as_bytes())
            .unwrap()
            .settings
            .views
            .len(),
        MAX_VIEWS
    );
    // Names: the bound, and one byte past it.
    let long = "n".repeat(settings::MAX_VIEW_NAME_BYTES + 1);
    let overlong = at_bound.replace("view V0\n", &format!("view {long}\n"));
    assert_eq!(
        settings::parse(signed(&overlong).as_bytes()),
        Err(Problem::Bound("view name"))
    );
    // A filter past the filter's own bound.
    let filter = format!(
        "filter {}\n",
        "f".repeat(srui_process_explorer::filter::MAX_FILTER_QUERY_BYTES + 1)
    );
    let wide = at_bound.replacen("filter \n", &filter, 1);
    assert_eq!(
        settings::parse(signed(&wide).as_bytes()),
        Err(Problem::Bound("filter"))
    );

    // Through the controls: a ninth view and an overlong name are refused and
    // nothing is written.
    let full = settings::parse(signed(&at_bound).as_bytes())
        .unwrap()
        .settings;
    store.save(&full).unwrap();
    let mut explorer = Explorer::start(store.clone(), false);
    assert!(!explorer.enabled(SAVED_VIEW_CREATE), "no slot is free");
    let before = read(store.path());
    {
        let mut views = lock_saved_views(&explorer.saved);
        views.set_name_input(&explorer.session, "Ninth").unwrap();
        views
            .create(&explorer.session, &lock_view(&explorer.view))
            .unwrap();
        assert_eq!(views.settings().views.len(), MAX_VIEWS);
    }
    assert_eq!(read(store.path()), before);
    explorer.sync();
    assert!(
        explorer.status().contains("at most 8 views"),
        "{}",
        explorer.status()
    );
    explorer.type_name(&long);
    explorer.processed(SAVED_VIEW_RENAME);
    assert!(
        explorer.status().contains("at most 64 bytes"),
        "{}",
        explorer.status()
    );
    assert_eq!(read(store.path()), before);
    // An invisible character in a name is refused too.
    explorer.type_name("a\u{202e}b");
    explorer.processed(SAVED_VIEW_RENAME);
    assert_eq!(read(store.path()), before);
    // Duplicate names are refused.
    explorer.type_name("V3");
    explorer.processed(SAVED_VIEW_RENAME);
    assert!(
        explorer.status().contains("exists"),
        "{}",
        explorer.status()
    );
    assert_eq!(read(store.path()), before);
}

// ---------------------------------------------------------------------------
// Identity, actions and what is never saved
// ---------------------------------------------------------------------------

#[test]
fn renaming_or_deleting_a_view_changes_no_identity_selection_or_pending_action() {
    let dir = TempDir::new("identity");
    let store = SettingsStore::new(dir.settings());
    let mut settings = Settings::defaults(SOURCE);
    for name in ["Mine", "Other"] {
        settings.views.push(view(
            name,
            &[ProcessField::Name, ProcessField::Pid, ProcessField::Cpu],
        ));
    }
    settings.active = Some(2);
    store.save(&settings).unwrap();
    let mut explorer = Explorer::start(store.clone(), true);
    explorer.select(4103);
    explorer.processed(ACTION_PREPARE);
    let gate = explorer.gate.clone().unwrap();
    let panel = lock_actions(&gate)
        .pending_panel()
        .expect("an intent is pending");
    let items = explorer.item_ids();
    let selected = lock_view(&explorer.view).inspector().selected();
    assert!(selected.is_some());
    let presented = presentation(&explorer);

    let before = explorer.session.current_revision();
    explorer.type_name("Renamed");
    explorer.processed(SAVED_VIEW_RENAME);
    assert_eq!(
        explorer.label(SAVED_VIEW_SLOTS[2]),
        "View: Renamed (active)"
    );
    explorer.processed(SAVED_VIEW_DELETE);
    assert_eq!(explorer.settings().views.len(), 2);
    assert_eq!(
        explorer.settings().active,
        None,
        "no other view was made active"
    );
    assert!(
        explorer.status().contains("Deleted view “Renamed”"),
        "{}",
        explorer.status()
    );

    // Nothing but the saved views' own nodes changed.
    let touched: std::collections::BTreeSet<u64> = explorer
        .session
        .collect_replayed_transactions(before)
        .unwrap()
        .iter()
        .flat_map(|transaction| transaction.operations.iter())
        .filter_map(|operation| match &operation.op {
            Some(srui_protocol::operation::Op::SetProperty(set)) => Some(set.node_id),
            Some(srui_protocol::operation::Op::Commit(_)) => None,
            Some(other) => panic!("rename or delete planned {other:?}"),
            None => None,
        })
        .collect();
    let views_nodes = SAVED_VIEWS_ROW.get()..=SAVED_VIEWS_STATUS.get();
    assert!(
        touched.iter().all(|node| views_nodes.contains(node)),
        "rename or delete touched {touched:?}"
    );
    assert_eq!(explorer.item_ids(), items);
    assert_eq!(lock_view(&explorer.view).inspector().selected(), selected);
    assert_eq!(presentation(&explorer), presented);
    assert_eq!(
        lock_actions(&gate).pending_panel(),
        Some(panel),
        "the intent is still pending, neither confirmed nor withdrawn"
    );
    assert_eq!(
        *explorer.executor.reached.lock().unwrap(),
        0,
        "nothing was executed"
    );
    // And the settings file holds the rename and the delete.
    let saved = store.load(SOURCE).settings;
    assert_eq!(
        saved
            .views
            .iter()
            .map(|view| view.name.as_str())
            .collect::<Vec<_>>(),
        ["Default", "Mine"]
    );
    assert_eq!(saved.active, None);
    // A restart with no active view applies none.
    let restarted = Explorer::start(store, false);
    assert_eq!(
        presentation(&restarted).0,
        ColumnLayout::default().fields().to_vec()
    );
}

#[test]
fn no_intent_confirmation_selection_or_process_identity_is_ever_saved() {
    let dir = TempDir::new("never");
    let store = SettingsStore::new(dir.settings());
    let mut explorer = Explorer::start(store.clone(), true);
    explorer.select(4102);
    explorer.processed(ACTION_PREPARE);
    let gate = explorer.gate.clone().unwrap();
    assert!(lock_actions(&gate).pending_panel().is_some());
    explorer.type_name("Prepared");
    explorer.processed(SAVED_VIEW_CREATE);
    assert_eq!(explorer.settings().views.len(), 2, "{}", explorer.status());
    let text = String::from_utf8(read(store.path())).unwrap();
    let keys: std::collections::BTreeSet<&str> = text
        .lines()
        .map(|line| line.split(' ').next().unwrap())
        .collect();
    assert_eq!(
        keys,
        [
            "active",
            "app",
            "checksum",
            "columns",
            "end-view",
            "filter",
            "kernel-threads",
            "owner",
            "source",
            "sort",
            "srtop-settings",
            "tree",
            "user-threads",
            "view",
        ]
        .into_iter()
        .collect(),
        "{text}"
    );
    // The checksum's hex digits are not a value; every other line is.
    let values = text
        .lines()
        .filter(|line| !line.starts_with("checksum "))
        .collect::<Vec<_>>()
        .join("\n");
    for absent in [
        "4102",
        "daemon",
        "intent",
        "confirm",
        "terminate",
        "px069-boot",
    ] {
        assert!(
            !values.contains(absent),
            "the settings file holds {absent:?}:\n{text}"
        );
    }
    // The intent the user prepared is still pending, never executed.
    assert!(lock_actions(&gate).pending_panel().is_some());
    assert_eq!(*explorer.executor.reached.lock().unwrap(), 0);
}

#[test]
fn the_widest_saved_views_controls_fit_their_reserve() {
    let dir = TempDir::new("reserve");
    let store = SettingsStore::new(dir.settings());
    let mut widest = Settings::defaults(SOURCE);
    widest.views.clear();
    for index in 0..MAX_VIEWS {
        // Four-byte characters up to the name bound.
        let name = format!("{index}{}", "\u{20000}".repeat(15));
        assert!(name.len() <= settings::MAX_VIEW_NAME_BYTES);
        widest.views.push(view(&name, &[ProcessField::Pid]));
    }
    widest.active = Some(0);
    store.save(&widest).unwrap();
    let mut explorer = Explorer::start(store, false);
    explorer.type_name(
        &"\u{20000}".repeat(srui_process_explorer::saved_views::MAX_NAME_INPUT_BYTES / 4),
    );
    {
        let mut views = lock_saved_views(&explorer.saved);
        // The status at its bound.
        views
            .set_name_input(&explorer.session, &"x".repeat(4096))
            .unwrap();
        views.rename(&explorer.session).unwrap();
    }
    explorer
        .session
        .transaction(|ui| {
            ui.set(
                SAVED_VIEWS_STATUS,
                PropertyRef::TEXT,
                "\u{20000}".repeat(srui_process_explorer::saved_views::MAX_VIEWS_STATUS_BYTES / 4)
                    + "…",
            )?;
            Ok(())
        })
        .unwrap();
    let mut nodes = vec![
        SAVED_VIEWS_ROW,
        SAVED_VIEW_NAME_INPUT,
        SAVED_VIEW_CREATE,
        SAVED_VIEW_RENAME,
        SAVED_VIEW_DELETE,
        SAVED_VIEWS_RESTORE,
        SAVED_VIEWS_STATUS,
    ];
    nodes.extend(SAVED_VIEW_SLOTS);
    assert_eq!(nodes.len(), srui_process_explorer::SAVED_VIEWS_NODE_COUNT);
    let encoded: usize = explorer.session.with_store(|store| {
        nodes
            .iter()
            .map(|id| {
                let node = store.get_node(*id).unwrap();
                let operation = srui_sdk::Operation::create_node(
                    *id,
                    node.node_type,
                    node.parent_id,
                    None,
                    node.properties
                        .iter()
                        .map(|(property, value)| (*property, value.clone())),
                );
                srui_protocol::framed_payload_len(&srui_protocol::SruiMessage {
                    msg: Some(srui_protocol::srui_message::Msg::Transaction(
                        srui_protocol::Transaction {
                            base_revision: 0,
                            new_revision: 0,
                            priority: 0,
                            operations: vec![operation.to_wire()],
                        },
                    )),
                })
            })
            .sum()
    });
    let reserve = srui_process_explorer::refresh::SAVED_VIEWS_RESERVE_BYTES;
    assert!(
        encoded <= reserve,
        "the widest views controls encode to {encoded} bytes"
    );
    assert!(
        encoded > reserve / 2,
        "measured against controls that really are wide: {encoded}"
    );
    println!("PX-069 saved views controls at their widest: encoded={encoded} reserve={reserve}");
}
