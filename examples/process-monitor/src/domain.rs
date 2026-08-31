//! Stable semantic identities and domain types for the process monitor (§6.2, §8).

use srui_sdk::*;
use srui_semantic_tree::ModelItem;

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

    pub fn from_record(record: &ProcessRecord) -> Self {
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

pub fn cpu_reading(cpu_percent: f64) -> ProgressReading {
    let clamped = cpu_percent.clamp(0.0, 100.0);
    ProgressReading {
        value: quantize(clamped / 100.0, 3),
        description: format!("{:.1}%", quantize(clamped, 1)),
    }
}

pub fn memory_reading(used: u64, total: u64) -> ProgressReading {
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
