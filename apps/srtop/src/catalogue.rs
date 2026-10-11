//! The typed metric catalogue and its collection-cost policy (design §§6.5, 8,
//! 20.4, 26; PX-042).
//!
//! Every field srtop publishes is one [`MetricSpec`]: a stable [`MetricKey`],
//! the typed [`Unit`] of its stored value, the §6.5 value shape it is collected
//! as ([`RawType`]), the interface it is read from ([`Origin`]), how values of
//! several processes may be combined ([`Aggregation`]), its default
//! [`Formatter`] and [`DefaultSort`], what the reader needs to collect it
//! ([`Capability`]), how much it reveals ([`Sensitivity`]), what one read costs
//! ([`CollectionCost`]) and which [`RefreshTier`]s may collect it. The prose
//! half — label, source and interpretation — stays the
//! [`MetricDefinition`] PX-005 to PX-007 wrote, and the catalogue checks its
//! unit text against the typed unit, so the two halves cannot drift.
//!
//! [`Catalogue::new`] refuses a malformed or duplicate key and every unit the
//! rest of an entry cannot carry: a raw type, formatter, aggregation or sort
//! that does not fit the unit, or a definition whose unit text names another
//! unit. It also enforces the cost policy: an expensive field may never be
//! collected on every sample (no all-metric polling), only on demand for the
//! rows that are selected or visible ([`crate::on_demand`]).
//!
//! Availability is one vocabulary for every field ([`Availability`]): present,
//! warming up, permission denied, unsupported, stale and failed are distinct,
//! and none but present carries a quantity. The existing per-field states map
//! onto it here without changing a single published cell.
//!
//! The process list's columns are [`PROCESS_LIST`]: the table's headings and
//! every row's cells are both derived from it, so a heading cannot name a field
//! other than the one published beneath it.
use crate::metric::{self, MetricDefinition};
use crate::source::{
    CpuUsage, Figure, FigureGap, MissingReason, Observed, ProcessKey, ProcessRecord, SystemCpu,
};
use srui_sdk::Value;
use std::collections::HashMap;
use std::fmt;

/// A metric's stable identifier: lowercase dotted segments such as
/// `process.resident_memory`. Never a display string, and never reused for a
/// different meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MetricKey(&'static str);

impl MetricKey {
    /// Longest key accepted, in bytes.
    pub const MAX_BYTES: usize = 64;

    pub const fn new(key: &'static str) -> Self {
        Self(key)
    }

    pub fn as_str(self) -> &'static str {
        self.0
    }

    /// At least two non-empty dot-separated segments of `a-z`, `0-9` and `_`,
    /// each starting with a letter, and at most [`Self::MAX_BYTES`] bytes.
    pub fn is_well_formed(self) -> bool {
        let key = self.0;
        if key.is_empty() || key.len() > Self::MAX_BYTES || !key.contains('.') {
            return false;
        }
        key.split('.').all(|segment| {
            segment.starts_with(|first: char| first.is_ascii_lowercase())
                && segment.chars().all(|character| {
                    character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
                })
        })
    }
}

impl fmt::Display for MetricKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.pad(self.0)
    }
}

/// What one field describes: one process instance, or the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Process,
    System,
}

/// The unit of a stored value. Its [`Self::describe`] text is exactly the
/// `unit` of the field's [`MetricDefinition`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Unit {
    /// An identifier that is numeric but not a quantity.
    ProcessId,
    /// Untrusted, sanitized text.
    Text,
    Bytes,
    /// Tenths of a percent of one logical CPU (100% = one CPU).
    TenthsOfOneCpu,
    /// Tenths of a percent of the time of all logical CPUs (100% = the host).
    TenthsOfAllCpus,
    Seconds,
    /// Hundredths of a task, as the kernel prints a load average.
    HundredthsOfTask,
    /// A wall-clock instant: seconds since the Unix epoch, UTC.
    UnixSeconds,
    Processes,
}

impl Unit {
    pub fn describe(self) -> &'static str {
        match self {
            Self::ProcessId => "process ID",
            Self::Text => "text",
            Self::Bytes => "bytes",
            Self::TenthsOfOneCpu => "tenths of a percent of one logical CPU",
            Self::TenthsOfAllCpus => "tenths of a percent of the time of all logical CPUs",
            Self::Seconds => "seconds",
            Self::HundredthsOfTask => "hundredths of a task",
            Self::UnixSeconds => "seconds since 1970-01-01 00:00:00 UTC",
            Self::Processes => "processes",
        }
    }

    /// Whether values in this unit are amounts that can be compared by size,
    /// added or averaged. An identifier, text and an instant are not.
    pub fn is_quantity(self) -> bool {
        !matches!(self, Self::ProcessId | Self::Text | Self::UnixSeconds)
    }

    /// The §6.5 value shapes a value in this unit may be collected as.
    pub fn carries(self, raw: RawType) -> bool {
        use RawType::*;
        matches!(
            (self, raw),
            (Self::ProcessId, UnsignedInteger)
                | (Self::Text, String)
                | (Self::Bytes, UnsignedInteger | UsedOfTotalRecord)
                | (Self::TenthsOfOneCpu, CpuTicksRecord)
                | (Self::TenthsOfAllCpus, SystemTicksRecord)
                | (Self::Seconds, UnsignedInteger)
                | (Self::HundredthsOfTask, UnsignedInteger | LoadAveragesRecord)
                | (Self::UnixSeconds, WallClock)
                | (Self::Processes, UnsignedInteger)
        )
    }
}

/// The shape a value is collected and stored as, from the small typed value
/// set of §6.5: an unsigned integer, a string, or a small typed record. A
/// value is stored exactly and becomes text only through its [`Formatter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RawType {
    UnsignedInteger,
    String,
    /// [`crate::source::CpuInterval`]: ticks, tick rate and monotonic elapsed time.
    CpuTicksRecord,
    /// [`crate::source::SystemCpuInterval`]: busy and total ticks of all CPUs.
    SystemTicksRecord,
    /// A used part and its total, both in the unit: memory or swap.
    UsedOfTotalRecord,
    /// [`crate::source::LoadAverages`]: three averages in hundredths.
    LoadAveragesRecord,
    /// [`crate::source::SnapshotTime`].
    WallClock,
}

/// The interface a field is read from. The definition's `source` text states
/// the exact fields; this is its machine-readable class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Origin {
    /// The numeric entries the scan listed in the scanned root.
    ScanListing,
    /// `/proc/<pid>/stat`.
    ProcPidStat,
    /// `/proc/<pid>/statm`.
    ProcPidStatm,
    /// `/proc/stat`.
    ProcStat,
    /// `/proc/meminfo`.
    ProcMeminfo,
    /// `/proc/uptime`.
    ProcUptime,
    /// `/proc/loadavg`.
    ProcLoadavg,
    /// The collecting server's own wall clock.
    ServerClock,
    /// A deterministic test source; never an OS claim.
    Fixture,
}

/// How values of the same field over several processes may be combined, for a
/// later group, tree or total. Only a quantity can be combined at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Aggregation {
    /// Not combinable: an identifier, text, an instant, or a host-wide figure
    /// that already describes the whole host.
    None,
    /// The sum is meaningful (CPU time shares, process counts).
    Sum,
    /// A quantity whose sum overstates the total, because the same resource is
    /// counted once per process (shared resident pages): a total may only be
    /// shown as an upper bound, never as the host's use.
    NonAdditive,
}

/// The default text form of a value. Each formatter accepts exactly one unit
/// and one raw type, and is the one function that turns them into text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Formatter {
    /// Decimal digits ([`Value::UnsignedInt`]).
    ProcessId,
    /// The sanitized text as published.
    SanitizedText,
    /// [`metric::format_iec_bytes`]: IEC multiples, truncated.
    IecBytes,
    /// IEC used of total and a truncated share, as the summary prints them.
    IecBytesUsedOfTotal,
    /// [`metric::cpu_cell`]: tenths of one CPU, truncated.
    ShareOfOneCpu,
    /// [`metric::format_cpu_tenths`] of all CPUs, as the summary prints it.
    ShareOfAllCpus,
    /// [`metric::format_uptime`]: whole minutes, truncated.
    Uptime,
    /// [`metric::format_hundredths`], three times.
    LoadAverages,
    /// [`metric::format_utc`]: UTC to the second, truncated.
    UtcTime,
    /// A decimal count.
    Count,
    /// [`metric::format_iec_bytes`] of an unsigned integer collected on demand;
    /// the same text as [`Self::IecBytes`].
    OnDemandBytes,
}

impl Formatter {
    /// The one unit and raw type this formatter accepts.
    pub fn accepts(self) -> (Unit, RawType) {
        match self {
            Self::ProcessId => (Unit::ProcessId, RawType::UnsignedInteger),
            Self::SanitizedText => (Unit::Text, RawType::String),
            Self::IecBytes | Self::OnDemandBytes => (Unit::Bytes, RawType::UnsignedInteger),
            Self::IecBytesUsedOfTotal => (Unit::Bytes, RawType::UsedOfTotalRecord),
            Self::ShareOfOneCpu => (Unit::TenthsOfOneCpu, RawType::CpuTicksRecord),
            Self::ShareOfAllCpus => (Unit::TenthsOfAllCpus, RawType::SystemTicksRecord),
            Self::Uptime => (Unit::Seconds, RawType::UnsignedInteger),
            Self::LoadAverages => (Unit::HundredthsOfTask, RawType::LoadAveragesRecord),
            Self::UtcTime => (Unit::UnixSeconds, RawType::WallClock),
            Self::Count => (Unit::Processes, RawType::UnsignedInteger),
        }
    }
}

/// How a column of this field sorts by default (applied by PX-009, which owns
/// sorting; the catalogue only states the typed order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SortKey {
    /// By the stored integer, never by its text: `9 B` sorts below `1.0 KiB`.
    Numeric,
    /// By the sanitized text.
    Lexical,
    /// By the instant.
    Chronological,
    /// One value per host: there is nothing to order.
    Unsorted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SortDirection {
    Ascending,
    Descending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DefaultSort {
    pub key: SortKey,
    pub direction: SortDirection,
}

impl DefaultSort {
    pub const fn new(key: SortKey, direction: SortDirection) -> Self {
        Self { key, direction }
    }

    /// Whether this order is defined for values in `unit`.
    pub fn fits(self, unit: Unit) -> bool {
        match self.key {
            SortKey::Numeric => unit.is_quantity() || unit == Unit::ProcessId,
            SortKey::Lexical => unit == Unit::Text,
            SortKey::Chronological => unit == Unit::UnixSeconds,
            SortKey::Unsorted => true,
        }
    }
}

/// What the reader needs to collect a field. srtop never acquires a
/// capability: a field the reader cannot collect is
/// [`Availability::PermissionDenied`], never zero and never escalated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    /// Any unprivileged reader of the scanned procfs. A per-record denial
    /// (`hidepid`, a mode-0 file) is still possible and reported as such.
    Unprivileged,
    /// The target's own user, or `CAP_SYS_PTRACE` over it: the class of
    /// `/proc/<pid>/` files gated by ptrace access mode checks (K1).
    SameUserOrPtrace,
}

/// How much a field reveals, for later privacy and export policy (PX-087).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sensitivity {
    /// A host-wide figure.
    Host,
    /// Metadata about one process that any reader of the procfs can see.
    ProcessMetadata,
    /// Content a process owns (arguments, environment, open paths): never
    /// collected without explicit demand.
    ProcessContent,
}

/// What one collection of a field costs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CollectionCost {
    /// Read with the scan, from a file it already opens or one more small read.
    Cheap,
    /// Large, slow or blocking per process (a walk of `smaps`, a descriptor
    /// table): collected only on demand, off the sample loop.
    Expensive,
}

/// When a field may be collected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RefreshTier {
    /// With every sample, inside the atomic snapshot of the process list.
    EverySample,
    /// Only for the selected or visible rows that ask for it, off the sample
    /// loop, each value carrying its own source time ([`crate::on_demand`]).
    OnDemand,
}

/// One catalogued field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetricSpec {
    pub definition: MetricDefinition,
    pub scope: Scope,
    pub unit: Unit,
    pub raw: RawType,
    pub origin: Origin,
    pub aggregation: Aggregation,
    pub formatter: Formatter,
    pub sort: DefaultSort,
    pub capability: Capability,
    pub sensitivity: Sensitivity,
    pub cost: CollectionCost,
    pub tiers: &'static [RefreshTier],
}

impl MetricSpec {
    pub fn key(&self) -> MetricKey {
        MetricKey(self.definition.id)
    }

    pub fn supports(&self, tier: RefreshTier) -> bool {
        self.tiers.contains(&tier)
    }

    /// Every rule one entry must satisfy on its own.
    fn check(&self) -> Result<(), CatalogueError> {
        let key = self.key();
        if !key.is_well_formed() {
            return Err(CatalogueError::MalformedKey(key.as_str()));
        }
        let incompatible = |conflict| {
            Err(CatalogueError::IncompatibleUnit {
                key,
                unit: self.unit,
                conflict,
            })
        };
        if self.definition.unit != self.unit.describe() {
            return incompatible(UnitConflict::Definition(self.definition.unit));
        }
        if !self.unit.carries(self.raw) {
            return incompatible(UnitConflict::RawType(self.raw));
        }
        if self.formatter.accepts() != (self.unit, self.raw) {
            return incompatible(UnitConflict::Formatter(self.formatter));
        }
        if self.aggregation != Aggregation::None && !self.unit.is_quantity() {
            return incompatible(UnitConflict::Aggregation(self.aggregation));
        }
        if !self.sort.fits(self.unit) {
            return incompatible(UnitConflict::Sort(self.sort.key));
        }
        let tier_policy = |reason| Err(CatalogueError::TierPolicy { key, reason });
        if self.tiers.is_empty() {
            return tier_policy("supports no refresh tier, so it could never be collected");
        }
        if self.tiers.len() == 2 && self.tiers[0] == self.tiers[1] {
            return tier_policy("names the same refresh tier twice");
        }
        if self.tiers.len() > 2 {
            return tier_policy("names more refresh tiers than exist");
        }
        if self.cost == CollectionCost::Expensive && self.supports(RefreshTier::EverySample) {
            return tier_policy("is expensive and may only be collected on demand");
        }
        if self.sensitivity == Sensitivity::ProcessContent
            && self.supports(RefreshTier::EverySample)
        {
            return tier_policy("is process content and may only be collected on demand");
        }
        if self.scope == Scope::System && self.supports(RefreshTier::OnDemand) {
            return tier_policy("is host-wide and has no row to be demanded for");
        }
        Ok(())
    }
}

/// The part of an entry that does not fit its unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitConflict {
    /// The definition's unit text names another unit.
    Definition(&'static str),
    RawType(RawType),
    Formatter(Formatter),
    Aggregation(Aggregation),
    Sort(SortKey),
}

/// Why a set of entries is not a catalogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogueError {
    MalformedKey(&'static str),
    DuplicateKey(MetricKey),
    IncompatibleUnit {
        key: MetricKey,
        unit: Unit,
        conflict: UnitConflict,
    },
    TierPolicy {
        key: MetricKey,
        reason: &'static str,
    },
    TooManyEntries(usize),
}

impl fmt::Display for CatalogueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedKey(key) => write!(formatter, "malformed metric key {key:?}"),
            Self::DuplicateKey(key) => write!(formatter, "duplicate metric key {key}"),
            Self::IncompatibleUnit {
                key,
                unit,
                conflict,
            } => write!(
                formatter,
                "{key}: unit {:?} is incompatible with {conflict:?}",
                unit.describe()
            ),
            Self::TierPolicy { key, reason } => write!(formatter, "{key} {reason}"),
            Self::TooManyEntries(count) => write!(
                formatter,
                "{count} entries exceed the catalogue bound of {}",
                Catalogue::MAX_ENTRIES
            ),
        }
    }
}

impl std::error::Error for CatalogueError {}

/// A validated set of [`MetricSpec`]s with unique keys.
#[derive(Debug, Clone)]
pub struct Catalogue {
    specs: Vec<MetricSpec>,
    index: HashMap<MetricKey, usize>,
}

impl Catalogue {
    /// Most entries one catalogue may hold: far above the field families the
    /// plan names, and a bound on what a later dynamic source can register.
    pub const MAX_ENTRIES: usize = 512;

    /// Validates every entry and refuses the first that breaks a rule.
    pub fn new(specs: impl IntoIterator<Item = MetricSpec>) -> Result<Self, CatalogueError> {
        let mut catalogue = Self {
            specs: Vec::new(),
            index: HashMap::new(),
        };
        for spec in specs {
            if catalogue.specs.len() == Self::MAX_ENTRIES {
                return Err(CatalogueError::TooManyEntries(Self::MAX_ENTRIES + 1));
            }
            spec.check()?;
            let key = spec.key();
            if catalogue.index.contains_key(&key) {
                return Err(CatalogueError::DuplicateKey(key));
            }
            catalogue.index.insert(key, catalogue.specs.len());
            catalogue.specs.push(spec);
        }
        Ok(catalogue)
    }

    /// The fields srtop publishes today.
    pub fn builtin() -> Result<Self, CatalogueError> {
        Self::new(BUILTIN)
    }

    pub fn get(&self, key: MetricKey) -> Option<&MetricSpec> {
        self.index.get(&key).map(|&position| &self.specs[position])
    }

    pub fn specs(&self) -> &[MetricSpec] {
        &self.specs
    }

    /// The fields of `scope` collected with every sample, in catalogue order.
    pub fn every_sample(&self, scope: Scope) -> impl Iterator<Item = &MetricSpec> {
        self.specs
            .iter()
            .filter(move |spec| spec.scope == scope && spec.supports(RefreshTier::EverySample))
    }
}

const EVERY_SAMPLE: &[RefreshTier] = &[RefreshTier::EverySample];

const fn process_field(
    definition: MetricDefinition,
    unit: Unit,
    raw: RawType,
    origin: Origin,
    aggregation: Aggregation,
    formatter: Formatter,
    sort: DefaultSort,
) -> MetricSpec {
    MetricSpec {
        definition,
        scope: Scope::Process,
        unit,
        raw,
        origin,
        aggregation,
        formatter,
        sort,
        capability: Capability::Unprivileged,
        sensitivity: Sensitivity::ProcessMetadata,
        cost: CollectionCost::Cheap,
        tiers: EVERY_SAMPLE,
    }
}

const fn system_field(
    definition: MetricDefinition,
    unit: Unit,
    raw: RawType,
    origin: Origin,
    formatter: Formatter,
) -> MetricSpec {
    MetricSpec {
        definition,
        scope: Scope::System,
        unit,
        raw,
        origin,
        aggregation: Aggregation::None,
        formatter,
        sort: DefaultSort::new(SortKey::Unsorted, SortDirection::Ascending),
        capability: Capability::Unprivileged,
        sensitivity: Sensitivity::Host,
        cost: CollectionCost::Cheap,
        tiers: EVERY_SAMPLE,
    }
}

/// Every field srtop publishes today, each cheap and collected with every
/// sample: the four process columns (PX-002, PX-005, PX-006) and the seven
/// system-summary figures (PX-007). No new metric family is added here.
pub const BUILTIN: [MetricSpec; 11] = [
    process_field(
        metric::PROCESS_ID,
        Unit::ProcessId,
        RawType::UnsignedInteger,
        Origin::ScanListing,
        Aggregation::None,
        Formatter::ProcessId,
        DefaultSort::new(SortKey::Numeric, SortDirection::Ascending),
    ),
    process_field(
        metric::PROCESS_NAME,
        Unit::Text,
        RawType::String,
        Origin::ProcPidStat,
        Aggregation::None,
        Formatter::SanitizedText,
        DefaultSort::new(SortKey::Lexical, SortDirection::Ascending),
    ),
    process_field(
        metric::RESIDENT_MEMORY,
        Unit::Bytes,
        RawType::UnsignedInteger,
        Origin::ProcPidStatm,
        Aggregation::NonAdditive,
        Formatter::IecBytes,
        DefaultSort::new(SortKey::Numeric, SortDirection::Descending),
    ),
    process_field(
        metric::CPU_USAGE,
        Unit::TenthsOfOneCpu,
        RawType::CpuTicksRecord,
        Origin::ProcPidStat,
        Aggregation::Sum,
        Formatter::ShareOfOneCpu,
        DefaultSort::new(SortKey::Numeric, SortDirection::Descending),
    ),
    system_field(
        metric::SYSTEM_CPU,
        Unit::TenthsOfAllCpus,
        RawType::SystemTicksRecord,
        Origin::ProcStat,
        Formatter::ShareOfAllCpus,
    ),
    system_field(
        metric::MEMORY_USED,
        Unit::Bytes,
        RawType::UsedOfTotalRecord,
        Origin::ProcMeminfo,
        Formatter::IecBytesUsedOfTotal,
    ),
    system_field(
        metric::SWAP_USED,
        Unit::Bytes,
        RawType::UsedOfTotalRecord,
        Origin::ProcMeminfo,
        Formatter::IecBytesUsedOfTotal,
    ),
    system_field(
        metric::UPTIME,
        Unit::Seconds,
        RawType::UnsignedInteger,
        Origin::ProcUptime,
        Formatter::Uptime,
    ),
    system_field(
        metric::LOAD_AVERAGE,
        Unit::HundredthsOfTask,
        RawType::LoadAveragesRecord,
        Origin::ProcLoadavg,
        Formatter::LoadAverages,
    ),
    system_field(
        metric::PROCESS_COUNT,
        Unit::Processes,
        RawType::UnsignedInteger,
        Origin::ScanListing,
        Formatter::Count,
    ),
    system_field(
        metric::SAMPLE_TIME,
        Unit::UnixSeconds,
        RawType::WallClock,
        Origin::ServerClock,
        Formatter::UtcTime,
    ),
];

/// Whether one field can be stated, and if not, why. Only `Present` carries a
/// quantity; every other state is published as a word with no digit in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Availability {
    Present,
    /// Needs a second sample, or its first on-demand read has not completed.
    WarmingUp,
    PermissionDenied,
    /// This source or platform does not provide the field.
    Unsupported,
    /// The last value is older than its freshness bound; it is kept for
    /// inspection but never presented as current.
    Stale,
    /// The read failed, or produced something no kernel writes.
    Failed,
}

impl Availability {
    /// The cell or line wording of a state that is not a value, or `None` for
    /// [`Self::Present`]. The existing wordings are kept exactly: a failed
    /// read still reads `Unavailable` and a denied one `Denied`.
    pub fn word(self) -> Option<&'static str> {
        match self {
            Self::Present => None,
            Self::WarmingUp => Some(metric::CPU_WARMING_UP),
            Self::PermissionDenied => Some(metric::missing_text(MissingReason::Denied)),
            Self::Unsupported => Some("Unsupported"),
            Self::Stale => Some("Stale"),
            Self::Failed => Some(metric::missing_text(MissingReason::Unavailable)),
        }
    }

    pub fn of_missing(reason: MissingReason) -> Self {
        match reason {
            MissingReason::Unavailable => Self::Failed,
            MissingReason::Denied => Self::PermissionDenied,
        }
    }

    pub fn of_observed<T>(observed: &Observed<T>) -> Self {
        match observed {
            Observed::Known(_) => Self::Present,
            Observed::Missing(reason) => Self::of_missing(*reason),
        }
    }

    /// A measured interval that cannot be divided by, or implies an
    /// implausible CPU count, is a failed measurement, as
    /// [`metric::cpu_cell`] publishes it.
    pub fn of_cpu(usage: &CpuUsage) -> Self {
        match usage {
            CpuUsage::Measured(interval) => match metric::cpu_tenths_of_percent(interval) {
                Some(_) => Self::Present,
                None => Self::Failed,
            },
            CpuUsage::WarmingUp => Self::WarmingUp,
            CpuUsage::Missing(reason) => Self::of_missing(*reason),
        }
    }

    pub fn of_gap(gap: FigureGap) -> Self {
        match gap {
            FigureGap::NotProvided => Self::Unsupported,
            FigureGap::Unread(reason) => Self::of_missing(reason),
            FigureGap::Unusable => Self::Failed,
        }
    }

    pub fn of_figure<T>(figure: &Figure<T>) -> Self {
        match figure {
            Ok(_) => Self::Present,
            Err(gap) => Self::of_gap(*gap),
        }
    }

    /// Interrupted counters left no interval: a failed measurement, never a
    /// zero.
    pub fn of_system_cpu(cpu: &SystemCpu) -> Self {
        match cpu {
            SystemCpu::Measured(interval) => {
                match metric::share_tenths(interval.busy, interval.total) {
                    Some(_) => Self::Present,
                    None => Self::Failed,
                }
            }
            SystemCpu::WarmingUp => Self::WarmingUp,
            SystemCpu::Interrupted => Self::Failed,
            SystemCpu::Missing(gap) => Self::of_gap(*gap),
        }
    }
}

/// One column of the process list, in the order [`PROCESS_LIST`] publishes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProcessField {
    Pid,
    Name,
    Resident,
    Cpu,
}

/// The process list's columns, collected with every sample. The table's
/// headings and every row's cells are both built from this one list.
pub const PROCESS_LIST: [ProcessField; 4] = [
    ProcessField::Pid,
    ProcessField::Name,
    ProcessField::Resident,
    ProcessField::Cpu,
];

impl ProcessField {
    /// The catalogue entry this column publishes.
    pub fn spec(self) -> &'static MetricSpec {
        match self {
            Self::Pid => &BUILTIN[0],
            Self::Name => &BUILTIN[1],
            Self::Resident => &BUILTIN[2],
            Self::Cpu => &BUILTIN[3],
        }
    }

    /// The column heading: the definition's label.
    pub fn heading(self) -> &'static str {
        self.spec().definition.label
    }

    /// Whether this record's value of this field can be stated. `key` is the
    /// session-anchored key the row is published under.
    pub fn availability(self, key: &ProcessKey, record: &ProcessRecord) -> Availability {
        match self {
            Self::Pid => Availability::of_observed(&key.pid),
            Self::Name => Availability::Present,
            Self::Resident => Availability::of_observed(&record.resident),
            Self::Cpu => Availability::of_cpu(&record.cpu),
        }
    }

    /// The published cell, exactly as PX-002, PX-005 and PX-006 publish it: a
    /// known PID is an unsigned integer and every other cell is text.
    pub fn cell(self, key: &ProcessKey, record: &ProcessRecord) -> Value {
        match self {
            Self::Pid => match key.pid {
                Observed::Known(pid) => Value::UnsignedInt(u64::from(pid)),
                Observed::Missing(reason) => {
                    Value::String(metric::missing_text(reason).to_string())
                }
            },
            Self::Name => Value::String(record.display_name.as_str().to_string()),
            // The one place a byte count becomes text: the stored value stays
            // an exact integer, and the client renders what this produced
            // rather than scaling a unit itself.
            Self::Resident => Value::String(metric::bytes_cell(&record.resident)),
            // Likewise for CPU: the stored interval stays exact, and the
            // warming-up and unread states are words, not zeros (PX-006).
            Self::Cpu => Value::String(metric::cpu_cell(&record.cpu)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{
        CpuInterval, FakeProcessSource, ProcessSource, ScriptedFakeSource, SystemCpuInterval,
    };
    use std::time::Duration;

    const FAKE_EXPENSIVE: MetricDefinition = MetricDefinition {
        id: "test.expensive_bytes",
        label: "Expensive",
        unit: "bytes",
        source: "A scripted test source",
        interpretation: "Deterministic; never an OS claim.",
    };

    const ON_DEMAND: &[RefreshTier] = &[RefreshTier::OnDemand];

    fn expensive() -> MetricSpec {
        MetricSpec {
            definition: FAKE_EXPENSIVE,
            scope: Scope::Process,
            unit: Unit::Bytes,
            raw: RawType::UnsignedInteger,
            origin: Origin::Fixture,
            aggregation: Aggregation::NonAdditive,
            formatter: Formatter::OnDemandBytes,
            sort: DefaultSort::new(SortKey::Numeric, SortDirection::Descending),
            capability: Capability::SameUserOrPtrace,
            sensitivity: Sensitivity::ProcessMetadata,
            cost: CollectionCost::Expensive,
            tiers: ON_DEMAND,
        }
    }

    #[test]
    fn the_builtin_catalogue_is_valid_and_lists_exactly_the_published_fields() {
        let catalogue = Catalogue::builtin().unwrap();
        let keys: Vec<&str> = catalogue
            .specs()
            .iter()
            .map(|spec| spec.key().as_str())
            .collect();
        assert_eq!(
            keys,
            [
                "process.pid",
                "process.name",
                "process.resident_memory",
                "process.cpu_usage",
                "system.cpu_busy",
                "system.memory_used",
                "system.swap_used",
                "system.uptime",
                "system.load_average",
                "system.process_count",
                "system.sample_time",
            ]
        );
        // Every field srtop publishes today is cheap and sampled every time;
        // none is collected on demand, and nothing is expensive.
        for spec in catalogue.specs() {
            assert_eq!(spec.cost, CollectionCost::Cheap, "{}", spec.key());
            assert_eq!(spec.tiers, EVERY_SAMPLE, "{}", spec.key());
            assert_eq!(catalogue.get(spec.key()), Some(spec));
        }
        let process: Vec<MetricKey> = catalogue
            .every_sample(Scope::Process)
            .map(MetricSpec::key)
            .collect();
        let columns: Vec<MetricKey> = PROCESS_LIST
            .iter()
            .map(|field| field.spec().key())
            .collect();
        assert_eq!(process, columns, "every cheap process field is a column");
        assert_eq!(catalogue.every_sample(Scope::System).count(), 7);
        assert_eq!(catalogue.get(MetricKey::new("process.unknown")), None);
    }

    #[test]
    fn headings_are_unchanged_by_the_catalogue() {
        let headings: Vec<&str> = PROCESS_LIST.iter().map(|field| field.heading()).collect();
        assert_eq!(headings, ["PID", "Name", "Resident", "CPU (100% = 1 CPU)"]);
    }

    #[test]
    fn duplicate_keys_are_rejected() {
        let resident = BUILTIN[2];
        assert_eq!(
            Catalogue::new([resident, BUILTIN[3], resident]).unwrap_err(),
            CatalogueError::DuplicateKey(MetricKey::new("process.resident_memory"))
        );
        // Same key, otherwise different entry: still the same meaning claimed
        // twice.
        let mut renamed = expensive();
        renamed.definition.id = metric::RESIDENT_MEMORY.id;
        assert_eq!(
            Catalogue::new([resident, renamed]).unwrap_err(),
            CatalogueError::DuplicateKey(MetricKey::new("process.resident_memory"))
        );
        let mut all = BUILTIN.to_vec();
        all.push(BUILTIN[10]);
        assert!(matches!(
            Catalogue::new(all),
            Err(CatalogueError::DuplicateKey(key)) if key.as_str() == "system.sample_time"
        ));
    }

    #[test]
    fn malformed_keys_are_rejected() {
        for bad in [
            "",
            "pid",
            "process.",
            ".pid",
            "Process.pid",
            "process.p-id",
            "process.1pid",
            "process..pid",
            "process.pid ",
        ] {
            let mut spec = expensive();
            spec.definition.id = bad;
            assert_eq!(
                Catalogue::new([spec]).unwrap_err(),
                CatalogueError::MalformedKey(bad),
                "{bad:?}"
            );
        }
        let mut long = expensive();
        long.definition.id = Box::leak(format!("process.{}", "a".repeat(57)).into_boxed_str());
        assert_eq!(long.definition.id.len(), MetricKey::MAX_BYTES + 1);
        assert!(Catalogue::new([long]).is_err());
        assert!(MetricKey::new("process.resident_memory_2").is_well_formed());
    }

    #[test]
    fn incompatible_units_are_rejected() {
        let key = MetricKey::new("test.expensive_bytes");
        let refused = |spec: MetricSpec, conflict| {
            assert_eq!(
                Catalogue::new([spec]).unwrap_err(),
                CatalogueError::IncompatibleUnit {
                    key,
                    unit: spec.unit,
                    conflict
                }
            );
        };
        // A definition that says one unit while the entry claims another.
        let mut spec = expensive();
        spec.definition.unit = "kB";
        refused(spec, UnitConflict::Definition("kB"));
        // Bytes cannot be carried as a CPU tick record or a string.
        let mut spec = expensive();
        spec.raw = RawType::CpuTicksRecord;
        refused(spec, UnitConflict::RawType(RawType::CpuTicksRecord));
        let mut spec = expensive();
        spec.raw = RawType::String;
        refused(spec, UnitConflict::RawType(RawType::String));
        // A byte count formatted as a CPU share, or as a time.
        let mut spec = expensive();
        spec.formatter = Formatter::ShareOfOneCpu;
        refused(spec, UnitConflict::Formatter(Formatter::ShareOfOneCpu));
        let mut spec = expensive();
        spec.formatter = Formatter::Uptime;
        refused(spec, UnitConflict::Formatter(Formatter::Uptime));
        // The CPU share of one CPU and of the host are different units, and
        // their formatters cannot be swapped.
        let mut cpu = BUILTIN[3];
        cpu.formatter = Formatter::ShareOfAllCpus;
        assert!(matches!(
            Catalogue::new([cpu]),
            Err(CatalogueError::IncompatibleUnit {
                conflict: UnitConflict::Formatter(Formatter::ShareOfAllCpus),
                ..
            })
        ));
        let mut cpu = BUILTIN[3];
        cpu.unit = Unit::TenthsOfAllCpus;
        assert!(matches!(
            Catalogue::new([cpu]),
            Err(CatalogueError::IncompatibleUnit {
                conflict: UnitConflict::Definition(_),
                ..
            })
        ));
        // An identifier or text cannot be summed or ordered by size.
        let mut pid = BUILTIN[0];
        pid.aggregation = Aggregation::Sum;
        assert!(matches!(
            Catalogue::new([pid]),
            Err(CatalogueError::IncompatibleUnit {
                conflict: UnitConflict::Aggregation(Aggregation::Sum),
                ..
            })
        ));
        let mut name = BUILTIN[1];
        name.sort = DefaultSort::new(SortKey::Numeric, SortDirection::Ascending);
        assert!(matches!(
            Catalogue::new([name]),
            Err(CatalogueError::IncompatibleUnit {
                conflict: UnitConflict::Sort(SortKey::Numeric),
                ..
            })
        ));
        let mut resident = BUILTIN[2];
        resident.sort = DefaultSort::new(SortKey::Lexical, SortDirection::Ascending);
        assert!(matches!(
            Catalogue::new([resident]),
            Err(CatalogueError::IncompatibleUnit {
                conflict: UnitConflict::Sort(SortKey::Lexical),
                ..
            })
        ));
        // The fixture itself is valid: each refusal above is the one change.
        assert!(Catalogue::new([expensive()]).is_ok());
    }

    #[test]
    fn an_expensive_field_may_never_be_collected_every_sample() {
        let mut spec = expensive();
        spec.tiers = &[RefreshTier::EverySample, RefreshTier::OnDemand];
        assert!(matches!(
            Catalogue::new([spec]),
            Err(CatalogueError::TierPolicy { .. })
        ));
        let mut content = BUILTIN[1];
        content.sensitivity = Sensitivity::ProcessContent;
        assert!(matches!(
            Catalogue::new([content]),
            Err(CatalogueError::TierPolicy { .. })
        ));
        for tiers in [
            &[][..],
            &[RefreshTier::OnDemand, RefreshTier::OnDemand][..],
            &[
                RefreshTier::OnDemand,
                RefreshTier::EverySample,
                RefreshTier::OnDemand,
            ][..],
        ] {
            let mut spec = expensive();
            spec.tiers = tiers;
            assert!(matches!(
                Catalogue::new([spec]),
                Err(CatalogueError::TierPolicy { .. })
            ));
        }
        let mut host = BUILTIN[7];
        host.tiers = ON_DEMAND;
        assert!(matches!(
            Catalogue::new([host]),
            Err(CatalogueError::TierPolicy { .. })
        ));
    }

    #[test]
    fn the_catalogue_is_bounded() {
        let specs = (0..=Catalogue::MAX_ENTRIES).map(|index| {
            let mut spec = expensive();
            spec.definition.id = Box::leak(format!("test.field_{index}").into_boxed_str());
            spec
        });
        assert_eq!(
            Catalogue::new(specs).unwrap_err(),
            CatalogueError::TooManyEntries(Catalogue::MAX_ENTRIES + 1)
        );
    }

    #[test]
    fn every_unavailable_state_is_distinct_and_never_a_quantity() {
        let states = [
            Availability::Present,
            Availability::WarmingUp,
            Availability::PermissionDenied,
            Availability::Unsupported,
            Availability::Stale,
            Availability::Failed,
        ];
        let words: Vec<Option<&str>> = states.iter().map(|state| state.word()).collect();
        assert_eq!(
            words,
            [
                None,
                Some("Warming up"),
                Some("Denied"),
                Some("Unsupported"),
                Some("Stale"),
                Some("Unavailable"),
            ]
        );
        for word in words.into_iter().flatten() {
            assert!(!word.chars().any(|character| character.is_ascii_digit()));
            assert!(word.len() <= metric::MAX_CELL_BYTES, "{word}");
        }
    }

    #[test]
    fn existing_states_map_onto_the_catalogue_vocabulary() {
        use Availability::*;
        let measured = |ticks| {
            CpuUsage::Measured(CpuInterval {
                ticks,
                ticks_per_second: 100,
                elapsed: Duration::from_secs(1),
            })
        };
        for (usage, expected) in [
            (measured(0), Present),
            (measured(u64::MAX), Failed),
            (CpuUsage::WarmingUp, WarmingUp),
            (CpuUsage::Missing(MissingReason::Denied), PermissionDenied),
            (CpuUsage::Missing(MissingReason::Unavailable), Failed),
        ] {
            assert_eq!(Availability::of_cpu(&usage), expected, "{usage:?}");
            // The word is the cell PX-006 already publishes.
            if let Some(word) = expected.word() {
                assert_eq!(metric::cpu_cell(&usage), word);
            }
        }
        for (observed, expected) in [
            (Observed::Known(0u64), Present),
            (Observed::Missing(MissingReason::Denied), PermissionDenied),
            (Observed::Missing(MissingReason::Unavailable), Failed),
        ] {
            assert_eq!(Availability::of_observed(&observed), expected);
            if let Some(word) = expected.word() {
                assert_eq!(metric::bytes_cell(&observed), word);
            }
        }
        for (gap, expected) in [
            (FigureGap::NotProvided, Unsupported),
            (FigureGap::Unread(MissingReason::Denied), PermissionDenied),
            (FigureGap::Unread(MissingReason::Unavailable), Failed),
            (FigureGap::Unusable, Failed),
        ] {
            assert_eq!(Availability::of_figure::<u64>(&Err(gap)), expected);
            assert_eq!(
                Availability::of_system_cpu(&SystemCpu::Missing(gap)),
                expected
            );
        }
        assert_eq!(Availability::of_figure(&Ok(7u64)), Present);
        let interval = |busy, total| {
            SystemCpu::Measured(SystemCpuInterval {
                busy,
                total,
                cpus: Some(4),
            })
        };
        assert_eq!(Availability::of_system_cpu(&interval(0, 400)), Present);
        assert_eq!(Availability::of_system_cpu(&interval(401, 400)), Failed);
        assert_eq!(
            Availability::of_system_cpu(&SystemCpu::WarmingUp),
            WarmingUp
        );
        assert_eq!(Availability::of_system_cpu(&SystemCpu::Interrupted), Failed);
    }

    /// The catalogue changes no published cell: every fixture row is built
    /// field by field from [`PROCESS_LIST`], and each non-present cell is the
    /// word of its availability.
    #[test]
    fn cells_follow_the_catalogue_and_keep_their_published_text() {
        let snapshot = FakeProcessSource.snapshot();
        let cells: Vec<Vec<Value>> = snapshot
            .records
            .iter()
            .map(|record| {
                PROCESS_LIST
                    .iter()
                    .map(|field| field.cell(&record.key, record))
                    .collect()
            })
            .collect();
        let text = |value: &str| Value::String(value.to_string());
        assert_eq!(
            cells,
            [
                vec![
                    Value::UnsignedInt(4101),
                    text("worker"),
                    text("1.1 MiB"),
                    text("250.0%")
                ],
                vec![
                    Value::UnsignedInt(4102),
                    text("worker"),
                    text("0 B"),
                    text("0.0%")
                ],
                vec![
                    text("Unavailable"),
                    text("helper"),
                    text("Denied"),
                    text("Warming up")
                ],
            ]
        );
        let words: Vec<&str> = [
            Availability::WarmingUp,
            Availability::PermissionDenied,
            Availability::Unsupported,
            Availability::Stale,
            Availability::Failed,
        ]
        .iter()
        .filter_map(|state| state.word())
        .collect();
        let mut scripted = ScriptedFakeSource::default();
        let mut checked = 0;
        for _ in 0..ScriptedFakeSource::STEPS {
            for record in scripted.snapshot().records {
                for field in PROCESS_LIST {
                    let state = field.availability(&record.key, &record);
                    let cell = field.cell(&record.key, &record);
                    match (state.word(), &cell) {
                        (Some(word), cell) => assert_eq!(cell, &text(word), "{field:?}"),
                        // A present metric is a quantity, never a state word.
                        (None, Value::String(value)) if field != ProcessField::Name => {
                            assert!(!words.contains(&value.as_str()), "{field:?}: {value}");
                            assert!(value.chars().any(|character| character.is_ascii_digit()));
                        }
                        (None, _) => {}
                    }
                    checked += 1;
                }
            }
        }
        assert!(checked > 0);
    }
}
