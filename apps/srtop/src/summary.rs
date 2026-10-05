//! The system summary and data-freshness indicator (design §§6–8, 12, 22, 29;
//! PX-007).
//!
//! Above the process table the shell publishes the host's overall CPU, memory
//! and swap, its load averages, uptime and process count, and when — and from
//! which source — those figures were sampled. They are standard `Text` and
//! `Progress` nodes laid out in `Column` and `Row` nodes: no custom graph, no
//! image, no drawing instruction. Every figure becomes text here, on the server,
//! once; the client renders strings and bar fractions and decides nothing.
//!
//! Three rules shape it:
//!
//! * A figure the collector could not state is never a zero. Its line words the
//!   state instead — warming up, unavailable or denied and why, no swap
//!   configured, not sampled — and the bar beside it is hidden and carries no
//!   value, because an empty bar reads as 0%.
//! * Every figure comes from the last successful sample: the last scan whose
//!   process list was read and accepted. A scan that fails publishes no figure;
//!   the freshness line keeps the last successful sample's time, says
//!   "Collector error", and is set in the warning role. Transport loss is
//!   reported by the generic client's own connection indicator, and nothing here
//!   imitates it: a server cannot publish while it is disconnected.
//! * Nothing reaches the wire unless what a client sees changed. The published
//!   properties are kept per node and per property, and a refresh sets only the
//!   ones that differ, as scalar `SET_PROPERTY` operations inside its own
//!   transactions, which a slow client's delivery queue may coalesce (§12.1).
//!   The one exception is a bar that loses its value: its `VALUE` is cleared,
//!   never zeroed, and that `CLEAR_PROPERTY` — a rare state change — is
//!   delivered on its own revision.
use crate::metric::{
    self, format_cpu_tenths, format_hundredths, format_iec_bytes, format_uptime, format_utc,
    missing_text, share_tenths,
};
use crate::source::{
    FigureGap, LoadAverages, MemoryFigures, MissingReason, ProcessSnapshot, SnapshotTime, SourceId,
    SwapFigures, SystemCpu, SystemSample,
};
use crate::{bounded, ScanReport, COLUMN, MAX_SOURCE_STATUS_BYTES};
use srui_sdk::{
    EnumToken, NodeId, Operation, PropertyRef, SpacingRole, TextRole, TypeRef, Value, Visibility,
};
use std::collections::BTreeMap;

/// The summary's own column, between the status line and the process table.
pub const SUMMARY: NodeId = NodeId::new(6);
/// The overall CPU row: its bar, then its line.
pub const CPU_ROW: NodeId = NodeId::new(7);
pub const CPU_BAR: NodeId = NodeId::new(8);
pub const CPU_TEXT: NodeId = NodeId::new(9);
/// The memory row: its bar, then its line.
pub const MEMORY_ROW: NodeId = NodeId::new(10);
pub const MEMORY_BAR: NodeId = NodeId::new(11);
pub const MEMORY_TEXT: NodeId = NodeId::new(12);
/// The swap row: its bar, then its line.
pub const SWAP_ROW: NodeId = NodeId::new(13);
pub const SWAP_BAR: NodeId = NodeId::new(14);
pub const SWAP_TEXT: NodeId = NodeId::new(15);
pub const LOAD_TEXT: NodeId = NodeId::new(16);
pub const UPTIME_TEXT: NodeId = NodeId::new(17);
pub const PROCESSES_TEXT: NodeId = NodeId::new(18);
/// The data-freshness indicator: the last successful sample's time and source,
/// or the collector error that kept it from being replaced.
pub const FRESHNESS_TEXT: NodeId = NodeId::new(19);

/// Every summary node in creation order, with its type and its parent. Each
/// bar comes before its line in its row, so the bars line up whatever the lines
/// say.
pub const LAYOUT: [(NodeId, TypeRef, NodeId); 14] = [
    (SUMMARY, TypeRef::COLUMN, COLUMN),
    (CPU_ROW, TypeRef::ROW, SUMMARY),
    (CPU_BAR, TypeRef::PROGRESS, CPU_ROW),
    (CPU_TEXT, TypeRef::TEXT, CPU_ROW),
    (MEMORY_ROW, TypeRef::ROW, SUMMARY),
    (MEMORY_BAR, TypeRef::PROGRESS, MEMORY_ROW),
    (MEMORY_TEXT, TypeRef::TEXT, MEMORY_ROW),
    (SWAP_ROW, TypeRef::ROW, SUMMARY),
    (SWAP_BAR, TypeRef::PROGRESS, SWAP_ROW),
    (SWAP_TEXT, TypeRef::TEXT, SWAP_ROW),
    (LOAD_TEXT, TypeRef::TEXT, SUMMARY),
    (UPTIME_TEXT, TypeRef::TEXT, SUMMARY),
    (PROCESSES_TEXT, TypeRef::TEXT, SUMMARY),
    (FRESHNESS_TEXT, TypeRef::TEXT, SUMMARY),
];

/// How many nodes the summary adds to the shell.
pub const NODE_COUNT: usize = LAYOUT.len();

/// The longest text one summary line may publish, in encoded bytes.
///
/// Every line is fixed wording and numbers, plus, on the freshness line, a
/// source identity cut at [`MAX_SOURCE_STATUS_BYTES`]. The widest line the
/// builder can emit is asserted to fit unbounded, so this cut never shortens a
/// line in practice: it is what makes the summary's share of the catch-up
/// snapshot a constant (`crate::refresh`).
pub const MAX_SUMMARY_TEXT_BYTES: usize = 512;

/// The longest description one summary bar may publish, in encoded bytes.
pub const MAX_BAR_DESCRIPTION_BYTES: usize = 128;

/// The summary's published properties, per node and per property. A property
/// absent from the map is not set on its node.
pub type Properties = BTreeMap<(NodeId, PropertyRef), Value>;

/// How many processes one successful scan listed, in the status line's terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProcessCount {
    /// Records the scan listed and read.
    pub listed: usize,
    /// Records that existed and could not be read.
    pub unreadable: usize,
    /// Entries listed beyond the collector's own record bound and never read.
    pub capped: usize,
    /// Whether the scan was complete.
    pub complete: bool,
}

impl ProcessCount {
    /// What `snapshot` says about how many processes it listed.
    pub fn of(snapshot: &ProcessSnapshot) -> Self {
        Self {
            listed: snapshot.records.len(),
            unreadable: snapshot.completeness.skipped(),
            capped: snapshot.capped.count(),
            complete: snapshot.completeness.is_complete(),
        }
    }
}

/// Everything the summary shows from one successful sample, kept until the
/// next successful sample replaces it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sample {
    pub sampled_at: SnapshotTime,
    pub source: SourceId,
    pub system: SystemSample,
    pub processes: ProcessCount,
}

impl Sample {
    /// The sample `snapshot` holds, or the collector error that kept it from
    /// being one: a scan whose process list could not be read is not a sample of
    /// anything, whatever system file it did read.
    pub fn of(snapshot: &ProcessSnapshot) -> Result<Self, CollectorError> {
        if let Some(reason) = ScanReport::of(snapshot).root {
            return Err(CollectorError::ProcessList(reason));
        }
        Ok(Self {
            sampled_at: snapshot.sampled_at,
            source: snapshot.source.clone(),
            system: snapshot.system,
            processes: ProcessCount::of(snapshot),
        })
    }
}

/// Why the collector's latest attempt produced no successful sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectorError {
    /// The process list could not be listed.
    ProcessList(MissingReason),
    /// The snapshot was refused whole: its process identities were not distinct.
    Rejected,
}

/// What the summary describes.
#[derive(Debug, Clone, Copy)]
pub enum State<'a> {
    /// No source at all: the empty shell, which never samples.
    NotStarted,
    /// A source is being sampled.
    Collecting {
        /// The last successful sample, if there has been one.
        last: Option<&'a Sample>,
        /// Why the latest attempt was not a successful sample, if it was not.
        error: Option<CollectorError>,
        /// The source the latest attempt read.
        source: &'a SourceId,
    },
}

/// One summary line and the role it is set in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    /// `Body` for a figure or a state, `Warning` for a collector error, and
    /// `Status` for the freshness line while the collector is healthy.
    pub role: TextRole,
}

impl Line {
    fn new(text: String, role: TextRole) -> Self {
        Self { text, role }
    }
}

/// What one summary bar shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bar {
    /// The share the bar fills, in tenths of a percent — the same truncated
    /// number its line prints — or `None` when there is no value: the bar is then
    /// hidden, never an empty bar that reads as 0%.
    pub tenths: Option<u64>,
    /// What the bar says to assistive technology: its value, or its state.
    pub description: String,
}

impl Bar {
    fn value(tenths: u64, description: String) -> Self {
        Self {
            tenths: Some(tenths),
            description,
        }
    }

    fn hidden(description: &str) -> Self {
        Self {
            tenths: None,
            description: description.to_string(),
        }
    }

    /// The fraction the bar is filled to, in `0.0..=1.0`.
    pub fn fraction(&self) -> Option<f64> {
        self.tenths.map(|tenths| tenths as f64 / 1_000.0)
    }
}

/// Everything the summary shows, line by line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub cpu: Line,
    pub cpu_bar: Bar,
    pub memory: Line,
    pub memory_bar: Bar,
    pub swap: Line,
    pub swap_bar: Bar,
    pub load: Line,
    pub uptime: Line,
    pub processes: Line,
    pub freshness: Line,
}

/// Every line's state when there is no sample to show.
const NOT_SAMPLED: &str = "Not sampled";

/// The summary `state` describes.
pub fn summarize(state: &State<'_>) -> Summary {
    match *state {
        State::NotStarted => unsampled(Line::new(
            "No sample: process collection not started".to_string(),
            TextRole::Status,
        )),
        State::Collecting {
            last,
            error,
            source,
        } => {
            let freshness = freshness(last, error, source);
            match last {
                Some(sample) => sampled(sample, freshness),
                None => unsampled(freshness),
            }
        }
    }
}

/// The summary of a shell that has nothing to show yet.
fn unsampled(freshness: Line) -> Summary {
    let line = |label: &str| Line::new(format!("{label}: {NOT_SAMPLED}"), TextRole::Body);
    Summary {
        cpu: line(metric::SYSTEM_CPU.label),
        cpu_bar: Bar::hidden(NOT_SAMPLED),
        memory: line(metric::MEMORY_USED.label),
        memory_bar: Bar::hidden(NOT_SAMPLED),
        swap: line(metric::SWAP_USED.label),
        swap_bar: Bar::hidden(NOT_SAMPLED),
        load: line(metric::LOAD_AVERAGE.label),
        uptime: line(metric::UPTIME.label),
        processes: line(metric::PROCESS_COUNT.label),
        freshness,
    }
}

/// The summary of one successful sample.
fn sampled(sample: &Sample, freshness: Line) -> Summary {
    let (cpu, cpu_bar) = cpu(&sample.system.cpu);
    let (memory, memory_bar) = memory(&sample.system.memory);
    let (swap, swap_bar) = swap(&sample.system.swap);
    Summary {
        cpu,
        cpu_bar,
        memory,
        memory_bar,
        swap,
        swap_bar,
        load: load(&sample.system.load),
        uptime: uptime(&sample.system.uptime),
        processes: processes(&sample.processes),
        freshness,
    }
}

/// A figure this sample could not state, in words and never as a number, with
/// the role that marks a collector error. `file` is the file the figure comes
/// from, named relative to the source's root, and `needs` what it needs from it.
fn gap(gap: FigureGap, file: &str, needs: &str) -> (String, TextRole) {
    match gap {
        // Not an error: the source simply has no such figure.
        FigureGap::NotProvided => (
            format!(
                "{} (not provided by this source)",
                missing_text(MissingReason::Unavailable)
            ),
            TextRole::Body,
        ),
        FigureGap::Unread(MissingReason::Denied) => (
            format!(
                "{} (reading {file} was refused)",
                missing_text(MissingReason::Denied)
            ),
            TextRole::Warning,
        ),
        FigureGap::Unread(MissingReason::Unavailable) => (
            format!(
                "{} ({file} could not be read)",
                missing_text(MissingReason::Unavailable)
            ),
            TextRole::Warning,
        ),
        FigureGap::Unusable => (
            format!(
                "{} ({file} has no usable {needs})",
                missing_text(MissingReason::Unavailable)
            ),
            TextRole::Warning,
        ),
    }
}

/// The overall CPU line and bar. The denominator is named in every state, and
/// it is never the process column's "100% = 1 CPU".
fn cpu(cpu: &SystemCpu) -> (Line, Bar) {
    // "Overall CPU": the label's text before its parenthesis.
    const PREFIX: &str = "Overall CPU";
    let state = |text: String, role| {
        let description = text.clone();
        (
            Line::new(format!("{}: {text}", metric::SYSTEM_CPU.label), role),
            Bar {
                tenths: None,
                description,
            },
        )
    };
    match cpu {
        SystemCpu::Measured(interval) => match share_tenths(interval.busy, interval.total) {
            Some(tenths) => {
                let all = match interval.cpus {
                    Some(1) => "1 logical CPU".to_string(),
                    Some(count) if count > 1 => format!("all {count} logical CPUs"),
                    _ => "all logical CPUs".to_string(),
                };
                let share = format_cpu_tenths(tenths);
                (
                    Line::new(format!("{PREFIX} (100% = {all}): {share}"), TextRole::Body),
                    Bar::value(tenths, format!("{share} of {all}")),
                )
            }
            // More busy ticks than ticks, or no ticks at all: not an interval.
            None => state(no_interval(), TextRole::Body),
        },
        SystemCpu::WarmingUp => state(metric::CPU_WARMING_UP.to_string(), TextRole::Body),
        SystemCpu::Interrupted => state(no_interval(), TextRole::Body),
        SystemCpu::Missing(missing) => {
            let (text, role) = gap(*missing, "stat", "cpu line");
            state(text, role)
        }
    }
}

fn no_interval() -> String {
    format!(
        "{} (no measurable interval since the previous sample)",
        missing_text(MissingReason::Unavailable)
    )
}

/// `used` of `total` bytes as a line under `label` and a bar, or `None` where
/// that share is not defined.
fn usage(label: &str, used: u64, total: u64, what: &str) -> Option<(Line, Bar)> {
    let tenths = share_tenths(used, total)?;
    let share = format_cpu_tenths(tenths);
    Some((
        Line::new(
            format!(
                "{label}: {} used of {} ({share} of total)",
                format_iec_bytes(used),
                format_iec_bytes(total)
            ),
            TextRole::Body,
        ),
        Bar::value(tenths, format!("{share} of total {what} used")),
    ))
}

/// A figure-shaped line that is a state rather than a figure: its bar is
/// hidden and says the same.
fn stated(label: &str, (text, role): (String, TextRole)) -> (Line, Bar) {
    let bar = Bar::hidden(&text);
    (Line::new(format!("{label}: {text}"), role), bar)
}

fn memory(memory: &Result<MemoryFigures, FigureGap>) -> (Line, Bar) {
    let label = metric::MEMORY_USED.label;
    let missing = |why| stated(label, gap(why, "meminfo", "MemTotal and MemAvailable"));
    match memory {
        // A MemAvailable above MemTotal, or a MemTotal of 0, is not memory.
        Ok(figures) => figures
            .total
            .checked_sub(figures.available)
            .and_then(|used| usage(label, used, figures.total, "memory"))
            .unwrap_or_else(|| missing(FigureGap::Unusable)),
        Err(why) => missing(*why),
    }
}

fn swap(swap: &Result<SwapFigures, FigureGap>) -> (Line, Bar) {
    let label = metric::SWAP_USED.label;
    let missing = |why| stated(label, gap(why, "meminfo", "SwapTotal and SwapFree"));
    match swap {
        // A host with no swap space says so: never 0% of 0, never a division.
        Ok(SwapFigures { total: 0, free: 0 }) => (
            Line::new(format!("{label}: none configured"), TextRole::Body),
            Bar::hidden("No swap configured"),
        ),
        // A SwapFree above SwapTotal, or free swap with no total, is not swap.
        Ok(figures) => figures
            .total
            .checked_sub(figures.free)
            .and_then(|used| usage(label, used, figures.total, "swap"))
            .unwrap_or_else(|| missing(FigureGap::Unusable)),
        Err(why) => missing(*why),
    }
}

fn load(load: &Result<LoadAverages, FigureGap>) -> Line {
    let label = metric::LOAD_AVERAGE.label;
    match load {
        Ok(averages) => Line::new(
            format!(
                "{label}: {}, {}, {}",
                format_hundredths(averages.one),
                format_hundredths(averages.five),
                format_hundredths(averages.fifteen)
            ),
            TextRole::Body,
        ),
        Err(missing) => {
            let (text, role) = gap(*missing, "loadavg", "load averages");
            Line::new(format!("{label}: {text}"), role)
        }
    }
}

fn uptime(uptime: &Result<u64, FigureGap>) -> Line {
    let label = metric::UPTIME.label;
    match uptime {
        Ok(seconds) => Line::new(
            format!("{label}: {}", format_uptime(*seconds)),
            TextRole::Body,
        ),
        Err(missing) => {
            let (text, role) = gap(*missing, "uptime", "uptime");
            Line::new(format!("{label}: {text}"), role)
        }
    }
}

/// The process count, in the status line's own counts and words, and scoped to
/// what srtop can state: the processes this reader can see in the scanned root,
/// which the PID namespace the mount was made for and the mount's visibility
/// options (`hidepid`) bound without telling it, and no filter of srtop's own. A complete scan is
/// complete over that view, never a claim about the whole host.
fn processes(count: &ProcessCount) -> Line {
    let mut text = format!("{}: {} listed", metric::PROCESS_COUNT.label, count.listed);
    if count.unreadable > 0 {
        text.push_str(&format!(" · {} unreadable", count.unreadable));
    }
    if count.capped > 0 {
        text.push_str(&format!(" · {} beyond the record limit", count.capped));
    }
    text.push_str(if count.complete {
        " · complete scan"
    } else {
        " · incomplete scan"
    });
    text.push_str(" · no srtop filter");
    Line::new(text, TextRole::Body)
}

/// The data-freshness line: when the figures on screen were sampled and from
/// which source, and, while the collector is failing, that it is.
fn freshness(last: Option<&Sample>, error: Option<CollectorError>, source: &SourceId) -> Line {
    let named = |source: &SourceId| bounded(&source.0, MAX_SOURCE_STATUS_BYTES);
    let sampled = |sample: &Sample| {
        format!(
            "{}: {} · source: {}",
            metric::SAMPLE_TIME.label.to_lowercase(),
            sample_time(sample.sampled_at),
            named(&sample.source)
        )
    };
    match (error, last) {
        (None, Some(sample)) => Line::new(capitalized(&sampled(sample)), TextRole::Status),
        (None, None) => Line::new(
            format!("No successful sample yet · source: {}", named(source)),
            TextRole::Status,
        ),
        (Some(error), Some(sample)) => Line::new(
            format!(
                "Collector error: {} · {}",
                collector_error(error),
                sampled(sample)
            ),
            TextRole::Warning,
        ),
        (Some(error), None) => Line::new(
            format!(
                "Collector error: {} · no successful sample yet · source: {}",
                collector_error(error),
                named(source)
            ),
            TextRole::Warning,
        ),
    }
}

fn capitalized(text: &str) -> String {
    let mut characters = text.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// A sample time in UTC on the server's own clock, which the line says,
/// because a time without its clock and zone is not a time.
fn sample_time(time: SnapshotTime) -> String {
    match format_utc(time.0) {
        Some(utc) => format!("{utc} (server clock)"),
        None => "before 1970-01-01 00:00:00 UTC (server clock)".to_string(),
    }
}

fn collector_error(error: CollectorError) -> String {
    match error {
        CollectorError::ProcessList(reason) => {
            format!("could not list processes ({})", reason.describe())
        }
        CollectorError::Rejected => "snapshot rejected".to_string(),
    }
}

/// The properties that publish `summary`: each line's text and role, and each
/// bar's visibility, value and description. A bar without a value is hidden
/// and carries no `VALUE` at all.
pub fn properties(summary: &Summary) -> Properties {
    let role = |role: TextRole| Value::from(EnumToken::from(role));
    let mut properties = Properties::new();
    for (id, line) in [
        (CPU_TEXT, &summary.cpu),
        (MEMORY_TEXT, &summary.memory),
        (SWAP_TEXT, &summary.swap),
        (LOAD_TEXT, &summary.load),
        (UPTIME_TEXT, &summary.uptime),
        (PROCESSES_TEXT, &summary.processes),
        (FRESHNESS_TEXT, &summary.freshness),
    ] {
        properties.insert(
            (id, PropertyRef::TEXT),
            Value::String(bounded(&line.text, MAX_SUMMARY_TEXT_BYTES)),
        );
        properties.insert((id, PropertyRef::ROLE), role(line.role));
    }
    for (id, bar) in [
        (CPU_BAR, &summary.cpu_bar),
        (MEMORY_BAR, &summary.memory_bar),
        (SWAP_BAR, &summary.swap_bar),
    ] {
        let visibility = match bar.fraction() {
            Some(fraction) => {
                properties.insert((id, PropertyRef::VALUE), Value::Float64(fraction));
                Visibility::Visible
            }
            None => Visibility::Hidden,
        };
        properties.insert(
            (id, PropertyRef::VISIBILITY),
            Value::from(EnumToken::from(visibility)),
        );
        properties.insert(
            (id, PropertyRef::VALUE_DESCRIPTION),
            Value::String(bounded(&bar.description, MAX_BAR_DESCRIPTION_BYTES)),
        );
    }
    properties
}

/// The properties a summary node is created with and keeps for the life of the
/// shell: the summary's accessibility label and spacing, each row's spacing, and
/// each bar's label.
fn fixed(id: NodeId) -> Vec<(PropertyRef, Value)> {
    let spacing = |role: SpacingRole| {
        (
            PropertyRef::SPACING_ROLE,
            Value::from(EnumToken::from(role)),
        )
    };
    let label = |text: &str| (PropertyRef::LABEL, Value::from(text));
    match id {
        SUMMARY => vec![label("System summary"), spacing(SpacingRole::Tight)],
        CPU_ROW | MEMORY_ROW | SWAP_ROW => vec![spacing(SpacingRole::Normal)],
        CPU_BAR => vec![label("Overall CPU")],
        MEMORY_BAR => vec![label("Memory used")],
        SWAP_BAR => vec![label("Swap used")],
        _ => Vec::new(),
    }
}

/// The operations that create the summary under the shell's column, each node
/// carrying its fixed properties and its share of `properties` — the same
/// single `CREATE_NODE` per node a catch-up snapshot carries (§13, §18).
pub fn create_operations(properties: &Properties) -> Vec<Operation> {
    LAYOUT
        .iter()
        .map(|&(id, node_type, parent)| {
            let mut node = fixed(id);
            node.extend(
                properties
                    .iter()
                    .filter(|((owner, _), _)| *owner == id)
                    .map(|((_, property), value)| (*property, value.clone())),
            );
            Operation::create_node(id, node_type, Some(parent), None, node)
        })
        .collect()
}

/// One property a refresh sets, or clears where `value` is `None`.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub node: NodeId,
    pub property: PropertyRef,
    pub value: Option<Value>,
}

/// The properties that turn `published` into `target`: every one that differs
/// is set, every one `target` no longer has is cleared, and nothing else is
/// touched, in node and property order.
pub fn changes(published: &Properties, target: &Properties) -> Vec<Change> {
    let mut changes: Vec<Change> = target
        .iter()
        .filter(|(key, value)| published.get(key) != Some(value))
        .map(|(&(node, property), value)| Change {
            node,
            property,
            value: Some(value.clone()),
        })
        .chain(
            published
                .keys()
                .filter(|key| !target.contains_key(key))
                .map(|&(node, property)| Change {
                    node,
                    property,
                    value: None,
                }),
        )
        .collect();
    changes.sort_by_key(|change| (change.node, change.property));
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{FakeProcessSource, ScriptedFakeSource, SystemCpuInterval};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    /// 2027-01-15 08:00:00 UTC, the fake sources' own sample time.
    fn fixture_time() -> SnapshotTime {
        SnapshotTime(UNIX_EPOCH + Duration::from_secs(1_800_000_000))
    }

    fn sample(system: SystemSample) -> Sample {
        Sample {
            sampled_at: fixture_time(),
            source: SourceId("fixture-source".into()),
            system,
            processes: ProcessCount {
                listed: 3,
                unreadable: 0,
                capped: 0,
                complete: true,
            },
        }
    }

    fn healthy(sample: &Sample) -> Summary {
        summarize(&State::Collecting {
            last: Some(sample),
            error: None,
            source: &sample.source,
        })
    }

    /// The text after a line's label: the figure, or the state that replaces it.
    fn value(line: &Line) -> &str {
        line.text
            .split_once(": ")
            .map_or(line.text.as_str(), |(_, value)| value)
    }

    #[test]
    fn the_fake_source_states_every_figure_with_its_denominator() {
        let summary = healthy(&sample(FakeProcessSource::system()));
        assert_eq!(
            summary.cpu.text,
            "Overall CPU (100% = all 8 logical CPUs): 31.2%"
        );
        assert_eq!(
            summary.memory.text,
            "Memory: 4.0 GiB used of 16.0 GiB (25.0% of total)"
        );
        assert_eq!(summary.swap.text, "Swap: none configured");
        assert_eq!(
            summary.load.text,
            "Load average (1, 5, 15 min): 0.52, 0.58, 0.59"
        );
        assert_eq!(summary.uptime.text, "Uptime: 3 days, 4 h 05 min");
        assert_eq!(
            summary.processes.text,
            "Processes visible to this reader: 3 listed · complete scan · no srtop filter"
        );
        assert_eq!(
            summary.freshness.text,
            "Last successful sample: 2027-01-15 08:00:00 UTC (server clock) · source: \
             fixture-source"
        );
        assert_eq!(summary.freshness.role, TextRole::Status);
        // 250 of 800 ticks is 31.25%: truncated, never rounded up.
        assert_eq!(summary.cpu_bar.tenths, Some(312));
        assert_eq!(summary.cpu_bar.description, "31.2% of all 8 logical CPUs");
        assert_eq!(summary.memory_bar.tenths, Some(250));
        assert_eq!(summary.memory_bar.description, "25.0% of total memory used");
        for line in [
            &summary.cpu,
            &summary.memory,
            &summary.swap,
            &summary.load,
            &summary.uptime,
            &summary.processes,
        ] {
            assert_eq!(line.role, TextRole::Body, "{}", line.text);
        }

        let scripted = healthy(&sample(ScriptedFakeSource::system()));
        assert_eq!(
            scripted.cpu.text,
            "Overall CPU (100% = all 4 logical CPUs): 30.0%"
        );
        assert_eq!(
            scripted.swap.text,
            "Swap: 256.0 MiB used of 2.0 GiB (12.5% of total)"
        );
        assert_eq!(scripted.swap_bar.tenths, Some(125));
        assert_eq!(scripted.uptime.text, "Uptime: 1 day, 2 h 30 min");
    }

    /// Totals, used amounts and percentages agree with each other: used is the
    /// total less what is available or free, the share is used over the total,
    /// and the bar fills exactly the truncated share its line prints.
    #[test]
    fn totals_and_percentages_are_consistent_and_truncated() {
        let mut system = FakeProcessSource::system();
        system.memory = Ok(MemoryFigures {
            total: 3 << 30,
            available: 1 << 30,
        });
        system.swap = Ok(SwapFigures {
            total: 3 << 30,
            free: 2 << 30,
        });
        let summary = healthy(&sample(system));
        // Two thirds is 66.66…%, published as 66.6%; a third as 33.3%.
        assert_eq!(
            summary.memory.text,
            "Memory: 2.0 GiB used of 3.0 GiB (66.6% of total)"
        );
        assert_eq!(summary.memory_bar.tenths, Some(666));
        assert_eq!(summary.memory_bar.fraction(), Some(0.666));
        assert_eq!(
            summary.swap.text,
            "Swap: 1.0 GiB used of 3.0 GiB (33.3% of total)"
        );
        assert_eq!(summary.swap_bar.tenths, Some(333));
        // Everything used, and nothing used.
        system.memory = Ok(MemoryFigures {
            total: 4096,
            available: 0,
        });
        system.swap = Ok(SwapFigures {
            total: 4096,
            free: 4096,
        });
        let summary = healthy(&sample(system));
        assert_eq!(
            summary.memory.text,
            "Memory: 4.0 KiB used of 4.0 KiB (100.0% of total)"
        );
        assert_eq!(summary.memory_bar.fraction(), Some(1.0));
        assert_eq!(
            summary.swap.text,
            "Swap: 0 B used of 4.0 KiB (0.0% of total)"
        );
        assert_eq!(
            summary.swap_bar.tenths,
            Some(0),
            "a measured zero is a value, and its bar is shown empty"
        );
    }

    #[test]
    fn a_host_without_swap_says_so_and_never_publishes_zero_percent_of_zero() {
        let summary = healthy(&sample(FakeProcessSource::system()));
        assert_eq!(summary.swap.text, "Swap: none configured");
        assert_eq!(summary.swap.role, TextRole::Body);
        assert_eq!(summary.swap_bar.tenths, None);
        assert_eq!(summary.swap_bar.description, "No swap configured");
        assert!(!summary.swap.text.contains('%'));
        assert!(!summary.swap.text.chars().any(|c| c.is_ascii_digit()));
        let published = properties(&summary);
        assert_eq!(
            published.get(&(SWAP_BAR, PropertyRef::VISIBILITY)),
            Some(&Value::from(EnumToken::from(Visibility::Hidden)))
        );
        assert!(
            !published.contains_key(&(SWAP_BAR, PropertyRef::VALUE)),
            "a host with no swap has no swap bar value, not an empty one"
        );
        // Free swap with no total is not a host without swap: it is unusable.
        let mut system = FakeProcessSource::system();
        system.swap = Ok(SwapFigures {
            total: 0,
            free: 4096,
        });
        let summary = healthy(&sample(system));
        assert_eq!(
            summary.swap.text,
            "Swap: Unavailable (meminfo has no usable SwapTotal and SwapFree)"
        );
        assert_eq!(summary.swap.role, TextRole::Warning);
    }

    #[test]
    fn overall_cpu_names_all_logical_cpus_and_never_the_one_cpu_convention() {
        let measured = |cpus| {
            let mut system = FakeProcessSource::system();
            system.cpu = SystemCpu::Measured(SystemCpuInterval {
                busy: 1,
                total: 4,
                cpus,
            });
            healthy(&sample(system)).cpu.text
        };
        assert_eq!(
            measured(Some(12)),
            "Overall CPU (100% = all 12 logical CPUs): 25.0%"
        );
        assert_eq!(
            measured(Some(1)),
            "Overall CPU (100% = 1 logical CPU): 25.0%"
        );
        assert_eq!(
            measured(None),
            "Overall CPU (100% = all logical CPUs): 25.0%"
        );
        assert_eq!(
            measured(Some(0)),
            "Overall CPU (100% = all logical CPUs): 25.0%",
            "a count of none is no count"
        );
        // The process column's convention is a different label entirely.
        assert_eq!(metric::CPU_USAGE.label, "CPU (100% = 1 CPU)");
        for cpus in [Some(12), Some(1), None] {
            assert!(!measured(cpus).contains(metric::CPU_USAGE.label));
            assert!(measured(cpus).starts_with("Overall CPU (100% = "));
        }
    }

    /// Unavailable, denied, warming-up and interrupted figures are words, never
    /// numbers, and their bars are hidden with no value at all.
    #[test]
    fn no_figure_the_collector_could_not_state_is_published_as_a_number() {
        let gaps = [
            (FigureGap::NotProvided, TextRole::Body),
            (FigureGap::Unread(MissingReason::Denied), TextRole::Warning),
            (
                FigureGap::Unread(MissingReason::Unavailable),
                TextRole::Warning,
            ),
            (FigureGap::Unusable, TextRole::Warning),
        ];
        let mut cases: Vec<(SystemSample, TextRole)> = gaps
            .iter()
            .map(|&(gap, role)| (SystemSample::missing(gap), role))
            .collect();
        for cpu in [
            SystemCpu::WarmingUp,
            SystemCpu::Interrupted,
            // More busy ticks than ticks, and no ticks at all: no interval.
            SystemCpu::Measured(SystemCpuInterval {
                busy: 5,
                total: 4,
                cpus: Some(2),
            }),
            SystemCpu::Measured(SystemCpuInterval {
                busy: 0,
                total: 0,
                cpus: Some(2),
            }),
        ] {
            let mut system = SystemSample::missing(FigureGap::NotProvided);
            system.cpu = cpu;
            cases.push((system, TextRole::Body));
        }
        for (system, role) in cases {
            let summary = healthy(&sample(system));
            for line in [
                &summary.cpu,
                &summary.memory,
                &summary.swap,
                &summary.load,
                &summary.uptime,
            ] {
                assert!(
                    !value(line).chars().any(|c| c.is_ascii_digit()),
                    "{system:?}: {}",
                    line.text
                );
                assert!(!value(line).contains('%'), "{}", line.text);
            }
            for line in [
                &summary.memory,
                &summary.swap,
                &summary.load,
                &summary.uptime,
            ] {
                assert_eq!(line.role, role, "{}", line.text);
            }
            let published = properties(&summary);
            for bar in [CPU_BAR, MEMORY_BAR, SWAP_BAR] {
                assert!(!published.contains_key(&(bar, PropertyRef::VALUE)));
                assert_eq!(
                    published.get(&(bar, PropertyRef::VISIBILITY)),
                    Some(&Value::from(EnumToken::from(Visibility::Hidden)))
                );
            }
        }
        // The wording of each state, and the file each figure is read from.
        let mut system = SystemSample::missing(FigureGap::Unread(MissingReason::Denied));
        system.cpu = SystemCpu::WarmingUp;
        let summary = healthy(&sample(system));
        assert_eq!(
            summary.cpu.text,
            "Overall CPU (100% = all logical CPUs): Warming up"
        );
        assert_eq!(
            summary.memory.text,
            "Memory: Denied (reading meminfo was refused)"
        );
        assert_eq!(
            summary.load.text,
            "Load average (1, 5, 15 min): Denied (reading loadavg was refused)"
        );
        assert_eq!(
            summary.uptime.text,
            "Uptime: Denied (reading uptime was refused)"
        );
        let summary = healthy(&sample(SystemSample::missing(FigureGap::Unread(
            MissingReason::Unavailable,
        ))));
        assert_eq!(
            summary.cpu.text,
            "Overall CPU (100% = all logical CPUs): Unavailable (stat could not be read)"
        );
        assert_eq!(summary.cpu.role, TextRole::Warning);
        let mut system = SystemSample::missing(FigureGap::Unusable);
        system.cpu = SystemCpu::Interrupted;
        let summary = healthy(&sample(system));
        assert_eq!(
            summary.cpu.text,
            "Overall CPU (100% = all logical CPUs): Unavailable (no measurable interval since \
             the previous sample)"
        );
        assert_eq!(
            summary.memory.text,
            "Memory: Unavailable (meminfo has no usable MemTotal and MemAvailable)"
        );
        // A MemAvailable above MemTotal is not memory either.
        let mut system = FakeProcessSource::system();
        system.memory = Ok(MemoryFigures {
            total: 4096,
            available: 8192,
        });
        assert_eq!(
            healthy(&sample(system)).memory.text,
            "Memory: Unavailable (meminfo has no usable MemTotal and MemAvailable)"
        );
        let summary = healthy(&sample(SystemSample::not_provided()));
        assert_eq!(
            summary.load.text,
            "Load average (1, 5, 15 min): Unavailable (not provided by this source)"
        );
    }

    /// A collector error keeps every figure of the last successful sample on
    /// screen, marked: the freshness line names the error and that sample's
    /// time, in the warning role.
    #[test]
    fn a_collector_error_keeps_the_last_sample_and_says_so() {
        let last = sample(FakeProcessSource::system());
        let good = healthy(&last);
        for (error, words) in [
            (
                CollectorError::ProcessList(MissingReason::Denied),
                "could not list processes (permission denied)",
            ),
            (
                CollectorError::ProcessList(MissingReason::Unavailable),
                "could not list processes (unavailable)",
            ),
            (CollectorError::Rejected, "snapshot rejected"),
        ] {
            let failed = summarize(&State::Collecting {
                last: Some(&last),
                error: Some(error),
                source: &last.source,
            });
            assert_eq!(
                failed.freshness.text,
                format!(
                    "Collector error: {words} · last successful sample: 2027-01-15 08:00:00 UTC \
                     (server clock) · source: fixture-source"
                )
            );
            assert_eq!(failed.freshness.role, TextRole::Warning);
            // Every figure is the last sample's, unchanged.
            assert_eq!(
                Summary {
                    freshness: good.freshness.clone(),
                    ..failed
                },
                good
            );
        }
        // With no successful sample yet, there is no figure to keep.
        let source = SourceId("procfs:/proc".into());
        let failed = summarize(&State::Collecting {
            last: None,
            error: Some(CollectorError::ProcessList(MissingReason::Unavailable)),
            source: &source,
        });
        assert_eq!(
            failed.freshness.text,
            "Collector error: could not list processes (unavailable) · no successful sample \
             yet · source: procfs:/proc"
        );
        assert_eq!(failed.memory.text, "Memory: Not sampled");
        assert_eq!(
            failed.cpu.text,
            "Overall CPU (100% = all logical CPUs): Not sampled"
        );
        assert_eq!(
            failed.processes.text,
            "Processes visible to this reader: Not sampled"
        );
        assert_eq!(failed.memory_bar.tenths, None);
        // And the shell with no source at all says that.
        let empty = summarize(&State::NotStarted);
        assert_eq!(
            empty.freshness.text,
            "No sample: process collection not started"
        );
        assert_eq!(empty.freshness.role, TextRole::Status);
        assert_eq!(empty.uptime.text, "Uptime: Not sampled");
    }

    #[test]
    fn the_freshness_line_states_its_clock_and_zone_and_bounds_its_source() {
        let mut last = sample(FakeProcessSource::system());
        last.source = SourceId("é".repeat(400));
        let line = healthy(&last).freshness.text;
        assert!(line.ends_with('…'), "{line}");
        assert!(line.contains("UTC (server clock)"), "{line}");
        assert!(line.len() <= MAX_SUMMARY_TEXT_BYTES);
        // The source is cut at the status label's own bound, at a character.
        let (_, named) = line.split_once("source: ").unwrap();
        assert!(named.len() <= MAX_SOURCE_STATUS_BYTES + '…'.len_utf8());
        // A server clock set before 1970 is said to be, never formatted as a
        // date it is not.
        last.sampled_at = SnapshotTime(UNIX_EPOCH - Duration::from_secs(1));
        let line = healthy(&last).freshness.text;
        assert!(
            line.starts_with(
                "Last successful sample: before 1970-01-01 00:00:00 UTC (server clock) · "
            ),
            "{line}"
        );
    }

    #[test]
    fn the_process_count_names_its_scope_in_the_status_lines_words() {
        let count = |listed, unreadable, capped, complete| {
            processes(&ProcessCount {
                listed,
                unreadable,
                capped,
                complete,
            })
            .text
        };
        assert_eq!(
            count(3, 0, 0, true),
            "Processes visible to this reader: 3 listed · complete scan · no srtop filter"
        );
        assert_eq!(
            count(300, 12, 5, false),
            "Processes visible to this reader: 300 listed · 12 unreadable · 5 beyond the \
             record limit · incomplete scan · no srtop filter"
        );
        // Only identity was degraded: every listed record was read.
        assert_eq!(
            count(3, 0, 0, false),
            "Processes visible to this reader: 3 listed · incomplete scan · no srtop filter"
        );
        assert_eq!(
            count(0, 0, 0, true),
            "Processes visible to this reader: 0 listed · complete scan · no srtop filter",
            "an authoritative empty list is a real zero"
        );
        // Review round 1 (W2): the count is scoped to this reader's view, which a
        // PID namespace or hidepid narrows without any error, and never worded
        // as if it were every process on the host.
        for (listed, unreadable, capped, complete) in
            [(3, 0, 0, true), (300, 12, 5, false), (0, 0, 0, true)]
        {
            let line = count(listed, unreadable, capped, complete);
            assert!(
                line.starts_with("Processes visible to this reader: "),
                "{line}"
            );
            assert!(line.ends_with(" · no srtop filter"), "{line}");
            assert!(!line.contains("unfiltered"), "{line}");
        }
    }

    /// Every line and bar description fits its bound uncut, at its widest, so
    /// the bound only ever guards the snapshot reserve.
    #[test]
    fn the_widest_lines_fit_their_bounds_uncut() {
        let widest = SystemSample {
            cpu: SystemCpu::Measured(SystemCpuInterval {
                busy: u64::MAX,
                total: u64::MAX,
                cpus: Some(u32::MAX),
            }),
            memory: Ok(MemoryFigures {
                total: u64::MAX,
                available: 1,
            }),
            swap: Ok(SwapFigures {
                total: u64::MAX,
                free: 1,
            }),
            uptime: Ok(u64::MAX),
            load: Ok(LoadAverages {
                one: u64::MAX,
                five: u64::MAX,
                fifteen: u64::MAX,
            }),
        };
        let latest = UNIX_EPOCH
            .checked_add(Duration::from_secs(i64::MAX as u64 / 2))
            .unwrap_or_else(SystemTime::now);
        let mut samples = Vec::new();
        for system in [
            widest,
            SystemSample::missing(FigureGap::Unusable),
            SystemSample::missing(FigureGap::Unread(MissingReason::Denied)),
            SystemSample::missing(FigureGap::NotProvided),
        ] {
            samples.push(Sample {
                sampled_at: SnapshotTime(latest),
                source: SourceId("\u{20000}".repeat(MAX_SOURCE_STATUS_BYTES)),
                system,
                processes: ProcessCount {
                    listed: usize::MAX,
                    unreadable: usize::MAX,
                    capped: usize::MAX,
                    complete: false,
                },
            });
        }
        let source = SourceId("\u{20000}".repeat(MAX_SOURCE_STATUS_BYTES));
        let mut summaries = vec![summarize(&State::NotStarted)];
        for error in [
            None,
            Some(CollectorError::ProcessList(MissingReason::Denied)),
            Some(CollectorError::Rejected),
        ] {
            summaries.push(summarize(&State::Collecting {
                last: None,
                error,
                source: &source,
            }));
            for last in &samples {
                summaries.push(summarize(&State::Collecting {
                    last: Some(last),
                    error,
                    source: &source,
                }));
            }
        }
        let mut widest_line = 0;
        for summary in &summaries {
            for line in [
                &summary.cpu,
                &summary.memory,
                &summary.swap,
                &summary.load,
                &summary.uptime,
                &summary.processes,
                &summary.freshness,
            ] {
                assert!(line.text.len() <= MAX_SUMMARY_TEXT_BYTES, "{}", line.text);
                widest_line = widest_line.max(line.text.len());
            }
            for bar in [&summary.cpu_bar, &summary.memory_bar, &summary.swap_bar] {
                assert!(
                    bar.description.len() <= MAX_BAR_DESCRIPTION_BYTES,
                    "{}",
                    bar.description
                );
            }
        }
        assert!(
            widest_line > MAX_SUMMARY_TEXT_BYTES / 2,
            "the bound must be measured against a line that really is wide: {widest_line}"
        );
    }

    #[test]
    fn changes_set_only_what_differs_and_clear_what_is_gone() {
        let shown = properties(&healthy(&sample(FakeProcessSource::system())));
        assert!(changes(&shown, &shown).is_empty(), "nothing changed");
        let mut system = FakeProcessSource::system();
        system.memory = Err(FigureGap::Unread(MissingReason::Denied));
        let hidden = properties(&healthy(&sample(system)));
        let planned = changes(&shown, &hidden);
        let keys: Vec<(NodeId, PropertyRef)> = planned
            .iter()
            .map(|change| (change.node, change.property))
            .collect();
        assert_eq!(
            keys,
            vec![
                (MEMORY_BAR, PropertyRef::VALUE_DESCRIPTION),
                (MEMORY_BAR, PropertyRef::VISIBILITY),
                (MEMORY_BAR, PropertyRef::VALUE),
                (MEMORY_TEXT, PropertyRef::ROLE),
                (MEMORY_TEXT, PropertyRef::TEXT),
            ],
            "only the memory line and its bar change, in node and property order"
        );
        let value_of = |property| {
            planned
                .iter()
                .find(|change| change.node == MEMORY_BAR && change.property == property)
                .map(|change| change.value.clone())
        };
        assert_eq!(
            value_of(PropertyRef::VALUE),
            Some(None),
            "cleared, not zeroed"
        );
        assert_eq!(
            value_of(PropertyRef::VISIBILITY),
            Some(Some(Value::from(EnumToken::from(Visibility::Hidden))))
        );
        // And back: the value is set again and the bar shown.
        let restored = changes(&hidden, &shown);
        assert_eq!(restored.len(), 5);
        assert!(restored.iter().all(|change| change.value.is_some()));
    }

    /// The summary is built from standard Text and Progress widgets in Column
    /// and Row layout and nothing else: no image, no resource, no drawing.
    #[test]
    fn the_summary_is_text_and_progress_in_rows_and_a_column() {
        let operations =
            create_operations(&properties(&healthy(&sample(FakeProcessSource::system()))));
        assert_eq!(operations.len(), NODE_COUNT);
        for (operation, (id, node_type, parent)) in operations.iter().zip(LAYOUT) {
            let Operation::CreateNode {
                id: created,
                node_type: created_type,
                parent_id,
                properties,
                ..
            } = operation
            else {
                panic!("the summary only creates nodes: {operation:?}")
            };
            assert_eq!(
                (*created, *created_type, *parent_id),
                (id, node_type, Some(parent))
            );
            assert!(
                [
                    TypeRef::COLUMN,
                    TypeRef::ROW,
                    TypeRef::TEXT,
                    TypeRef::PROGRESS
                ]
                .contains(created_type),
                "{created_type:?}"
            );
            assert!(properties
                .iter()
                .all(|(property, _)| *property != PropertyRef::RESOURCE));
            let has = |wanted: PropertyRef| properties.iter().any(|(p, _)| *p == wanted);
            match *created_type {
                TypeRef::TEXT => assert!(has(PropertyRef::TEXT) && has(PropertyRef::ROLE)),
                TypeRef::PROGRESS => assert!(
                    has(PropertyRef::LABEL)
                        && has(PropertyRef::VISIBILITY)
                        && has(PropertyRef::VALUE_DESCRIPTION)
                ),
                _ => {}
            }
        }
    }
}
