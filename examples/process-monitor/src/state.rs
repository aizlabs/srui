//! Authoritative server application state and planning logic (§12.1, §22.9, §27).

use std::collections::{HashMap, HashSet};

use srui_sdk::*;

use crate::diff::diff_visible_rows;
use crate::domain::{
    cpu_reading, memory_reading, ProcessKey, ProcessRecord, ProcessSnapshot, ProgressReading,
    RowValues, VisibleRow, CPU_PROGRESS_ID, MEM_PROGRESS_ID, PROCESS_MODEL_ID, SHOW_ALL_ID,
};

/// Core mutable state payload shared between authoritative state and pending transaction plans.
#[derive(Debug, Clone, Default)]
pub struct StateData {
    pub show_all: bool,
    pub selected_item: Option<ItemId>,
    pub client_selections: HashMap<Vec<u8>, ItemId>,
    pub next_item_id: u64,
    pub key_to_item: HashMap<ProcessKey, ItemId>,
    pub item_to_key: HashMap<ItemId, ProcessKey>,
    pub visible: Vec<VisibleRow>,
    pub latest: Vec<ProcessRecord>,
    pub cpu: ProgressReading,
    pub mem: ProgressReading,
}

/// Server-owned application state (§22.9 semantic inspection, §27 server authority).
#[derive(Debug, Clone)]
pub struct MonitorState {
    effective_uid: u32,
    denylist: HashSet<u32>,
    data: StateData,
}

/// State that a [`TickPlan`] will install once its operations commit.
#[derive(Debug, Clone)]
pub struct PendingState {
    pub data: StateData,
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
        &self.next.data.visible
    }
}

impl MonitorState {
    /// Constructs state for the given effective user, denylisting PID 0, 1, and this process (§27).
    pub fn new(effective_uid: u32) -> Self {
        let mut denylist = HashSet::new();
        // PID 0 is not a process: `kill(2)` would read it as "my whole process group".
        denylist.insert(0);
        denylist.insert(1);
        denylist.insert(std::process::id());
        Self::with_denylist(effective_uid, denylist)
    }

    /// Constructs state with an explicit denylist (tests supply deterministic values).
    pub fn with_denylist(effective_uid: u32, denylist: HashSet<u32>) -> Self {
        Self {
            effective_uid,
            denylist,
            data: StateData {
                next_item_id: 1,
                ..Default::default()
            },
        }
    }

    /// Whether unfiltered enumeration is active.
    pub fn show_all(&self) -> bool {
        self.data.show_all
    }

    /// The currently selected item, if any.
    pub fn selected_item(&self) -> Option<ItemId> {
        self.data.selected_item
    }

    /// The currently selected item for a specific client instance, if any.
    pub fn selected_item_for_client(&self, client_instance_id: &[u8]) -> Option<ItemId> {
        self.data.client_selections.get(client_instance_id).copied()
    }

    /// Map of active client selections.
    pub fn client_selections(&self) -> &HashMap<Vec<u8>, ItemId> {
        &self.data.client_selections
    }

    /// Rows currently published in the process model.
    pub fn visible(&self) -> &[VisibleRow] {
        &self.data.visible
    }

    /// Latest unfiltered process records from the operating system.
    pub fn latest(&self) -> &[ProcessRecord] {
        &self.data.latest
    }

    /// Current CPU progress reading.
    pub fn cpu(&self) -> &ProgressReading {
        &self.data.cpu
    }

    /// Current memory progress reading.
    pub fn mem(&self) -> &ProgressReading {
        &self.data.mem
    }

    /// Set of numeric PIDs the server refuses to signal.
    pub fn denylist(&self) -> &HashSet<u32> {
        &self.denylist
    }

    /// Returns the server-assigned process key for `item`, if known.
    pub fn key_for_item(&self, item: ItemId) -> Option<ProcessKey> {
        self.data.item_to_key.get(&item).copied()
    }

    /// Predicate computing row membership for this state's configuration.
    pub fn is_visible(&self, record: &ProcessRecord, show_all: bool) -> bool {
        show_all || record.uid == Some(self.effective_uid)
    }

    /// Computes the initial transaction plan for a freshly populated snapshot.
    pub fn plan_initial(&mut self, snapshot: &ProcessSnapshot) -> TickPlan {
        let (cpu, mem) = snapshot_readings(snapshot);
        let mut plan = self.plan(
            snapshot.processes.clone(),
            cpu,
            mem,
            self.data.show_all,
            false,
        );
        self.commit(plan.clone());
        plan.operations.clear();
        plan
    }

    /// Seeds state with an initial snapshot without generating operations (§12.1).
    pub fn seed(&mut self, snapshot: &ProcessSnapshot) {
        let _ = self.plan_initial(snapshot);
    }

    /// Computes the incremental transaction plan for an incoming polling sample (§12.1).
    pub fn plan_sample(&self, snapshot: &ProcessSnapshot) -> TickPlan {
        let (cpu, mem) = snapshot_readings(snapshot);
        self.plan(
            snapshot.processes.clone(),
            cpu,
            mem,
            self.data.show_all,
            false,
        )
    }

    /// Alias for `plan_sample`.
    pub fn plan_snapshot(&self, snapshot: &ProcessSnapshot) -> TickPlan {
        self.plan_sample(snapshot)
    }

    /// Computes the transaction plan when the user toggles "Show all processes".
    pub fn plan_visibility(&self, show_all: bool) -> TickPlan {
        if show_all == self.data.show_all {
            return TickPlan {
                operations: Vec::new(),
                next: PendingState {
                    data: self.data.clone(),
                },
            };
        }
        self.plan(
            self.data.latest.clone(),
            self.data.cpu.clone(),
            self.data.mem.clone(),
            show_all,
            true,
        )
    }

    fn plan(
        &self,
        latest: Vec<ProcessRecord>,
        cpu: ProgressReading,
        mem: ProgressReading,
        show_all: bool,
        confirm_toggle: bool,
    ) -> TickPlan {
        let mut key_to_item = self.data.key_to_item.clone();
        let mut item_to_key = self.data.item_to_key.clone();
        let mut next_item_id = self.data.next_item_id;

        let live_keys: HashSet<ProcessKey> = latest.iter().map(|record| record.key).collect();
        key_to_item.retain(|key, _| live_keys.contains(key));
        item_to_key.retain(|_, key| live_keys.contains(key));

        for record in &latest {
            let item_id = match key_to_item.get(&record.key) {
                Some(&id) => id,
                None => {
                    let id = ItemId::new(next_item_id);
                    next_item_id += 1;
                    id
                }
            };
            key_to_item.insert(record.key, item_id);
            item_to_key.insert(item_id, record.key);
        }

        let mut visible: Vec<VisibleRow> = latest
            .iter()
            .filter(|record| self.is_visible(record, show_all))
            .map(|record| VisibleRow {
                item_id: key_to_item[&record.key],
                key: record.key,
                values: RowValues::from_record(record),
            })
            .collect();
        visible.sort_by_key(|row| row.key);

        let mut operations = diff_visible_rows(PROCESS_MODEL_ID, &self.data.visible, &visible);

        if confirm_toggle {
            operations.push(Operation::set_property(
                SHOW_ALL_ID,
                VALUE,
                Value::Bool(show_all),
            ));
        }

        push_property_if_changed(
            &mut operations,
            CPU_PROGRESS_ID,
            VALUE,
            &Value::Float64(self.data.cpu.value),
            Value::Float64(cpu.value),
        );
        push_property_if_changed(
            &mut operations,
            CPU_PROGRESS_ID,
            VALUE_DESCRIPTION,
            &Value::String(self.data.cpu.description.clone()),
            Value::String(cpu.description.clone()),
        );
        push_property_if_changed(
            &mut operations,
            MEM_PROGRESS_ID,
            VALUE,
            &Value::Float64(self.data.mem.value),
            Value::Float64(mem.value),
        );
        push_property_if_changed(
            &mut operations,
            MEM_PROGRESS_ID,
            VALUE_DESCRIPTION,
            &Value::String(self.data.mem.description.clone()),
            Value::String(mem.description.clone()),
        );

        // A selection that left the visible model is no longer actionable (§27).
        let selected_item = self
            .data
            .selected_item
            .filter(|selected| visible.iter().any(|row| row.item_id == *selected));
        let mut client_selections = self.data.client_selections.clone();
        client_selections.retain(|_, selected| visible.iter().any(|row| row.item_id == *selected));

        TickPlan {
            operations,
            next: PendingState {
                data: StateData {
                    show_all,
                    selected_item,
                    client_selections,
                    next_item_id,
                    key_to_item,
                    item_to_key,
                    visible,
                    latest,
                    cpu,
                    mem,
                },
            },
        }
    }

    /// Installs a plan's state after its operations committed.
    pub fn commit(&mut self, plan: TickPlan) {
        self.data = plan.next.data;
    }

    /// Records a client selection for a specific client instance, accepting it only if visible.
    pub fn select_for_client(&mut self, client_instance_id: &[u8], item: ItemId) -> bool {
        if self.data.visible.iter().any(|row| row.item_id == item) {
            self.data
                .client_selections
                .insert(client_instance_id.to_vec(), item);
            true
        } else {
            false
        }
    }

    /// Records a client selection, accepting it only if the item exists in the visible model.
    pub fn select(&mut self, item: ItemId) -> bool {
        if self.data.visible.iter().any(|row| row.item_id == item) {
            self.data.selected_item = Some(item);
            true
        } else {
            false
        }
    }
}

fn push_property_if_changed(
    operations: &mut Vec<Operation>,
    node: NodeId,
    property: PropertyRef,
    old: &Value,
    new: Value,
) {
    if *old != new {
        operations.push(Operation::set_property(node, property, new));
    }
}

fn snapshot_readings(snapshot: &ProcessSnapshot) -> (ProgressReading, ProgressReading) {
    (
        cpu_reading(snapshot.cpu_percent),
        memory_reading(snapshot.memory_used, snapshot.memory_total),
    )
}
