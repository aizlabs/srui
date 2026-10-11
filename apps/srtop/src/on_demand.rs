//! On-demand collection of expensive fields, off the sample loop (design
//! §§8, 20.4, 26; PX-042).
//!
//! A cheap field is read with every sample, inside the atomic snapshot of the
//! process list. An expensive one ([`crate::catalogue::CollectionCost::Expensive`])
//! is never polled for every process: it is read only for the rows a client
//! has selected or can see, and only while they ask for it. This module is that
//! policy, and it holds four guarantees:
//!
//! * **Hidden fields are not collected.** [`OnDemandFields::demand`] takes the
//!   complete set of (process instance, field) pairs wanted now. Anything not
//!   in it is forgotten, and nothing is read for it again; a field the
//!   catalogue does not allow on demand is refused, never quietly polled.
//! * **Slow source work cannot block the sample or control loop.** Every read
//!   is handed to a [`Dispatch`] — in production the runtime's blocking pool —
//!   and its result comes back through a channel the loop drains without
//!   waiting ([`OnDemandFields::collect_completed`]). At most
//!   [`Limits::max_in_flight`] reads are outstanding at once, so a stuck
//!   source occupies a bounded number of workers, and at most
//!   [`Limits::max_fields`] fields are tracked (§20.4: every queue is bounded).
//! * **A newer result is never overwritten by an older completion.** Each
//!   read carries a sequence number, and a completion is accepted only if it
//!   answers the read its field is currently waiting for. A read that was
//!   superseded — its row hidden and shown again, so a newer read was issued —
//!   is discarded whenever it finishes.
//! * **A delayed result cannot pass for a fresh snapshot value.** A value is
//!   stamped with the monotonic time its read *started*, which must lie
//!   between the request and the drain (a stamp outside that window is a
//!   failed read, not a value), and it is reported as
//!   [`FieldState::Stale`] — never present — once older than
//!   [`Limits::max_age`]. Its time is a [`SourceTime`] of its own: it is not
//!   the snapshot's sample time, and nothing merges it into the snapshot.
//!
//! No production field is collected on demand yet: every field srtop publishes
//! is cheap ([`crate::catalogue::BUILTIN`]). The first expensive collector,
//! and the selection or visible range that drives [`OnDemandFields::demand`],
//! connect here; this module's tests prove the policy with scripted sources.
use crate::catalogue::{Availability, Catalogue, MetricKey, RawType, RefreshTier};
use crate::source::ProcessKey;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A point on the collector's monotonic clock: the time since that clock's
/// epoch. Never a wall-clock time and never a [`crate::source::SnapshotTime`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceTime(pub Duration);

impl SourceTime {
    /// How long before `now` this was, or zero if it is not before `now`.
    pub fn age_at(self, now: SourceTime) -> Duration {
        now.0.saturating_sub(self.0)
    }
}

/// The monotonic clock on-demand reads are stamped with.
pub trait MonotonicClock: Send + Sync {
    fn now(&self) -> SourceTime;
}

/// [`Instant`] since the clock was made: `CLOCK_MONOTONIC` on Linux, so a
/// wall-clock change cannot age or rejuvenate a value.
#[derive(Debug, Clone, Copy)]
pub struct InstantClock {
    epoch: Instant,
}

impl InstantClock {
    pub fn new() -> Self {
        Self {
            epoch: Instant::now(),
        }
    }
}

impl Default for InstantClock {
    fn default() -> Self {
        Self::new()
    }
}

impl MonotonicClock for InstantClock {
    fn now(&self) -> SourceTime {
        SourceTime(self.epoch.elapsed())
    }
}

/// A value read on demand, in one of the §6.5 shapes an on-demand field may
/// have.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RawValue {
    UnsignedInteger(u64),
    String(String),
}

impl RawValue {
    pub fn raw_type(&self) -> RawType {
        match self {
            Self::UnsignedInteger(_) => RawType::UnsignedInteger,
            Self::String(_) => RawType::String,
        }
    }
}

/// Why an on-demand read produced no value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Unavailable {
    PermissionDenied,
    Unsupported,
    Failed,
}

pub type FieldResult = Result<RawValue, Unavailable>;

/// Reads one expensive field of one process instance. It may block for as
/// long as the read takes: it is only ever called from a [`Dispatch`]ed job,
/// never from the sample loop.
pub trait OnDemandSource: Send + Sync + 'static {
    fn read_field(&self, target: &ProcessKey, key: MetricKey) -> FieldResult;
}

/// One read, ready to run somewhere other than the sample loop.
pub type Job = Box<dyn FnOnce() + Send + 'static>;

/// Where reads run. It must return without running the job to completion.
pub trait Dispatch {
    fn dispatch(&self, job: Job);
}

/// The tokio runtime's blocking pool: the same place the sample loop's own
/// scan runs ([`crate::refresh::poll`]), so a blocking read never occupies a
/// runtime worker.
#[derive(Debug, Clone)]
pub struct BlockingPool {
    runtime: tokio::runtime::Handle,
}

impl BlockingPool {
    /// The pool of the runtime this is called in, or `None` outside one.
    pub fn current() -> Option<Self> {
        tokio::runtime::Handle::try_current()
            .ok()
            .map(|runtime| Self { runtime })
    }
}

impl Dispatch for BlockingPool {
    fn dispatch(&self, job: Job) {
        // Detached: the result comes back through the completion channel, and
        // a read that never finishes holds one in-flight slot, not the loop.
        drop(self.runtime.spawn_blocking(job));
    }
}

/// Bounds on on-demand collection (§20.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Most (process instance, field) pairs tracked at once.
    pub max_fields: usize,
    /// Most reads outstanding at once, including reads for fields that have
    /// since been hidden.
    pub max_in_flight: usize,
    /// Age past which a value is [`FieldState::Stale`].
    pub max_age: Duration,
    /// A field with a value younger than this is not read again yet.
    pub min_interval: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_fields: 1_024,
            max_in_flight: 4,
            max_age: Duration::from_secs(10),
            min_interval: Duration::from_secs(1),
        }
    }
}

/// What one field holds now. Only `Present` is a current value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldState {
    /// Demanded, and no read has completed yet.
    WarmingUp,
    Present {
        value: RawValue,
        observed_at: SourceTime,
    },
    /// The last value is older than [`Limits::max_age`]: kept, never current.
    Stale {
        value: RawValue,
        observed_at: SourceTime,
    },
    PermissionDenied,
    Unsupported,
    Failed,
}

impl FieldState {
    pub fn availability(&self) -> Availability {
        match self {
            Self::WarmingUp => Availability::WarmingUp,
            Self::Present { .. } => Availability::Present,
            Self::Stale { .. } => Availability::Stale,
            Self::PermissionDenied => Availability::PermissionDenied,
            Self::Unsupported => Availability::Unsupported,
            Self::Failed => Availability::Failed,
        }
    }
}

/// What one call to [`OnDemandFields::demand`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DemandReport {
    /// Pairs asked for, duplicates included.
    pub requested: usize,
    /// Reads handed to the dispatcher by this call.
    pub dispatched: usize,
    /// Keys the catalogue does not know.
    pub unknown: usize,
    /// Fields the catalogue does not allow on demand (they are sampled).
    pub not_on_demand: usize,
    /// Pairs beyond [`Limits::max_fields`].
    pub over_capacity: usize,
    /// Fields hidden since the previous call, and forgotten.
    pub forgotten: usize,
}

type FieldId = (ProcessKey, MetricKey);

#[derive(Debug, Clone, Copy)]
struct Pending {
    sequence: u64,
    issued_at: SourceTime,
}

#[derive(Debug, Clone)]
struct Reading {
    sequence: u64,
    observed_at: SourceTime,
    result: FieldResult,
}

#[derive(Debug, Clone, Default)]
struct Slot {
    pending: Option<Pending>,
    reading: Option<Reading>,
}

struct Completion {
    field: FieldId,
    sequence: u64,
    started: SourceTime,
    finished: SourceTime,
    result: FieldResult,
}

/// The on-demand fields of one session: what is demanded, what is being read,
/// and the last value of each.
pub struct OnDemandFields<D: Dispatch> {
    catalogue: Arc<Catalogue>,
    source: Arc<dyn OnDemandSource>,
    clock: Arc<dyn MonotonicClock>,
    dispatcher: D,
    limits: Limits,
    slots: HashMap<FieldId, Slot>,
    last_sequence: u64,
    in_flight: usize,
    sender: Sender<Completion>,
    completions: Receiver<Completion>,
}

impl<D: Dispatch> OnDemandFields<D> {
    pub fn new(
        catalogue: Arc<Catalogue>,
        source: Arc<dyn OnDemandSource>,
        clock: Arc<dyn MonotonicClock>,
        dispatcher: D,
        limits: Limits,
    ) -> Self {
        let (sender, completions) = mpsc::channel();
        Self {
            catalogue,
            source,
            clock,
            dispatcher,
            limits,
            slots: HashMap::new(),
            last_sequence: 0,
            in_flight: 0,
            sender,
            completions,
        }
    }

    /// Replaces the demanded set with `wanted` — the fields of the selected or
    /// visible rows — forgets every field not in it, and dispatches a read for
    /// each demanded field that has none outstanding and no value younger than
    /// [`Limits::min_interval`], in the order given, up to
    /// [`Limits::max_in_flight`]. Never waits for a read.
    pub fn demand(&mut self, wanted: impl IntoIterator<Item = FieldId>) -> DemandReport {
        let now = self.clock.now();
        let mut report = DemandReport::default();
        let mut order: Vec<FieldId> = Vec::new();
        let mut kept: HashSet<FieldId> = HashSet::new();
        for field in wanted {
            report.requested += 1;
            let Some(spec) = self.catalogue.get(field.1) else {
                report.unknown += 1;
                continue;
            };
            if !spec.supports(RefreshTier::OnDemand) {
                report.not_on_demand += 1;
                continue;
            }
            if kept.contains(&field) {
                continue;
            }
            if kept.len() == self.limits.max_fields {
                report.over_capacity += 1;
                continue;
            }
            kept.insert(field.clone());
            order.push(field);
        }
        let before = self.slots.len();
        self.slots.retain(|field, _| kept.contains(field));
        report.forgotten = before - self.slots.len();
        for field in order {
            if self.in_flight >= self.limits.max_in_flight {
                // Still demanded: it warms up, and a later call dispatches it.
                self.slots.entry(field).or_default();
                continue;
            }
            let slot = self.slots.entry(field.clone()).or_default();
            let fresh = slot
                .reading
                .as_ref()
                .is_some_and(|reading| reading.observed_at.age_at(now) < self.limits.min_interval);
            if slot.pending.is_some() || fresh {
                continue;
            }
            self.last_sequence += 1;
            let sequence = self.last_sequence;
            slot.pending = Some(Pending {
                sequence,
                issued_at: now,
            });
            self.in_flight += 1;
            report.dispatched += 1;
            let source = Arc::clone(&self.source);
            let clock = Arc::clone(&self.clock);
            let sender = self.sender.clone();
            self.dispatcher.dispatch(Box::new(move || {
                let started = clock.now();
                let result = source.read_field(&field.0, field.1);
                let finished = clock.now();
                // The collector may be gone; its result then has no reader.
                let _ = sender.send(Completion {
                    field,
                    sequence,
                    started,
                    finished,
                    result,
                });
            }));
        }
        report
    }

    /// Takes every read that has finished, without waiting for any other, and
    /// returns how many were accepted as the current value of their field.
    pub fn collect_completed(&mut self) -> usize {
        let now = self.clock.now();
        let mut accepted = 0;
        while let Ok(completion) = self.completions.try_recv() {
            accepted += usize::from(self.accept(completion, now));
        }
        accepted
    }

    /// As [`Self::collect_completed`], after waiting up to `timeout` for the
    /// first read to finish. A test's safety net, never the loop's.
    #[cfg(test)]
    fn collect_completed_within(&mut self, timeout: Duration) -> usize {
        match self.completions.recv_timeout(timeout) {
            Ok(completion) => {
                let now = self.clock.now();
                usize::from(self.accept(completion, now)) + self.collect_completed()
            }
            Err(_) => 0,
        }
    }

    fn accept(&mut self, completion: Completion, now: SourceTime) -> bool {
        self.in_flight = self.in_flight.saturating_sub(1);
        let Some(slot) = self.slots.get_mut(&completion.field) else {
            // Hidden since it was requested: the value has no reader.
            return false;
        };
        let Some(pending) = slot.pending else {
            return false;
        };
        if pending.sequence != completion.sequence {
            // Superseded by a newer read of the same field.
            return false;
        }
        slot.pending = None;
        if slot
            .reading
            .as_ref()
            .is_some_and(|reading| reading.sequence >= completion.sequence)
        {
            return false;
        }
        let bounded = pending.issued_at <= completion.started
            && completion.started <= completion.finished
            && completion.finished <= now;
        let raw = self.catalogue.get(completion.field.1).map(|spec| spec.raw);
        let result = match completion.result {
            // A stamp outside the request-to-drain window, or a value of the
            // wrong shape, is a failed read rather than a value.
            Ok(_) if !bounded => Err(Unavailable::Failed),
            Ok(value) if Some(value.raw_type()) != raw => Err(Unavailable::Failed),
            other => other,
        };
        let observed_at = if bounded {
            completion.started
        } else {
            pending.issued_at
        };
        slot.reading = Some(Reading {
            sequence: completion.sequence,
            observed_at,
            result,
        });
        true
    }

    /// The state of one field, or `None` when it is not demanded — a hidden
    /// field has no state, because it is not collected.
    pub fn state(&self, target: &ProcessKey, key: MetricKey) -> Option<FieldState> {
        let slot = self.slots.get(&(target.clone(), key))?;
        let Some(reading) = &slot.reading else {
            return Some(FieldState::WarmingUp);
        };
        let observed_at = reading.observed_at;
        Some(match &reading.result {
            Ok(value) if observed_at.age_at(self.clock.now()) > self.limits.max_age => {
                FieldState::Stale {
                    value: value.clone(),
                    observed_at,
                }
            }
            Ok(value) => FieldState::Present {
                value: value.clone(),
                observed_at,
            },
            Err(Unavailable::PermissionDenied) => FieldState::PermissionDenied,
            Err(Unavailable::Unsupported) => FieldState::Unsupported,
            Err(Unavailable::Failed) => FieldState::Failed,
        })
    }

    /// Reads outstanding now.
    pub fn in_flight(&self) -> usize {
        self.in_flight
    }

    /// Fields demanded now.
    pub fn tracked(&self) -> usize {
        self.slots.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue::{
        Aggregation, Capability, CollectionCost, DefaultSort, Formatter, MetricSpec, Origin, Scope,
        Sensitivity, SortDirection, SortKey, Unit, BUILTIN,
    };
    use crate::metric::MetricDefinition;
    use crate::refresh::ProcessView;
    use crate::source::{FakeProcessSource, ProcessSource, ScriptedFakeSource};
    use srui_sessiond::Session;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    const SMAPS_LIKE: MetricKey = MetricKey::new("test.expensive_bytes");
    const TABLE_LIKE: MetricKey = MetricKey::new("test.expensive_text");
    const ON_DEMAND: &[RefreshTier] = &[RefreshTier::OnDemand];

    fn expensive(id: &'static str, unit: Unit) -> MetricSpec {
        let (raw, formatter, unit_text, aggregation, sort) = match unit {
            Unit::Text => (
                RawType::String,
                Formatter::SanitizedText,
                "text",
                Aggregation::None,
                SortKey::Lexical,
            ),
            _ => (
                RawType::UnsignedInteger,
                Formatter::OnDemandBytes,
                "bytes",
                Aggregation::NonAdditive,
                SortKey::Numeric,
            ),
        };
        MetricSpec {
            definition: MetricDefinition {
                id,
                label: "Expensive",
                unit: unit_text,
                source: "A scripted test source",
                interpretation: "Deterministic; never an OS claim.",
            },
            scope: Scope::Process,
            unit,
            raw,
            origin: Origin::Fixture,
            aggregation,
            formatter,
            sort: DefaultSort::new(sort, SortDirection::Descending),
            capability: Capability::SameUserOrPtrace,
            sensitivity: Sensitivity::ProcessMetadata,
            cost: CollectionCost::Expensive,
            tiers: ON_DEMAND,
        }
    }

    fn catalogue() -> Arc<Catalogue> {
        let mut specs = BUILTIN.to_vec();
        specs.push(expensive(SMAPS_LIKE.as_str(), Unit::Bytes));
        specs.push(expensive(TABLE_LIKE.as_str(), Unit::Text));
        Arc::new(Catalogue::new(specs).unwrap())
    }

    /// A clock that moves only when a test moves it.
    #[derive(Default)]
    struct ManualClock(AtomicU64);

    impl ManualClock {
        fn advance(&self, by: Duration) {
            self.0
                .fetch_add(u64::try_from(by.as_nanos()).unwrap(), Ordering::SeqCst);
        }
    }

    impl MonotonicClock for ManualClock {
        fn now(&self) -> SourceTime {
            SourceTime(Duration::from_nanos(self.0.load(Ordering::SeqCst)))
        }
    }

    /// Queues jobs and runs them only when, and in the order, a test says.
    #[derive(Clone, Default)]
    struct Manual(Arc<Mutex<VecDeque<Job>>>);

    impl Manual {
        fn queued(&self) -> usize {
            self.0.lock().unwrap().len()
        }

        fn take(&self, index: usize) -> Job {
            self.0.lock().unwrap().remove(index).unwrap()
        }
    }

    impl Dispatch for Manual {
        fn dispatch(&self, job: Job) {
            self.0.lock().unwrap().push_back(job);
        }
    }

    /// Counts reads per field and answers from a script: a value derived
    /// from the read's ordinal, or a fixed outcome, optionally after moving
    /// the clock to model a slow read.
    struct Scripted {
        reads: Mutex<Vec<FieldId>>,
        outcome: Mutex<Option<FieldResult>>,
        clock: Arc<ManualClock>,
        read_takes: Mutex<Duration>,
    }

    impl Scripted {
        fn new(clock: Arc<ManualClock>) -> Arc<Self> {
            Arc::new(Self {
                reads: Mutex::new(Vec::new()),
                outcome: Mutex::new(None),
                clock,
                read_takes: Mutex::new(Duration::ZERO),
            })
        }

        fn reads(&self) -> Vec<FieldId> {
            self.reads.lock().unwrap().clone()
        }
    }

    impl OnDemandSource for Scripted {
        fn read_field(&self, target: &ProcessKey, key: MetricKey) -> FieldResult {
            let ordinal = {
                let mut reads = self.reads.lock().unwrap();
                reads.push((target.clone(), key));
                reads.len() as u64
            };
            self.clock.advance(*self.read_takes.lock().unwrap());
            if let Some(outcome) = self.outcome.lock().unwrap().clone() {
                return outcome;
            }
            Ok(if key == TABLE_LIKE {
                RawValue::String(format!("read {ordinal}"))
            } else {
                RawValue::UnsignedInteger(ordinal * 1_000)
            })
        }
    }

    struct Fixture {
        fields: OnDemandFields<Manual>,
        jobs: Manual,
        source: Arc<Scripted>,
        clock: Arc<ManualClock>,
    }

    fn fixture(limits: Limits) -> Fixture {
        let clock = Arc::new(ManualClock::default());
        let source = Scripted::new(Arc::clone(&clock));
        let jobs = Manual::default();
        let fields = OnDemandFields::new(
            catalogue(),
            Arc::clone(&source) as Arc<dyn OnDemandSource>,
            Arc::clone(&clock) as Arc<dyn MonotonicClock>,
            jobs.clone(),
            limits,
        );
        Fixture {
            fields,
            jobs,
            source,
            clock,
        }
    }

    fn limits() -> Limits {
        Limits {
            max_fields: 8,
            max_in_flight: 4,
            max_age: Duration::from_secs(10),
            min_interval: Duration::ZERO,
        }
    }

    fn targets() -> Vec<ProcessKey> {
        FakeProcessSource
            .snapshot()
            .records
            .into_iter()
            .map(|record| record.key)
            .collect()
    }

    fn present(value: u64, at: u64) -> FieldState {
        FieldState::Present {
            value: RawValue::UnsignedInteger(value),
            observed_at: SourceTime(Duration::from_secs(at)),
        }
    }

    #[test]
    fn hidden_expensive_fields_are_never_collected() {
        let mut f = fixture(limits());
        let [visible, hidden, _] = <[ProcessKey; 3]>::try_from(targets()).unwrap();
        let report = f.fields.demand([(visible.clone(), SMAPS_LIKE)]);
        assert_eq!(report.dispatched, 1);
        f.jobs.take(0)();
        assert_eq!(f.fields.collect_completed(), 1);
        assert_eq!(
            f.fields.state(&visible, SMAPS_LIKE),
            Some(present(1_000, 0))
        );
        // The other row and the other field were never asked for: never read,
        // and no state at all rather than a zero.
        assert_eq!(f.fields.state(&hidden, SMAPS_LIKE), None);
        assert_eq!(f.fields.state(&visible, TABLE_LIKE), None);
        assert_eq!(f.source.reads(), [(visible.clone(), SMAPS_LIKE)]);
        // The row scrolls out of view: its field is forgotten, and however
        // many samples pass, nothing is read for it again.
        for _ in 0..5 {
            f.clock.advance(Duration::from_secs(1));
            let report = f.fields.demand([]);
            assert_eq!(report.dispatched, 0);
            assert_eq!(f.jobs.queued(), 0);
        }
        assert_eq!(f.fields.state(&visible, SMAPS_LIKE), None);
        assert_eq!(f.fields.tracked(), 0);
        assert_eq!(f.source.reads().len(), 1);
    }

    #[test]
    fn sampled_and_unknown_fields_are_refused_rather_than_polled() {
        let mut f = fixture(limits());
        let target = targets().remove(0);
        let report = f.fields.demand([
            (target.clone(), BUILTIN[2].key()),
            (target.clone(), MetricKey::new("test.not_catalogued")),
            (target.clone(), SMAPS_LIKE),
            (target.clone(), SMAPS_LIKE),
        ]);
        assert_eq!(
            report,
            DemandReport {
                requested: 4,
                dispatched: 1,
                unknown: 1,
                not_on_demand: 1,
                over_capacity: 0,
                forgotten: 0,
            }
        );
        assert_eq!(f.fields.state(&target, BUILTIN[2].key()), None);
        assert_eq!(f.jobs.queued(), 1);
    }

    #[test]
    fn demand_and_outstanding_reads_are_bounded() {
        let mut f = fixture(Limits {
            max_fields: 4,
            max_in_flight: 2,
            ..limits()
        });
        let keys = targets();
        let wanted: Vec<FieldId> = keys
            .iter()
            .flat_map(|key| [(key.clone(), SMAPS_LIKE), (key.clone(), TABLE_LIKE)])
            .collect();
        let report = f.fields.demand(wanted.clone());
        assert_eq!(report.over_capacity, 2);
        assert_eq!(report.dispatched, 2);
        assert_eq!(f.fields.tracked(), 4);
        assert_eq!(f.fields.in_flight(), 2);
        // Nothing completes: asking again dispatches nothing more, and the
        // waiting fields warm up rather than read as zero.
        assert_eq!(f.fields.demand(wanted.clone()).dispatched, 0);
        assert_eq!(f.jobs.queued(), 2);
        assert_eq!(
            f.fields.state(&keys[1], SMAPS_LIKE),
            Some(FieldState::WarmingUp)
        );
        // A finished read frees one slot for the next field in order.
        f.jobs.take(0)();
        f.fields.collect_completed();
        assert_eq!(f.fields.in_flight(), 1);
        assert_eq!(f.fields.demand(wanted).dispatched, 1);
        assert_eq!(f.fields.in_flight(), 2);
    }

    /// A field hidden while its read is outstanding, then shown again, is
    /// read anew. Whichever of the two reads finishes last, the field holds
    /// the newer one.
    #[test]
    fn an_older_completion_never_overwrites_a_newer_result() {
        for older_finishes_first in [false, true] {
            let mut f = fixture(limits());
            let target = targets().remove(0);
            f.fields.demand([(target.clone(), SMAPS_LIKE)]);
            let older = f.jobs.take(0);
            f.fields.demand([]);
            f.clock.advance(Duration::from_secs(1));
            f.fields.demand([(target.clone(), SMAPS_LIKE)]);
            let newer = f.jobs.take(0);
            if older_finishes_first {
                older();
                newer();
            } else {
                newer();
                older();
            }
            // Both completions arrive in one drain, in the order they ran.
            assert_eq!(f.fields.collect_completed(), 1);
            let expected = if older_finishes_first { 2_000 } else { 1_000 };
            assert_eq!(
                f.fields.state(&target, SMAPS_LIKE),
                Some(present(expected, 1)),
                "older finishes first: {older_finishes_first}"
            );
            assert_eq!(f.fields.in_flight(), 0);
        }
    }

    #[test]
    fn an_older_completion_drained_later_is_still_discarded() {
        let mut f = fixture(limits());
        let target = targets().remove(0);
        f.fields.demand([(target.clone(), SMAPS_LIKE)]);
        let older = f.jobs.take(0);
        f.fields.demand([]);
        f.fields.demand([(target.clone(), SMAPS_LIKE)]);
        f.jobs.take(0)();
        assert_eq!(f.fields.collect_completed(), 1);
        let newer = f.fields.state(&target, SMAPS_LIKE);
        assert_eq!(newer, Some(present(1_000, 0)));
        older();
        assert_eq!(f.fields.collect_completed(), 0);
        assert_eq!(f.fields.state(&target, SMAPS_LIKE), newer);
        assert_eq!(f.fields.in_flight(), 0);
    }

    /// A read that took longer than the freshness bound is stamped with the
    /// time it started, so it arrives already stale; it is never presented as
    /// a value of the sample in which it happened to finish.
    #[test]
    fn a_delayed_result_is_stale_rather_than_fresh() {
        let mut f = fixture(limits());
        let target = targets().remove(0);
        f.fields.demand([(target.clone(), SMAPS_LIKE)]);
        *f.source.read_takes.lock().unwrap() = Duration::from_secs(11);
        f.jobs.take(0)();
        assert_eq!(f.fields.collect_completed(), 1);
        let state = f.fields.state(&target, SMAPS_LIKE).unwrap();
        assert_eq!(
            state,
            FieldState::Stale {
                value: RawValue::UnsignedInteger(1_000),
                observed_at: SourceTime(Duration::ZERO),
            }
        );
        assert_eq!(state.availability(), Availability::Stale);
        // A fresh read replaces it, and a value ages into staleness.
        *f.source.read_takes.lock().unwrap() = Duration::ZERO;
        f.fields.demand([(target.clone(), SMAPS_LIKE)]);
        f.jobs.take(0)();
        f.fields.collect_completed();
        assert_eq!(
            f.fields.state(&target, SMAPS_LIKE),
            Some(present(2_000, 11))
        );
        f.clock.advance(Duration::from_secs(10));
        assert_eq!(
            f.fields.state(&target, SMAPS_LIKE),
            Some(present(2_000, 11))
        );
        f.clock.advance(Duration::from_nanos(1));
        assert_eq!(
            f.fields.state(&target, SMAPS_LIKE).unwrap().availability(),
            Availability::Stale
        );
    }

    /// A completion whose stamps fall outside the window between its request
    /// and the drain cannot have been read when it claims: it is a failed
    /// read, never a value with an invented time.
    #[test]
    fn a_result_stamped_outside_its_request_window_is_failed() {
        let clock = Arc::new(ManualClock::default());
        clock.advance(Duration::from_secs(5));
        let source = Scripted::new(Arc::clone(&clock));
        let jobs = Manual::default();
        let mut fields = OnDemandFields::new(
            catalogue(),
            Arc::clone(&source) as Arc<dyn OnDemandSource>,
            Arc::clone(&clock) as Arc<dyn MonotonicClock>,
            jobs.clone(),
            limits(),
        );
        let target = targets().remove(0);
        fields.demand([(target.clone(), SMAPS_LIKE)]);
        // A completion claiming a time before its own request.
        fields
            .sender
            .send(Completion {
                field: (target.clone(), SMAPS_LIKE),
                sequence: 1,
                started: SourceTime(Duration::from_secs(1)),
                finished: SourceTime(Duration::from_secs(5)),
                result: Ok(RawValue::UnsignedInteger(7)),
            })
            .unwrap();
        assert_eq!(fields.collect_completed(), 1);
        assert_eq!(fields.state(&target, SMAPS_LIKE), Some(FieldState::Failed));
        // And one claiming a time after the drain.
        fields.demand([(target.clone(), SMAPS_LIKE)]);
        fields
            .sender
            .send(Completion {
                field: (target.clone(), SMAPS_LIKE),
                sequence: 2,
                started: SourceTime(Duration::from_secs(5)),
                finished: SourceTime(Duration::from_secs(6)),
                result: Ok(RawValue::UnsignedInteger(7)),
            })
            .unwrap();
        assert_eq!(fields.collect_completed(), 1);
        assert_eq!(fields.state(&target, SMAPS_LIKE), Some(FieldState::Failed));
        // The dispatched jobs, run late, are both superseded.
        while jobs.queued() > 0 {
            jobs.take(0)();
        }
        assert_eq!(fields.collect_completed(), 0);
        assert_eq!(fields.in_flight(), 0);
    }

    #[test]
    fn every_unavailable_outcome_is_its_own_state_and_never_zero() {
        let mut f = fixture(limits());
        let target = targets().remove(0);
        for (outcome, expected) in [
            (
                Err(Unavailable::PermissionDenied),
                FieldState::PermissionDenied,
            ),
            (Err(Unavailable::Unsupported), FieldState::Unsupported),
            (Err(Unavailable::Failed), FieldState::Failed),
            // A value of the wrong shape for its catalogue entry.
            (Ok(RawValue::String("0".into())), FieldState::Failed),
            (
                Ok(RawValue::UnsignedInteger(0)),
                FieldState::Present {
                    value: RawValue::UnsignedInteger(0),
                    observed_at: SourceTime(Duration::ZERO),
                },
            ),
        ] {
            *f.source.outcome.lock().unwrap() = Some(outcome);
            assert_eq!(
                f.fields.demand([(target.clone(), SMAPS_LIKE)]).dispatched,
                1
            );
            f.jobs.take(0)();
            f.fields.collect_completed();
            assert_eq!(f.fields.state(&target, SMAPS_LIKE), Some(expected));
        }
        let mut first = fixture(limits());
        first.fields.demand([(target.clone(), SMAPS_LIKE)]);
        assert_eq!(
            first.fields.state(&target, SMAPS_LIKE),
            Some(FieldState::WarmingUp)
        );
    }

    #[test]
    fn a_recent_value_is_not_read_again_before_its_interval() {
        let mut f = fixture(Limits {
            min_interval: Duration::from_secs(2),
            ..limits()
        });
        let target = targets().remove(0);
        let wanted = [(target.clone(), SMAPS_LIKE)];
        f.fields.demand(wanted.clone());
        f.jobs.take(0)();
        f.fields.collect_completed();
        f.clock.advance(Duration::from_secs(1));
        assert_eq!(f.fields.demand(wanted.clone()).dispatched, 0);
        f.clock.advance(Duration::from_secs(1));
        assert_eq!(f.fields.demand(wanted).dispatched, 1);
    }

    /// The production dispatcher: a read that blocks indefinitely leaves the
    /// ordinary sample loop — scan, project, commit — running tick after
    /// tick, and its result is taken once it does finish. The read is held by
    /// a gate the test opens; nothing races a wall clock, and the only timeout
    /// is a safety net for a broken implementation.
    #[tokio::test]
    async fn a_blocked_read_does_not_block_the_sample_loop() {
        struct Gated {
            gate: Mutex<Receiver<()>>,
            sample_loop: std::thread::ThreadId,
        }
        impl OnDemandSource for Gated {
            fn read_field(&self, _: &ProcessKey, _: MetricKey) -> FieldResult {
                // A dispatcher that ran the read on the loop's own thread
                // would wait on the gate forever; answer at once instead, so
                // that mistake fails the assertions below rather than hangs.
                if std::thread::current().id() == self.sample_loop {
                    return Ok(RawValue::UnsignedInteger(0));
                }
                self.gate.lock().unwrap().recv().unwrap();
                Ok(RawValue::UnsignedInteger(42))
            }
        }
        let (open, gate) = mpsc::channel();
        let clock = Arc::new(ManualClock::default());
        let mut fields = OnDemandFields::new(
            catalogue(),
            Arc::new(Gated {
                gate: Mutex::new(gate),
                sample_loop: std::thread::current().id(),
            }),
            Arc::clone(&clock) as Arc<dyn MonotonicClock>,
            BlockingPool::current().unwrap(),
            limits(),
        );
        let session = Session::mint();
        let mut processes = ScriptedFakeSource::default();
        let (mut view, first) = ProcessView::start(&session, &mut processes).unwrap();
        let target = first.records[0].key.clone();
        assert_eq!(fields.demand([(target.clone(), SMAPS_LIKE)]).dispatched, 1);
        let revision = session.current_revision();
        for tick in 1..=ScriptedFakeSource::STEPS {
            let snapshot = processes.snapshot();
            view.apply(&session, ScriptedFakeSource::STATUS_TEXT, &snapshot)
                .unwrap();
            assert_eq!(fields.collect_completed(), 0, "tick {tick}");
            assert_eq!(fields.demand([(target.clone(), SMAPS_LIKE)]).dispatched, 0);
            assert_eq!(
                fields.state(&target, SMAPS_LIKE),
                Some(FieldState::WarmingUp)
            );
        }
        assert!(
            session.current_revision() > revision,
            "the loop published while the read was blocked"
        );
        open.send(()).unwrap();
        assert_eq!(fields.collect_completed_within(Duration::from_secs(60)), 1);
        assert_eq!(
            fields.state(&target, SMAPS_LIKE),
            Some(FieldState::Present {
                value: RawValue::UnsignedInteger(42),
                observed_at: SourceTime(Duration::ZERO),
            })
        );
    }
}
