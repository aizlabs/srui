//! Deterministic test doubles and event builder helpers (§19, §27).
//!
//! Automated tests must never enumerate or signal arbitrary real host processes; every test in
//! this crate drives the monitor through these doubles.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use srui_protocol::Event as WireEvent;
use srui_sdk::*;
use srui_semantic_tree::Event as SemanticEvent;

use crate::domain::{ProcessKey, ProcessRecord, ProcessSnapshot, PROCESS_TABLE_ID, SHOW_ALL_ID};
use crate::monitor::lock_or_recover;
use crate::source::ProcessSource;
use crate::terminator::{ProcessTerminator, TerminateError};

/// Client instance id stamped on every wire event built by this module.
///
/// Selections are owned by the client instance that made them (§27), so a test that drives the
/// monitor through these builders must resolve the selection under this id.
pub const TEST_CLIENT_INSTANCE_ID: &str = "process-monitor-test";

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
    WireEvent::from(&event.with_client_instance_id(TEST_CLIENT_INSTANCE_ID))
}
