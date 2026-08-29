//! Remote process monitor application logic (§5.2, §7.2, §7.3, §7.6, §7.7, §8, §12.1, §12.2, §22.7, §22.9, §23).
//!
//! # Architecture & Protocol Invariants
//!
//! - **§5.2 Semantic state, not display remoting**: the server owns process state and publishes
//!   only the semantic mutations required by an observed change. There is no frame, repaint, or
//!   pixel concept anywhere in this example.
//! - **§8 Collections**: the process list is a single collection [`Model`] with stable
//!   [`ItemId`]s, not one semantic node per process. Item identity is derived from a server-side
//!   [`ProcessKey`] (pid + start time) so PID reuse can never alias two different processes.
//! - **§12.1 Atomic transactions**: every polling sample and every accepted semantic event
//!   produces at most one all-or-nothing transaction; an empty operation list commits nothing.
//! - **§23 Incremental performance**: steady-state traffic is `SET_PROPERTY` on the two progress
//!   nodes plus `MODEL_INSERT` / `MODEL_UPDATE` / `MODEL_DELETE` for rows that actually changed.
//!   `CREATE_NODE`, `CREATE_MODEL`, and `MODEL_RESET_RANGE` never appear after initialization.
//! - **§7.7 `action_key` is data**: action keys are transported as opaque metadata and are never
//!   parsed, dispatched, or executed. Authorization uses server-owned state only.
//!
//! # Locking
//!
//! Two mutexes exist: application state and the process source. They are **never** held at the
//! same time, and neither is ever held across an `.await`. The session's internal transaction lock
//! is only ever taken *inside* the application-state lock (state → session), never the reverse,
//! and no `sysinfo` refresh or `kill(2)` call runs while a transaction is open.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use srui_protocol::Event as WireEvent;
use srui_sdk::*;
use srui_semantic_tree::{Event as SemanticEvent, ModelItem};
use srui_sessiond::{Session, SessionError};
use tracing::{debug, info, warn};

// =============================================================================
// Stable semantic identities (§6.2, §8)
// =============================================================================

/// Root surface node.
pub const SURFACE_ID: NodeId = NodeId::new(1);
/// Vertical root container.
pub const COLUMN_ID: NodeId = NodeId::new(2);
/// Row holding the heading and the two progress indicators.
pub const STATS_ROW_ID: NodeId = NodeId::new(3);
/// Heading text node.
pub const HEADING_ID: NodeId = NodeId::new(4);
/// CPU utilization progress node.
pub const CPU_PROGRESS_ID: NodeId = NodeId::new(5);
/// Memory utilization progress node.
pub const MEM_PROGRESS_ID: NodeId = NodeId::new(6);
/// "Show all processes" toggle node.
pub const SHOW_ALL_ID: NodeId = NodeId::new(7);
/// Process table node.
pub const PROCESS_TABLE_ID: NodeId = NodeId::new(8);
/// Row holding the destructive action button.
pub const ACTIONS_ROW_ID: NodeId = NodeId::new(9);
/// "Kill Selected" button node.
pub const KILL_BUTTON_ID: NodeId = NodeId::new(10);

/// Collection model backing the process table (§8).
pub const PROCESS_MODEL_ID: ModelId = ModelId::new(1);

/// Opaque action key advertised on the toggle (§7.7). Never parsed or executed.
pub const SHOW_ALL_ACTION_KEY: &str = "process.show-all";
/// Opaque action key advertised on the kill button (§7.7). Never parsed or executed.
pub const KILL_ACTION_KEY: &str = "process.kill-selected";

/// Table column titles, in row-value order.
pub const COLUMN_TITLES: [&str; 4] = ["PID", "Name", "CPU %", "Memory MiB"];

/// Maximum items packed into a single model operation (§26 keeps the hard ceiling at 10 000).
pub const MAX_ITEMS_PER_MODEL_OP: usize = 1_000;

fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

// =============================================================================
// Domain types
// =============================================================================

/// Stable server-side process identity (§8 stable item identity).
///
/// A PID alone is not an identity: operating systems reuse PIDs. The process start time
/// disambiguates a reused PID from the original process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProcessKey {
    /// Numeric process identifier.
    pub pid: u32,
    /// Process start time in seconds since the Unix epoch.
    pub start_time: u64,
}

impl ProcessKey {
    /// Constructs a process key.
    pub const fn new(pid: u32, start_time: u64) -> Self {
        Self { pid, start_time }
    }
}

/// One enumerated process as sampled from the operating system.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessRecord {
    /// Stable identity of this process.
    pub key: ProcessKey,
    /// Process name as reported by the OS.
    pub name: String,
    /// Instantaneous CPU utilization percentage (may exceed 100 on multi-core systems).
    pub cpu_percent: f64,
    /// Resident memory in bytes.
    pub memory_bytes: u64,
    /// Owning user id, when the platform reports one.
    pub uid: Option<u32>,
}

/// One sample of global and per-process system state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProcessSnapshot {
    /// Global CPU utilization percentage in `0.0..=100.0`.
    pub cpu_percent: f64,
    /// Used physical memory in bytes.
    pub memory_used: u64,
    /// Total physical memory in bytes.
    pub memory_total: u64,
    /// Every enumerated process, unfiltered.
    pub processes: Vec<ProcessRecord>,
}

/// Displayed semantic row values, in table column order.
#[derive(Debug, Clone, PartialEq)]
pub struct RowValues {
    /// Numeric PID.
    pub pid: u64,
    /// Process name.
    pub name: String,
    /// CPU utilization percentage, quantized for display stability.
    pub cpu_percent: f64,
    /// Resident memory in MiB.
    pub memory_mib: u64,
}

impl RowValues {
    /// Builds the ordered [`Value::List`] the AppKit table adapter renders as cells (§8).
    pub fn to_value(&self) -> Value {
        Value::List(vec![
            Value::UnsignedInt(self.pid),
            Value::String(self.name.clone()),
            Value::Float64(self.cpu_percent),
            Value::UnsignedInt(self.memory_mib),
        ])
    }

    fn from_record(record: &ProcessRecord) -> Self {
        Self {
            pid: u64::from(record.key.pid),
            name: record.name.clone(),
            cpu_percent: quantize(record.cpu_percent, 1),
            memory_mib: record.memory_bytes / (1024 * 1024),
        }
    }
}

/// A row currently published in the process model.
#[derive(Debug, Clone, PartialEq)]
pub struct VisibleRow {
    /// Stable collection item identity.
    pub item_id: ItemId,
    /// Authoritative process identity behind this row.
    pub key: ProcessKey,
    /// Displayed semantic values.
    pub values: RowValues,
}

impl VisibleRow {
    /// Converts this row into a wire-ready [`ModelItem`].
    pub fn to_model_item(&self) -> ModelItem {
        ModelItem::with_value(self.item_id, self.values.to_value())
    }
}

/// A determinate progress reading and its human-readable description (§7.4).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProgressReading {
    /// Normalized value in `0.0..=1.0`.
    pub value: f64,
    /// Human-readable description, e.g. `"37.2%"`.
    pub description: String,
}

/// Rounds `value` to `places` decimal places, suppressing meaningless float noise.
pub fn quantize(value: f64, places: u32) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    let factor = 10f64.powi(places as i32);
    (value * factor).round() / factor
}

fn cpu_reading(cpu_percent: f64) -> ProgressReading {
    let clamped = cpu_percent.clamp(0.0, 100.0);
    ProgressReading {
        value: quantize(clamped / 100.0, 3),
        description: format!("{:.1}%", quantize(clamped, 1)),
    }
}

fn memory_reading(used: u64, total: u64) -> ProgressReading {
    if total == 0 {
        return ProgressReading {
            value: 0.0,
            description: "unknown".to_string(),
        };
    }
    let gib = 1024.0 * 1024.0 * 1024.0;
    ProgressReading {
        value: quantize((used as f64 / total as f64).clamp(0.0, 1.0), 3),
        description: format!("{:.1} / {:.1} GiB", used as f64 / gib, total as f64 / gib),
    }
}

// =============================================================================
// Injectable external effects
// =============================================================================

/// Source of operating-system process samples.
pub trait ProcessSource: Send {
    /// Returns one full enumeration of system and process state.
    fn sample(&mut self) -> ProcessSnapshot;

    /// Re-reads the start time of `pid` right now, or `None` if no such process exists.
    fn start_time_of(&mut self, pid: u32) -> Option<u64>;
}

/// Failure modes of a termination request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminateError {
    /// The caller lacks permission to signal the target.
    PermissionDenied,
    /// The target process no longer exists.
    NoSuchProcess,
    /// Any other OS failure.
    Other(String),
}

impl std::fmt::Display for TerminateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PermissionDenied => write!(f, "permission denied"),
            Self::NoSuchProcess => write!(f, "no such process"),
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

/// Sends a termination signal to a numeric PID.
pub trait ProcessTerminator: Send + Sync {
    /// Sends `SIGTERM` to `pid`. Implementations must use a direct OS signal API: never a shell.
    fn terminate(&self, pid: u32) -> Result<(), TerminateError>;
}

/// Result of a "Kill Selected" activation, reported for logging and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KillOutcome {
    /// No row is selected in authoritative server state.
    NoSelection,
    /// The selected item id is unknown or stale.
    UnknownSelection(ItemId),
    /// The resolved PID is denylisted and was not signalled.
    Denied(u32),
    /// The live process at that PID has a different start time: identity is stale.
    StaleIdentity(u32),
    /// `SIGTERM` was delivered.
    Terminated(u32),
    /// The OS refused the signal.
    Failed(u32, TerminateError),
}

// =============================================================================
// Authoritative application state
// =============================================================================

/// Server-owned application state (§22.9 semantic inspection, §27 server authority).
#[derive(Debug, Clone)]
pub struct MonitorState {
    show_all: bool,
    effective_uid: Option<u32>,
    selected_item: Option<ItemId>,
    next_item_id: u64,
    key_to_item: HashMap<ProcessKey, ItemId>,
    item_to_key: HashMap<ItemId, ProcessKey>,
    visible: Vec<VisibleRow>,
    latest: Vec<ProcessRecord>,
    cpu: ProgressReading,
    mem: ProgressReading,
    denylist: HashSet<u32>,
}

/// State that a [`TickPlan`] will install once its operations commit.
#[derive(Debug, Clone)]
struct PendingState {
    show_all: bool,
    selected_item: Option<ItemId>,
    next_item_id: u64,
    key_to_item: HashMap<ProcessKey, ItemId>,
    item_to_key: HashMap<ItemId, ProcessKey>,
    visible: Vec<VisibleRow>,
    latest: Vec<ProcessRecord>,
    cpu: ProgressReading,
    mem: ProgressReading,
}

/// A computed, not-yet-committed semantic update (§12.1).
#[derive(Debug, Clone)]
pub struct TickPlan {
    operations: Vec<Operation>,
    next: PendingState,
}

impl TickPlan {
    /// Operations this plan would commit, in application order.
    pub fn operations(&self) -> &[Operation] {
        &self.operations
    }

    /// Returns `true` when nothing changed, in which case no transaction must be committed.
    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }

    /// Rows this plan would publish.
    pub fn visible(&self) -> &[VisibleRow] {
        &self.next.visible
    }
}

impl MonitorState {
    /// Constructs state for the given effective user, denylisting PID 1 and this process (§27).
    pub fn new(effective_uid: Option<u32>) -> Self {
        let mut denylist = HashSet::new();
        denylist.insert(1);
        denylist.insert(std::process::id());
        Self::with_denylist(effective_uid, denylist)
    }

    /// Constructs state with an explicit denylist (tests supply deterministic values).
    pub fn with_denylist(effective_uid: Option<u32>, denylist: HashSet<u32>) -> Self {
        Self {
            show_all: false,
            effective_uid,
            selected_item: None,
            next_item_id: 1,
            key_to_item: HashMap::new(),
            item_to_key: HashMap::new(),
            visible: Vec::new(),
            latest: Vec::new(),
            cpu: ProgressReading::default(),
            mem: ProgressReading::default(),
            denylist,
        }
    }

    /// Whether unfiltered enumeration is active.
    pub fn show_all(&self) -> bool {
        self.show_all
    }

    /// The currently selected item, if any.
    pub fn selected_item(&self) -> Option<ItemId> {
        self.selected_item
    }

    /// Rows currently published in the process model.
    pub fn visible(&self) -> &[VisibleRow] {
        &self.visible
    }

    /// Current CPU progress reading.
    pub fn cpu(&self) -> &ProgressReading {
        &self.cpu
    }

    /// Current memory progress reading.
    pub fn mem(&self) -> &ProgressReading {
        &self.mem
    }

    /// Denylisted PIDs that must never be signalled.
    pub fn denylist(&self) -> &HashSet<u32> {
        &self.denylist
    }

    /// Authoritative process identity for an item id.
    pub fn key_for_item(&self, item: ItemId) -> Option<ProcessKey> {
        self.item_to_key.get(&item).copied()
    }

    /// Installs the very first sample without producing operations (used before the initial
    /// transaction is built).
    pub fn seed(&mut self, snapshot: &ProcessSnapshot) {
        let plan = self.plan(
            snapshot.processes.clone(),
            self.show_all,
            snapshot_readings(snapshot),
            false,
        );
        self.commit(plan);
    }

    /// Plans the semantic update implied by a fresh sample (§12.1, §23).
    pub fn plan_snapshot(&self, snapshot: &ProcessSnapshot) -> TickPlan {
        self.plan(
            snapshot.processes.clone(),
            self.show_all,
            snapshot_readings(snapshot),
            false,
        )
    }

    /// Plans the semantic update implied by an authoritative `show_all` change (§27).
    ///
    /// Membership is recomputed from the server's latest enumeration; the toggle's authoritative
    /// `VALUE` is always confirmed back to the client in the same transaction.
    pub fn plan_visibility(&self, show_all: bool) -> TickPlan {
        self.plan(
            self.latest.clone(),
            show_all,
            (self.cpu.clone(), self.mem.clone()),
            true,
        )
    }

    fn is_visible(&self, record: &ProcessRecord, show_all: bool) -> bool {
        if show_all {
            return true;
        }
        match (self.effective_uid, record.uid) {
            (Some(effective), Some(owner)) => effective == owner,
            // Without a resolvable owner identity the server cannot claim ownership: fall back to
            // showing the process rather than silently hiding system state (§4 inv. 13).
            _ => true,
        }
    }

    fn plan(
        &self,
        mut latest: Vec<ProcessRecord>,
        show_all: bool,
        readings: (ProgressReading, ProgressReading),
        confirm_toggle: bool,
    ) -> TickPlan {
        let (cpu, mem) = readings;

        // Deterministic identity assignment: sort by the stable key, never by CPU (§23 - sorting
        // by utilization would reorder most rows on every tick).
        latest.sort_by_key(|record| record.key);

        let mut next_item_id = self.next_item_id;
        let mut key_to_item: HashMap<ProcessKey, ItemId> = HashMap::with_capacity(latest.len());
        let mut item_to_key: HashMap<ItemId, ProcessKey> = HashMap::with_capacity(latest.len());
        for record in &latest {
            // A key still present in the enumeration retains its item id, even while filtered out
            // of the visible model. A key that disappeared is dropped, so a later PID reuse with a
            // different start time necessarily receives a fresh item id.
            let item_id = match self.key_to_item.get(&record.key) {
                Some(&existing) => existing,
                None => {
                    let assigned = ItemId::new(next_item_id);
                    next_item_id += 1;
                    assigned
                }
            };
            key_to_item.insert(record.key, item_id);
            item_to_key.insert(item_id, record.key);
        }

        let visible: Vec<VisibleRow> = latest
            .iter()
            .filter(|record| self.is_visible(record, show_all))
            .map(|record| VisibleRow {
                item_id: key_to_item[&record.key],
                key: record.key,
                values: RowValues::from_record(record),
            })
            .collect();

        let mut operations = diff_visible_rows(PROCESS_MODEL_ID, &self.visible, &visible);

        if confirm_toggle {
            operations.push(Operation::set_property(
                SHOW_ALL_ID,
                VALUE,
                Value::Bool(show_all),
            ));
        }

        if cpu.value != self.cpu.value {
            operations.push(Operation::set_property(
                CPU_PROGRESS_ID,
                VALUE,
                Value::Float64(cpu.value),
            ));
        }
        if cpu.description != self.cpu.description {
            operations.push(Operation::set_property(
                CPU_PROGRESS_ID,
                VALUE_DESCRIPTION,
                Value::String(cpu.description.clone()),
            ));
        }
        if mem.value != self.mem.value {
            operations.push(Operation::set_property(
                MEM_PROGRESS_ID,
                VALUE,
                Value::Float64(mem.value),
            ));
        }
        if mem.description != self.mem.description {
            operations.push(Operation::set_property(
                MEM_PROGRESS_ID,
                VALUE_DESCRIPTION,
                Value::String(mem.description.clone()),
            ));
        }

        // A selection that left the visible model is no longer actionable (§27).
        let selected_item = self
            .selected_item
            .filter(|selected| visible.iter().any(|row| row.item_id == *selected));

        TickPlan {
            operations,
            next: PendingState {
                show_all,
                selected_item,
                next_item_id,
                key_to_item,
                item_to_key,
                visible,
                latest,
                cpu,
                mem,
            },
        }
    }

    /// Installs a plan's state after its operations committed.
    pub fn commit(&mut self, plan: TickPlan) {
        let PendingState {
            show_all,
            selected_item,
            next_item_id,
            key_to_item,
            item_to_key,
            visible,
            latest,
            cpu,
            mem,
        } = plan.next;
        self.show_all = show_all;
        self.selected_item = selected_item;
        self.next_item_id = next_item_id;
        self.key_to_item = key_to_item;
        self.item_to_key = item_to_key;
        self.visible = visible;
        self.latest = latest;
        self.cpu = cpu;
        self.mem = mem;
    }

    /// Records a client selection, accepting it only if the item exists in the visible model.
    pub fn select(&mut self, item: ItemId) -> bool {
        if self.visible.iter().any(|row| row.item_id == item) {
            self.selected_item = Some(item);
            true
        } else {
            false
        }
    }
}

fn snapshot_readings(snapshot: &ProcessSnapshot) -> (ProgressReading, ProgressReading) {
    (
        cpu_reading(snapshot.cpu_percent),
        memory_reading(snapshot.memory_used, snapshot.memory_total),
    )
}

// =============================================================================
// Minimal model diff (§8, §13, §23)
// =============================================================================

/// Computes the minimal `MODEL_*` operation list transforming `prev` into `next`.
///
/// Deletions are emitted first so insertion indices are the item's final index in `next`.
/// Retained items are only re-sent when their displayed values changed. A retained item that
/// somehow reordered relative to its peers is deleted and reinserted explicitly rather than left
/// in an inconsistent position.
pub fn diff_visible_rows(
    model: ModelId,
    prev: &[VisibleRow],
    next: &[VisibleRow],
) -> Vec<Operation> {
    let next_positions: HashMap<ItemId, usize> = next
        .iter()
        .enumerate()
        .map(|(index, row)| (row.item_id, index))
        .collect();

    let mut deleted: Vec<ItemId> = Vec::new();
    let mut retained: HashSet<ItemId> = HashSet::new();
    let mut highest_kept: Option<usize> = None;
    for row in prev {
        match next_positions.get(&row.item_id) {
            None => deleted.push(row.item_id),
            Some(&position) => {
                if highest_kept.is_some_and(|kept| position < kept) {
                    deleted.push(row.item_id);
                } else {
                    highest_kept = Some(position);
                    retained.insert(row.item_id);
                }
            }
        }
    }

    let mut operations = Vec::new();
    for chunk in deleted.chunks(MAX_ITEMS_PER_MODEL_OP) {
        operations.push(Operation::model_delete_items(model, chunk.iter().copied()));
    }

    let mut run_start: u64 = 0;
    let mut run: Vec<ModelItem> = Vec::new();
    for (index, row) in next.iter().enumerate() {
        if retained.contains(&row.item_id) {
            flush_insert(&mut operations, model, run_start, &mut run);
            continue;
        }
        if run.is_empty() {
            run_start = index as u64;
        }
        run.push(row.to_model_item());
        if run.len() == MAX_ITEMS_PER_MODEL_OP {
            flush_insert(&mut operations, model, run_start, &mut run);
        }
    }
    flush_insert(&mut operations, model, run_start, &mut run);

    let previous_values: HashMap<ItemId, &RowValues> =
        prev.iter().map(|row| (row.item_id, &row.values)).collect();
    let updates: Vec<ModelItem> = next
        .iter()
        .filter(|row| retained.contains(&row.item_id))
        .filter(|row| {
            previous_values
                .get(&row.item_id)
                .is_some_and(|old| **old != row.values)
        })
        .map(VisibleRow::to_model_item)
        .collect();
    for chunk in updates.chunks(MAX_ITEMS_PER_MODEL_OP) {
        operations.push(Operation::model_update(model, None, chunk.to_vec()));
    }

    operations
}

fn flush_insert(
    operations: &mut Vec<Operation>,
    model: ModelId,
    run_start: u64,
    run: &mut Vec<ModelItem>,
) {
    if run.is_empty() {
        return;
    }
    operations.push(Operation::model_insert(
        model,
        run_start,
        std::mem::take(run),
    ));
}

// =============================================================================
// Initial semantic graph (§7.2, §7.3)
// =============================================================================

/// Creates the model, its initial items, and the complete node tree in one transaction (§12.1).
pub fn build_initial_ui(session: &Session, state: &MonitorState) -> Result<(), SessionError> {
    let items: Vec<ModelItem> = state
        .visible
        .iter()
        .map(VisibleRow::to_model_item)
        .collect();
    let cpu = state.cpu.clone();
    let mem = state.mem.clone();
    let show_all = state.show_all;

    session.transaction(move |ui| {
        ui.apply_op(&Operation::create_model(
            PROCESS_MODEL_ID,
            TypeRef::TABLE,
            0,
        ))?;
        if !items.is_empty() {
            ui.apply_op(&Operation::model_insert(PROCESS_MODEL_ID, 0, items.clone()))?;
        }

        Surface::builder(SURFACE_ID)
            .label("System Monitor")
            .create(ui)?;

        Column::builder(COLUMN_ID)
            .parent(SURFACE_ID)
            .spacing_role(SpacingRole::Normal)
            .padding_role(PaddingRole::Normal)
            .grow(1.0)
            .create(ui)?;

        Row::builder(STATS_ROW_ID)
            .parent(COLUMN_ID)
            .spacing_role(SpacingRole::Normal)
            .create(ui)?;

        Text::builder(HEADING_ID)
            .parent(STATS_ROW_ID)
            .text("System Monitor")
            .role(TextRole::Heading)
            .create(ui)?;

        Progress::builder(CPU_PROGRESS_ID)
            .parent(STATS_ROW_ID)
            .label("CPU")
            .accessible_description("Global CPU utilization")
            .value(cpu.value)
            .value_description(cpu.description.clone())
            .grow(1.0)
            .create(ui)?;

        Progress::builder(MEM_PROGRESS_ID)
            .parent(STATS_ROW_ID)
            .label("Memory")
            .accessible_description("Physical memory in use")
            .value(mem.value)
            .value_description(mem.description.clone())
            .grow(1.0)
            .create(ui)?;

        Toggle::switch(SHOW_ALL_ID)
            .parent(COLUMN_ID)
            .label("Show all processes")
            .value(show_all)
            .action_key(SHOW_ALL_ACTION_KEY)
            .create(ui)?;

        Table::builder(PROCESS_TABLE_ID)
            .parent(COLUMN_ID)
            .model_ref(PROCESS_MODEL_ID)
            .columns(
                COLUMN_TITLES
                    .iter()
                    .map(|title| Value::String((*title).to_string())),
            )
            .selection_mode(SelectionMode::Single)
            .label("Running processes")
            .accessible_description("PID, name, CPU percentage and resident memory per process")
            .grow(1.0)
            .create(ui)?;

        Row::builder(ACTIONS_ROW_ID)
            .parent(COLUMN_ID)
            .spacing_role(SpacingRole::Normal)
            .create(ui)?;

        Button::builder(KILL_BUTTON_ID)
            .parent(ACTIONS_ROW_ID)
            .label("Kill Selected")
            .role(ActionRole::Destructive)
            .action_key(KILL_ACTION_KEY)
            .create(ui)?;

        Ok(())
    })?;

    Ok(())
}

// =============================================================================
// Monitor wiring
// =============================================================================

/// Live process monitor: authoritative state plus the session it publishes into.
pub struct Monitor {
    session: Arc<Session>,
    state: Mutex<MonitorState>,
    source: Mutex<Box<dyn ProcessSource>>,
    terminator: Box<dyn ProcessTerminator>,
}

impl std::fmt::Debug for Monitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Monitor")
            .field("session_id", &self.session.session_id())
            .finish_non_exhaustive()
    }
}

impl Monitor {
    /// Samples once, publishes the initial graph, and registers semantic event handlers.
    pub fn start(
        session: Arc<Session>,
        mut source: Box<dyn ProcessSource>,
        terminator: Box<dyn ProcessTerminator>,
        effective_uid: Option<u32>,
    ) -> Result<Arc<Self>, SessionError> {
        let snapshot = source.sample();
        let mut state = MonitorState::new(effective_uid);
        state.seed(&snapshot);
        build_initial_ui(&session, &state)?;

        let monitor = Arc::new(Self {
            session,
            state: Mutex::new(state),
            source: Mutex::new(source),
            terminator,
        });
        monitor.register_handlers();
        Ok(monitor)
    }

    /// The underlying session.
    pub fn session(&self) -> &Arc<Session> {
        &self.session
    }

    /// Runs `f` against authoritative application state (semantic inspection, §22.9).
    pub fn with_state<T>(&self, f: impl FnOnce(&MonitorState) -> T) -> T {
        f(&lock_or_recover(&self.state))
    }

    /// Samples the system and commits at most one transaction (§12.1). Returns the operation count.
    ///
    /// The sample is taken before the application-state lock is acquired, so no `sysinfo` refresh
    /// ever runs inside an open transaction.
    pub fn tick(&self) -> Result<usize, SessionError> {
        let snapshot = lock_or_recover(&self.source).sample();
        let mut state = lock_or_recover(&self.state);
        let plan = state.plan_snapshot(&snapshot);
        self.commit_plan(&mut state, plan)
    }

    fn commit_plan(&self, state: &mut MonitorState, plan: TickPlan) -> Result<usize, SessionError> {
        // An empty diff commits no transaction (§12.1), but authoritative state still advances:
        // a newly enumerated process that is currently filtered out must still be tracked so a
        // later `show_all` change can publish it.
        let count = plan.operations().len();
        if count > 0 {
            let operations = plan.operations().to_vec();
            self.session.transaction(move |ui| {
                for op in &operations {
                    ui.apply_op(op)?;
                }
                Ok(())
            })?;
        }
        state.commit(plan);
        Ok(count)
    }

    fn register_handlers(self: &Arc<Self>) {
        let selection_target = Arc::downgrade(self);
        self.session
            .on(PROCESS_TABLE_ID, SELECTION_CHANGED, move |_, event| {
                if let Some(monitor) = selection_target.upgrade() {
                    monitor.on_selection_changed(event);
                }
            });

        let toggle_target = Arc::downgrade(self);
        self.session
            .on(SHOW_ALL_ID, VALUE_CHANGED, move |_, event| {
                if let Some(monitor) = toggle_target.upgrade() {
                    monitor.on_show_all_changed(event);
                }
            });

        let kill_target: Weak<Self> = Arc::downgrade(self);
        self.session.on(KILL_BUTTON_ID, ACTIVATE, move |_, _| {
            if let Some(monitor) = kill_target.upgrade() {
                monitor.on_kill_activated();
            }
        });
    }

    /// Handles `SELECTION_CHANGED` on the process table (§7.6).
    ///
    /// Only the item id is trusted, and only if it currently exists in authoritative state. Row
    /// text, PID text, index, label, and `action_key` from the client are never consulted.
    pub fn on_selection_changed(&self, event: &WireEvent) {
        let Some(item) = decode_item_id(event) else {
            warn!("rejecting SELECTION_CHANGED without a usable item id argument");
            return;
        };
        let mut state = lock_or_recover(&self.state);
        if state.select(item) {
            debug!("selection accepted for item {}", item.get());
        } else {
            warn!(
                "rejecting SELECTION_CHANGED for unknown or no longer visible item {}",
                item.get()
            );
        }
    }

    /// Handles `VALUE_CHANGED` on the "Show all processes" toggle (§7.6, §27).
    pub fn on_show_all_changed(&self, event: &WireEvent) {
        let Some(show_all) = decode_bool(event) else {
            warn!("rejecting VALUE_CHANGED without a boolean value argument");
            return;
        };
        let mut state = lock_or_recover(&self.state);
        let plan = state.plan_visibility(show_all);
        match self.commit_plan(&mut state, plan) {
            Ok(count) => info!("show_all set to {show_all} ({count} operations)"),
            Err(error) => warn!("failed to apply show_all change: {error}"),
        }
    }

    /// Handles `ACTIVATE` on the "Kill Selected" button (§7.6, §27).
    pub fn on_kill_activated(&self) {
        match self.kill_selected() {
            KillOutcome::NoSelection => warn!("kill refused: no process is selected"),
            KillOutcome::UnknownSelection(item) => {
                warn!("kill refused: selected item {} is stale", item.get())
            }
            KillOutcome::Denied(pid) => warn!("kill refused: pid {pid} is denylisted"),
            KillOutcome::StaleIdentity(pid) => {
                warn!("kill refused: pid {pid} no longer matches the selected process identity")
            }
            KillOutcome::Terminated(pid) => info!("SIGTERM delivered to pid {pid}"),
            KillOutcome::Failed(pid, error) => warn!("kill of pid {pid} failed: {error}"),
        }
    }

    /// Resolves the selection through server-owned state and signals it, or refuses (§27).
    ///
    /// The PID is never read from client input: the selection is an [`ItemId`], resolved to a
    /// [`ProcessKey`] the server assigned, then revalidated against the live process start time
    /// immediately before signalling so a reused PID cannot be hit.
    pub fn kill_selected(&self) -> KillOutcome {
        let target = {
            let state = lock_or_recover(&self.state);
            let Some(selected) = state.selected_item else {
                return KillOutcome::NoSelection;
            };
            let Some(key) = state.key_for_item(selected) else {
                return KillOutcome::UnknownSelection(selected);
            };
            if state.denylist.contains(&key.pid) {
                return KillOutcome::Denied(key.pid);
            }
            key
        };

        // Revalidate identity with no application-state lock held and no transaction open.
        let live_start_time = lock_or_recover(&self.source).start_time_of(target.pid);
        if live_start_time != Some(target.start_time) {
            return KillOutcome::StaleIdentity(target.pid);
        }

        match self.terminator.terminate(target.pid) {
            Ok(()) => KillOutcome::Terminated(target.pid),
            Err(error) => KillOutcome::Failed(target.pid, error),
        }
    }
}

fn decode_semantic_event(event: &WireEvent) -> Option<SemanticEvent> {
    SemanticEvent::try_from(event.clone()).ok()
}

fn decode_item_id(event: &WireEvent) -> Option<ItemId> {
    let decoded = decode_semantic_event(event)?;
    match decoded.value_arg() {
        Some(Value::ItemId(item)) => Some(*item),
        Some(Value::UnsignedInt(raw)) => Some(ItemId::new(*raw)),
        _ => None,
    }
}

fn decode_bool(event: &WireEvent) -> Option<bool> {
    let decoded = decode_semantic_event(event)?;
    match decoded.value_arg() {
        Some(Value::Bool(value)) => Some(*value),
        _ => None,
    }
}

// =============================================================================
// Wire-traffic measurement (§19, §23)
// =============================================================================

/// Framed size and operation mix of one committed transaction.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TransactionWireStats {
    /// Revision this transaction advanced the session to.
    pub revision: u64,
    /// Total operation count.
    pub operations: usize,
    /// `SET_PROPERTY` operations.
    pub set_property: usize,
    /// `MODEL_INSERT` operations.
    pub model_insert: usize,
    /// `MODEL_UPDATE` operations.
    pub model_update: usize,
    /// `MODEL_DELETE` operations.
    pub model_delete: usize,
    /// Every other operation kind.
    pub other: usize,
    /// Length of the length-delimited SRUI frame, before SSH encryption (§26).
    pub framed_bytes: usize,
}

impl std::fmt::Display for TransactionWireStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "revision={} ops={} set_property={} model_insert={} model_update={} model_delete={} other={} framed_bytes={}",
            self.revision,
            self.operations,
            self.set_property,
            self.model_insert,
            self.model_update,
            self.model_delete,
            self.other,
            self.framed_bytes
        )
    }
}

/// Measures a committed transaction exactly as `handle_connection` would put it on the wire.
///
/// The transaction is wrapped in the same [`srui_protocol::SruiMessage`] envelope and encoded with
/// the canonical varint length-delimited framing helper, so the reported size is the semantic SRUI
/// frame the client would receive (§26). SSH transport encryption and compression are not included.
pub fn measure_transaction(transaction: &srui_protocol::Transaction) -> TransactionWireStats {
    use srui_protocol::operation::Op;

    let mut stats = TransactionWireStats {
        revision: transaction.new_revision,
        operations: transaction.operations.len(),
        ..TransactionWireStats::default()
    };
    for operation in &transaction.operations {
        match operation.op {
            Some(Op::SetProperty(_)) => stats.set_property += 1,
            Some(Op::ModelInsert(_)) => stats.model_insert += 1,
            Some(Op::ModelUpdate(_)) => stats.model_update += 1,
            Some(Op::ModelDelete(_)) => stats.model_delete += 1,
            _ => stats.other += 1,
        }
    }

    let envelope = srui_protocol::SruiMessage {
        msg: Some(srui_protocol::srui_message::Msg::Transaction(
            transaction.clone(),
        )),
    };
    stats.framed_bytes = srui_protocol::encode_framed(&envelope)
        .map(|bytes| bytes.len())
        .unwrap_or(0);
    stats
}

// =============================================================================
// Production external effects
// =============================================================================

/// `sysinfo`-backed process source.
pub struct SysinfoProcessSource {
    system: sysinfo::System,
}

impl SysinfoProcessSource {
    /// Creates the source and performs the first refresh.
    ///
    /// `sysinfo` derives CPU utilization from the delta between two refreshes, so the very first
    /// sample reports zero utilization by construction. Callers should wait at least
    /// [`sysinfo::MINIMUM_CPU_UPDATE_INTERVAL`] before the first meaningful sample; a zeroed first
    /// CPU reading is a warm-up state, not a failure.
    pub fn new() -> Self {
        let mut system = sysinfo::System::new();
        system.refresh_memory();
        system.refresh_cpu_usage();
        system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
        Self { system }
    }
}

impl Default for SysinfoProcessSource {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessSource for SysinfoProcessSource {
    fn sample(&mut self) -> ProcessSnapshot {
        self.system.refresh_memory();
        self.system.refresh_cpu_usage();
        self.system
            .refresh_processes(sysinfo::ProcessesToUpdate::All, true);

        let processes = self
            .system
            .processes()
            .values()
            .map(|process| ProcessRecord {
                key: ProcessKey::new(process.pid().as_u32(), process.start_time()),
                name: process.name().to_string_lossy().into_owned(),
                cpu_percent: f64::from(process.cpu_usage()),
                memory_bytes: process.memory(),
                uid: process.user_id().map(|uid| **uid),
            })
            .collect();

        ProcessSnapshot {
            cpu_percent: f64::from(self.system.global_cpu_usage()),
            memory_used: self.system.used_memory(),
            memory_total: self.system.total_memory(),
            processes,
        }
    }

    fn start_time_of(&mut self, pid: u32) -> Option<u64> {
        let pid = sysinfo::Pid::from_u32(pid);
        self.system
            .refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        self.system.process(pid).map(|process| process.start_time())
    }
}

/// `kill(2)`-backed terminator. Sends `SIGTERM` directly: never through a shell.
#[derive(Debug, Default, Clone, Copy)]
pub struct SignalTerminator;

impl ProcessTerminator for SignalTerminator {
    fn terminate(&self, pid: u32) -> Result<(), TerminateError> {
        use nix::errno::Errno;
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;

        match kill(Pid::from_raw(pid as i32), Signal::SIGTERM) {
            Ok(()) => Ok(()),
            Err(Errno::EPERM) => Err(TerminateError::PermissionDenied),
            Err(Errno::ESRCH) => Err(TerminateError::NoSuchProcess),
            Err(errno) => Err(TerminateError::Other(errno.to_string())),
        }
    }
}

/// Returns the effective user id of this process.
pub fn effective_uid() -> u32 {
    nix::unistd::geteuid().as_raw()
}

// =============================================================================
// Deterministic test doubles
// =============================================================================

/// Deterministic fakes for the injectable external effects.
///
/// Automated tests must never enumerate or signal arbitrary real host processes; every test in
/// this crate drives the monitor through these doubles.
pub mod testing {
    use super::*;

    #[derive(Debug, Default)]
    struct FakeSourceInner {
        snapshot: ProcessSnapshot,
        live_start_times: HashMap<u32, u64>,
        samples: usize,
    }

    /// In-memory [`ProcessSource`] whose snapshots the test controls.
    #[derive(Debug, Clone, Default)]
    pub struct FakeProcessSource {
        inner: Arc<Mutex<FakeSourceInner>>,
    }

    impl FakeProcessSource {
        /// Creates a source that will return `snapshot`.
        pub fn new(snapshot: ProcessSnapshot) -> Self {
            let source = Self::default();
            source.publish(snapshot);
            source
        }

        /// Replaces the snapshot returned by the next [`ProcessSource::sample`] call.
        ///
        /// Live start times used by kill revalidation are derived from the snapshot, so a process
        /// present in the snapshot revalidates successfully by default.
        pub fn publish(&self, snapshot: ProcessSnapshot) {
            let mut inner = lock_or_recover(&self.inner);
            inner.live_start_times = snapshot
                .processes
                .iter()
                .map(|record| (record.key.pid, record.key.start_time))
                .collect();
            inner.snapshot = snapshot;
        }

        /// Overrides what the OS would report as the live start time of `pid`.
        pub fn set_live_start_time(&self, pid: u32, start_time: Option<u64>) {
            let mut inner = lock_or_recover(&self.inner);
            match start_time {
                Some(value) => {
                    inner.live_start_times.insert(pid, value);
                }
                None => {
                    inner.live_start_times.remove(&pid);
                }
            }
        }

        /// Number of samples taken so far.
        pub fn samples(&self) -> usize {
            lock_or_recover(&self.inner).samples
        }

        /// Boxes a clone sharing the same controllable state.
        pub fn boxed(&self) -> Box<dyn ProcessSource> {
            Box::new(self.clone())
        }
    }

    impl ProcessSource for FakeProcessSource {
        fn sample(&mut self) -> ProcessSnapshot {
            let mut inner = lock_or_recover(&self.inner);
            inner.samples += 1;
            inner.snapshot.clone()
        }

        fn start_time_of(&mut self, pid: u32) -> Option<u64> {
            lock_or_recover(&self.inner)
                .live_start_times
                .get(&pid)
                .copied()
        }
    }

    #[derive(Debug)]
    struct FakeTerminatorInner {
        calls: Vec<u32>,
        result: Result<(), TerminateError>,
    }

    /// [`ProcessTerminator`] that records every PID it was asked to signal.
    #[derive(Debug, Clone)]
    pub struct RecordingTerminator {
        inner: Arc<Mutex<FakeTerminatorInner>>,
    }

    impl Default for RecordingTerminator {
        fn default() -> Self {
            Self {
                inner: Arc::new(Mutex::new(FakeTerminatorInner {
                    calls: Vec::new(),
                    result: Ok(()),
                })),
            }
        }
    }

    impl RecordingTerminator {
        /// Creates a terminator that reports `result` for every request.
        pub fn with_result(result: Result<(), TerminateError>) -> Self {
            let terminator = Self::default();
            lock_or_recover(&terminator.inner).result = result;
            terminator
        }

        /// PIDs signalled so far, in order.
        pub fn calls(&self) -> Vec<u32> {
            lock_or_recover(&self.inner).calls.clone()
        }

        /// Boxes a clone sharing the same recording state.
        pub fn boxed(&self) -> Box<dyn ProcessTerminator> {
            Box::new(self.clone())
        }
    }

    impl ProcessTerminator for RecordingTerminator {
        fn terminate(&self, pid: u32) -> Result<(), TerminateError> {
            let mut inner = lock_or_recover(&self.inner);
            inner.calls.push(pid);
            inner.result.clone()
        }
    }

    /// Builds a process record.
    pub fn record(
        pid: u32,
        start_time: u64,
        name: &str,
        cpu_percent: f64,
        memory_bytes: u64,
        uid: Option<u32>,
    ) -> ProcessRecord {
        ProcessRecord {
            key: ProcessKey::new(pid, start_time),
            name: name.to_string(),
            cpu_percent,
            memory_bytes,
            uid,
        }
    }

    /// Builds a snapshot with a fixed 8 GiB / 16 GiB memory reading.
    pub fn snapshot(cpu_percent: f64, processes: Vec<ProcessRecord>) -> ProcessSnapshot {
        ProcessSnapshot {
            cpu_percent,
            memory_used: 8 * 1024 * 1024 * 1024,
            memory_total: 16 * 1024 * 1024 * 1024,
            processes,
        }
    }

    /// Builds a wire `SELECTION_CHANGED` event carrying an item id.
    pub fn selection_event(event_seq: u64, observed_revision: u64, item: ItemId) -> WireEvent {
        wire(SemanticEvent::selection_changed(
            event_seq,
            format!("selection-{event_seq}"),
            observed_revision,
            PROCESS_TABLE_ID,
            item,
        ))
    }

    /// Builds a wire `VALUE_CHANGED` event for the toggle with an arbitrary argument value.
    pub fn toggle_event(event_seq: u64, observed_revision: u64, value: Value) -> WireEvent {
        wire(SemanticEvent::value_changed(
            event_seq,
            format!("toggle-{event_seq}"),
            observed_revision,
            SHOW_ALL_ID,
            value,
        ))
    }

    /// Builds a wire `ACTIVATE` event for `node`.
    pub fn activate_event(event_seq: u64, observed_revision: u64, node: NodeId) -> WireEvent {
        wire(SemanticEvent::activate(
            event_seq,
            format!("activate-{event_seq}"),
            observed_revision,
            node,
        ))
    }

    /// Builds a wire event with no arguments at all, for malformed-input coverage.
    pub fn argumentless_event(
        event_seq: u64,
        observed_revision: u64,
        node: NodeId,
        event_type: TypeRef,
    ) -> WireEvent {
        wire(SemanticEvent::new(
            None,
            event_seq,
            format!("bare-{event_seq}"),
            observed_revision,
            node,
            event_type,
            std::iter::empty(),
        ))
    }

    fn wire(event: SemanticEvent) -> WireEvent {
        WireEvent::from(&event.with_client_instance_id("process-monitor-test"))
    }
}
