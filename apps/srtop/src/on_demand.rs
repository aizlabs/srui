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
//!   [`Limits::max_in_flight`] reads are outstanding at once and at most
//!   [`Limits::max_fields`] fields are tracked (§20.4: every queue is
//!   bounded). Every dispatched read answers exactly once: a job dropped unrun
//!   or a read that panics answers as a failure through its reply guard, and a
//!   read outstanding past [`Limits::read_deadline`] is given up — its field
//!   fails and its slot is reclaimed. A field never has more than one read
//!   outstanding, in flight or given up, however often it is hidden and
//!   shown again: a field shown again while its read is in flight adopts
//!   that read, and a field whose read was given up is not read again until
//!   that read returns. So one wedged field holds exactly one worker and
//!   every other field keeps being read. Reads
//!   still running — in flight or given up — never exceed
//!   [`Limits::max_abandoned`], so a stuck source occupies a bounded number of
//!   workers.
//!
//!   The residual: if that many distinct fields wedge at once, nothing can be
//!   dispatched until one of their reads returns. Collection then stalls, but
//!   honestly: a field with a value turns stale, a field without one reads
//!   failed once it has waited past the read deadline (a field merely queued
//!   behind healthy reads stays warming up), and collection resumes
//!   by itself as soon as any wedged read returns. A wedged read that never
//!   returns holds its worker for the life of the process; nothing here can
//!   cancel blocking work.
//!
//!   The deadline is measured on [`SuspendAwareClock`], so a wall clock set
//!   forward by at least [`Limits::read_deadline`] gives up every read in
//!   flight at that moment: their fields read failed until their reads
//!   return, and recover on the next read.
//! * **Every demanded field is read.** Dispatch goes to never-read fields
//!   first, then to the oldest reading, so no field starves behind the front
//!   of the demanded list.
//! * **A newer result is never overwritten by an older completion.** A field
//!   has at most one read outstanding, so a newer read of it is only ever
//!   dispatched after the older one has answered; and each read carries a
//!   sequence number, so a completion is accepted only if it answers the
//!   read its field is currently waiting for, never twice and never after a
//!   newer reading.
//! * **A delayed result cannot pass for a fresh snapshot value.** A value is
//!   stamped with the monotonic time its read *started*, which must lie
//!   between the request and the drain (a stamp outside that window is a
//!   failed read, not a value), and it is reported as
//!   [`FieldState::Stale`] — never present — once older than
//!   [`Limits::max_age`]. Its time is a [`SourceTime`] of its own: it is not
//!   the snapshot's sample time, and nothing merges it into the snapshot.
//!   In production that time is [`SuspendAwareClock`], which counts time the
//!   host spent suspended, so a value read before a sleep is stale after it.
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

/// The production clock for freshness: monotonic, and counting time the host
/// spent suspended.
///
/// [`Instant`] alone does not advance across a suspend (`CLOCK_MONOTONIC` on
/// Linux, `CLOCK_UPTIME_RAW` on macOS), so a value read before a long sleep
/// would still be younger than [`Limits::max_age`] on resume. That is right
/// for a CPU interval ([`crate::procfs::SystemMonotonicClock`]), and wrong
/// for freshness. A suspend-inclusive kernel clock (`CLOCK_BOOTTIME`,
/// `mach_continuous_time`) needs FFI, which srtop's production code may not
/// use (PX-008's audit), so this clock derives the same thing from `std`:
/// each reading advances by the larger of the [`Instant`] and the wall-clock
/// ([`std::time::SystemTime`]) progress since the previous reading
/// ([`ElapsedTracker`]). The wall clock keeps running across a suspend, so
/// the gap is counted; a wall clock set backwards is ignored, so the clock
/// never stops or reverses; and a wall clock set forwards ages values early —
/// the safe direction: a value turns stale sooner, never later. What it
/// cannot see is a suspend across which the wall clock itself did not advance
/// (a host without a running real-time clock).
#[derive(Debug)]
pub struct SuspendAwareClock {
    origin: Instant,
    tracker: std::sync::Mutex<ElapsedTracker>,
}

impl SuspendAwareClock {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
            tracker: std::sync::Mutex::new(ElapsedTracker::new(wall_clock())),
        }
    }
}

impl Default for SuspendAwareClock {
    fn default() -> Self {
        Self::new()
    }
}

impl MonotonicClock for SuspendAwareClock {
    fn now(&self) -> SourceTime {
        let mut tracker = match self.tracker.lock() {
            Ok(tracker) => tracker,
            // A reading cannot leave the tracker half-updated.
            Err(poisoned) => poisoned.into_inner(),
        };
        tracker.advance(self.origin.elapsed(), wall_clock())
    }
}

/// The wall clock as time since the Unix epoch, or `None` before it.
fn wall_clock() -> Option<Duration> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
}

/// The arithmetic of [`SuspendAwareClock`], on readings the caller supplies:
/// each step adds the larger of the monotonic and the wall-clock progress
/// since the previous step, where wall-clock progress that went backwards, or
/// a wall clock that could not be read, counts as none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElapsedTracker {
    total: Duration,
    monotonic: Duration,
    wall: Option<Duration>,
}

impl ElapsedTracker {
    pub fn new(wall: Option<Duration>) -> Self {
        Self {
            total: Duration::ZERO,
            monotonic: Duration::ZERO,
            wall,
        }
    }

    /// `monotonic` is time since the tracker's origin on a monotonic clock;
    /// `wall` is the wall clock now.
    pub fn advance(&mut self, monotonic: Duration, wall: Option<Duration>) -> SourceTime {
        let monotonic_step = monotonic.saturating_sub(self.monotonic);
        let wall_step = match (self.wall, wall) {
            (Some(before), Some(now)) => now.saturating_sub(before),
            _ => Duration::ZERO,
        };
        self.total = self.total.saturating_add(monotonic_step.max(wall_step));
        self.monotonic = self.monotonic.max(monotonic);
        if wall.is_some() {
            self.wall = wall;
        }
        SourceTime(self.total)
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
        // Detached: the result comes back through the completion channel. A
        // job the pool drops unrun, or that panics, still answers (its reply
        // guard sends a failure), and one that never finishes is given up at
        // its deadline.
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
    /// A read outstanding this long is given up: its field becomes
    /// [`FieldState::Failed`], its in-flight slot is reclaimed, and whatever
    /// it returns later is discarded. While dispatch is closed by given-up
    /// reads, a demanded field still without a value this long after it was
    /// first demanded also reads failed.
    pub read_deadline: Duration,
    /// Most reads still running at once, in flight and given up together: a
    /// read is dispatched only while fewer than this many are outstanding, so
    /// given-up reads never exceed it either, and a source that hangs every
    /// read occupies at most this many workers. At least `max_in_flight`
    /// ([`Limits::validate`]).
    pub max_abandoned: usize,
}

/// Why a set of [`Limits`] is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitsError {
    /// A bound that must be positive is zero: the named limit would let
    /// nothing be tracked, dispatched, current or ever given up.
    Zero(&'static str),
    /// `max_abandoned` is below `max_in_flight`, so `max_in_flight` could
    /// never be reached.
    AbandonedBelowInFlight {
        max_in_flight: usize,
        max_abandoned: usize,
    },
}

impl std::fmt::Display for LimitsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Zero(limit) => write!(formatter, "on-demand limit {limit} must be positive"),
            Self::AbandonedBelowInFlight {
                max_in_flight,
                max_abandoned,
            } => write!(
                formatter,
                "max_abandoned ({max_abandoned}) is below max_in_flight ({max_in_flight})"
            ),
        }
    }
}

impl std::error::Error for LimitsError {}

impl Limits {
    /// Every rule [`OnDemandFields::new`] holds a set of limits to: every
    /// count and the read deadline and freshness bound positive, and
    /// `max_abandoned` at least `max_in_flight`. `min_interval` may be zero:
    /// a field is then re-read as soon as its previous read answers.
    pub fn validate(&self) -> Result<(), LimitsError> {
        let zero = [
            ("max_fields", self.max_fields == 0),
            ("max_in_flight", self.max_in_flight == 0),
            ("max_abandoned", self.max_abandoned == 0),
            ("max_age", self.max_age.is_zero()),
            ("read_deadline", self.read_deadline.is_zero()),
        ];
        if let Some((limit, _)) = zero.iter().find(|(_, is_zero)| *is_zero) {
            return Err(LimitsError::Zero(limit));
        }
        if self.max_abandoned < self.max_in_flight {
            return Err(LimitsError::AbandonedBelowInFlight {
                max_in_flight: self.max_in_flight,
                max_abandoned: self.max_abandoned,
            });
        }
        Ok(())
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_fields: 1_024,
            max_in_flight: 4,
            max_age: Duration::from_secs(10),
            min_interval: Duration::from_secs(1),
            read_deadline: Duration::from_secs(30),
            max_abandoned: 16,
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

#[derive(Debug, Clone)]
struct Slot {
    pending: Option<Pending>,
    reading: Option<Reading>,
    /// When this field was first demanded, since it was last hidden.
    demanded_since: SourceTime,
}

impl Slot {
    fn new(now: SourceTime) -> Self {
        Self {
            pending: None,
            reading: None,
            demanded_since: now,
        }
    }
}

struct Completion {
    field: FieldId,
    sequence: u64,
    started: SourceTime,
    finished: SourceTime,
    result: FieldResult,
}

/// One dispatched read that has not answered yet.
#[derive(Debug, Clone)]
struct Outstanding {
    field: FieldId,
    issued_at: SourceTime,
    /// Given up at its deadline: no longer in flight, still possibly running.
    abandoned: bool,
}

/// The one answer a dispatched read owes. Sent explicitly with the read's
/// result, or — if the job is dropped unrun or the read panics — sent as a
/// failure when the guard is dropped, so no read can leave its field waiting
/// and its slot occupied for want of a reply.
struct Reply {
    sender: Sender<Completion>,
    clock: Arc<dyn MonotonicClock>,
    field: Option<FieldId>,
    sequence: u64,
}

impl Reply {
    fn send(mut self, started: SourceTime, finished: SourceTime, result: FieldResult) {
        if let Some(field) = self.field.take() {
            // The collector may be gone; its result then has no reader.
            let _ = self.sender.send(Completion {
                field,
                sequence: self.sequence,
                started,
                finished,
                result,
            });
        }
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        if let Some(field) = self.field.take() {
            let now = self.clock.now();
            let _ = self.sender.send(Completion {
                field,
                sequence: self.sequence,
                started: now,
                finished: now,
                result: Err(Unavailable::Failed),
            });
        }
    }
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
    /// Every read dispatched and not yet answered, by sequence number. Its
    /// size is bounded by `max_abandoned`.
    outstanding: HashMap<u64, Outstanding>,
    /// The one outstanding read of each field, in flight or given up, by
    /// sequence number. Independent of the field's slot, so hiding and
    /// showing a field again never lets it hold a second read.
    reading_of: HashMap<FieldId, u64>,
    in_flight: usize,
    abandoned: usize,
    sender: Sender<Completion>,
    completions: Receiver<Completion>,
}

impl<D: Dispatch> OnDemandFields<D> {
    /// Refuses [`Limits`] under which nothing could ever be dispatched or a
    /// value could never be current ([`Limits::validate`]).
    pub fn new(
        catalogue: Arc<Catalogue>,
        source: Arc<dyn OnDemandSource>,
        clock: Arc<dyn MonotonicClock>,
        dispatcher: D,
        limits: Limits,
    ) -> Result<Self, LimitsError> {
        limits.validate()?;
        let (sender, completions) = mpsc::channel();
        Ok(Self {
            catalogue,
            source,
            clock,
            dispatcher,
            limits,
            slots: HashMap::new(),
            last_sequence: 0,
            outstanding: HashMap::new(),
            reading_of: HashMap::new(),
            in_flight: 0,
            abandoned: 0,
            sender,
            completions,
        })
    }

    /// Replaces the demanded set with `wanted` — the fields of the selected or
    /// visible rows — forgets every field not in it, and dispatches reads, up
    /// to [`Limits::max_in_flight`], for demanded fields that have no read
    /// outstanding and no value younger than [`Limits::min_interval`]. Never
    /// waits for a read.
    ///
    /// A field shown again while its earlier read is still in flight adopts
    /// that read: its answer is accepted under the usual window and staleness
    /// rules, and no second read is dispatched.
    ///
    /// Dispatch is fair whatever order `wanted` is in: fields never read go
    /// first, then the field whose last read is oldest, ties in the order
    /// given. With `n` demanded fields and room for `k` reads per call, every
    /// field is read within `ceil(n / k)` calls of being demanded and then
    /// again at least once in every `ceil(n / k)` calls, as long as reads
    /// complete between calls.
    pub fn demand(&mut self, wanted: impl IntoIterator<Item = FieldId>) -> DemandReport {
        let now = self.clock.now();
        self.expire(now);
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
        // Every demanded field has a slot, so one not dispatched yet warms up.
        // (last read's time and sequence, position in `wanted`, field)
        type Due = (Option<(SourceTime, u64)>, usize, FieldId);
        let mut due: Vec<Due> = Vec::new();
        for (position, field) in order.into_iter().enumerate() {
            let slot = self
                .slots
                .entry(field.clone())
                .or_insert_with(|| Slot::new(now));
            if let Some(&sequence) = self.reading_of.get(&field) {
                // A read of this field is outstanding: never a second one. A
                // field shown again while that read is in flight waits for it.
                if slot.pending.is_none() {
                    if let Some(read) = self.outstanding.get(&sequence) {
                        if !read.abandoned {
                            slot.pending = Some(Pending {
                                sequence,
                                issued_at: read.issued_at,
                            });
                        }
                    }
                }
                continue;
            }
            let last = slot
                .reading
                .as_ref()
                .map(|reading| (reading.observed_at, reading.sequence));
            let fresh = last.is_some_and(|(at, _)| at.age_at(now) < self.limits.min_interval);
            if !fresh {
                due.push((last, position, field));
            }
        }
        // `None` sorts first: never-read fields, then the oldest reading, and
        // among readings of the same time the one dispatched first, so equal
        // ages rotate instead of favouring the front of `wanted`.
        due.sort_unstable_by_key(|due| (due.0, due.1));
        for (_, _, field) in due {
            if self.in_flight >= self.limits.max_in_flight
                || self.in_flight + self.abandoned >= self.limits.max_abandoned
            {
                break;
            }
            self.dispatch(field, now);
            report.dispatched += 1;
        }
        report
    }

    fn dispatch(&mut self, field: FieldId, now: SourceTime) {
        self.last_sequence += 1;
        let sequence = self.last_sequence;
        if let Some(slot) = self.slots.get_mut(&field) {
            slot.pending = Some(Pending {
                sequence,
                issued_at: now,
            });
        }
        self.outstanding.insert(
            sequence,
            Outstanding {
                field: field.clone(),
                issued_at: now,
                abandoned: false,
            },
        );
        self.reading_of.insert(field.clone(), sequence);
        self.in_flight += 1;
        let source = Arc::clone(&self.source);
        let clock = Arc::clone(&self.clock);
        let reply = Reply {
            sender: self.sender.clone(),
            clock: Arc::clone(&self.clock),
            field: Some(field.clone()),
            sequence,
        };
        self.dispatcher.dispatch(Box::new(move || {
            let started = clock.now();
            let result = source.read_field(&field.0, field.1);
            let finished = clock.now();
            reply.send(started, finished, result);
        }));
    }

    /// Gives up every in-flight read older than [`Limits::read_deadline`]:
    /// its field, if it is still waiting for that read, becomes failed, and
    /// its in-flight slot is free for another field. The field itself stays
    /// unreadable until the given-up read returns.
    fn expire(&mut self, now: SourceTime) {
        for (&sequence, read) in self.outstanding.iter_mut() {
            if read.abandoned || read.issued_at.age_at(now) < self.limits.read_deadline {
                continue;
            }
            read.abandoned = true;
            self.in_flight -= 1;
            self.abandoned += 1;
            if let Some(slot) = self.slots.get_mut(&read.field) {
                if slot
                    .pending
                    .is_some_and(|pending| pending.sequence == sequence)
                {
                    slot.pending = None;
                    slot.reading = Some(Reading {
                        sequence,
                        observed_at: read.issued_at,
                        result: Err(Unavailable::Failed),
                    });
                }
            }
        }
    }

    /// Takes every read that has finished, without waiting for any other,
    /// gives up reads past their deadline, and returns how many completions
    /// were accepted as the current value of their field.
    pub fn collect_completed(&mut self) -> usize {
        let mut accepted = 0;
        while let Ok(completion) = self.completions.try_recv() {
            let now = self.clock.now();
            accepted += usize::from(self.accept(completion, now));
        }
        self.expire(self.clock.now());
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
        let Some(read) = self.outstanding.remove(&completion.sequence) else {
            // Not a read this collector is waiting on.
            return false;
        };
        if self.reading_of.get(&read.field) == Some(&completion.sequence) {
            // The field's one outstanding read has answered: it may be read
            // again.
            self.reading_of.remove(&read.field);
        }
        if read.abandoned {
            // Given up at its deadline; its field already says so.
            self.abandoned -= 1;
            return false;
        }
        self.in_flight -= 1;
        let Some(slot) = self.slots.get_mut(&completion.field) else {
            // Hidden, and not shown again: the value has no reader.
            return false;
        };
        let Some(pending) = slot.pending else {
            return false;
        };
        if pending.sequence != completion.sequence {
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

    /// Whether dispatch is closed by given-up reads: there is room for
    /// another read in flight, but in-flight and given-up reads together have
    /// reached [`Limits::max_abandoned`]. Waiting behind healthy reads is
    /// not this.
    fn saturated_by_wedged_reads(&self) -> bool {
        self.abandoned > 0
            && self.in_flight < self.limits.max_in_flight
            && self.in_flight + self.abandoned >= self.limits.max_abandoned
    }

    /// The state of one field, or `None` when it is not demanded — a hidden
    /// field has no state, because it is not collected.
    ///
    /// A field without a value reads failed, not warming up, only when a read
    /// of it failed or cannot happen: its own given-up read is outstanding,
    /// or dispatch is closed by given-up reads
    /// ([`Self::saturated_by_wedged_reads`]) and it has waited
    /// [`Limits::read_deadline`] since it was demanded. A field merely queued
    /// behind healthy reads stays warming up however long it waits.
    pub fn state(&self, target: &ProcessKey, key: MetricKey) -> Option<FieldState> {
        let field = (target.clone(), key);
        let slot = self.slots.get(&field)?;
        let now = self.clock.now();
        let Some(reading) = &slot.reading else {
            let wedged = self
                .reading_of
                .get(&field)
                .and_then(|sequence| self.outstanding.get(sequence))
                .is_some_and(|read| read.abandoned);
            let starved = slot.pending.is_none()
                && self.saturated_by_wedged_reads()
                && slot.demanded_since.age_at(now) >= self.limits.read_deadline;
            return Some(if wedged || starved {
                FieldState::Failed
            } else {
                FieldState::WarmingUp
            });
        };
        let observed_at = reading.observed_at;
        Some(match &reading.result {
            Ok(value) if observed_at.age_at(now) > self.limits.max_age => FieldState::Stale {
                value: value.clone(),
                observed_at,
            },
            Ok(value) => FieldState::Present {
                value: value.clone(),
                observed_at,
            },
            Err(Unavailable::PermissionDenied) => FieldState::PermissionDenied,
            Err(Unavailable::Unsupported) => FieldState::Unsupported,
            Err(Unavailable::Failed) => FieldState::Failed,
        })
    }

    /// Reads outstanding now and not given up.
    pub fn in_flight(&self) -> usize {
        self.in_flight
    }

    /// Reads given up at their deadline that have not returned yet.
    pub fn abandoned(&self) -> usize {
        self.abandoned
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
        panic_next: std::sync::atomic::AtomicBool,
    }

    impl Scripted {
        fn new(clock: Arc<ManualClock>) -> Arc<Self> {
            Arc::new(Self {
                reads: Mutex::new(Vec::new()),
                outcome: Mutex::new(None),
                clock,
                read_takes: Mutex::new(Duration::ZERO),
                panic_next: std::sync::atomic::AtomicBool::new(false),
            })
        }

        fn reads(&self) -> Vec<FieldId> {
            self.reads.lock().unwrap().clone()
        }
    }

    impl OnDemandSource for Scripted {
        fn read_field(&self, target: &ProcessKey, key: MetricKey) -> FieldResult {
            if self.panic_next.swap(false, Ordering::SeqCst) {
                panic!("scripted read failure");
            }
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
        )
        .unwrap();
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
            read_deadline: Duration::from_secs(60),
            max_abandoned: 4,
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

    /// A field hidden while its read is outstanding, then shown again, adopts
    /// that read instead of issuing a second one, and takes its answer. A
    /// read that answers while its field is hidden is discarded, and the
    /// field is read anew when shown again.
    #[test]
    fn a_field_shown_again_adopts_its_outstanding_read() {
        let mut f = fixture(limits());
        let target = targets().remove(0);
        f.fields.demand([(target.clone(), SMAPS_LIKE)]);
        f.fields.demand([]);
        f.clock.advance(Duration::from_secs(1));
        assert_eq!(
            f.fields.demand([(target.clone(), SMAPS_LIKE)]).dispatched,
            0
        );
        assert_eq!(f.jobs.queued(), 1);
        assert_eq!(
            f.fields.state(&target, SMAPS_LIKE),
            Some(FieldState::WarmingUp)
        );
        f.jobs.take(0)();
        assert_eq!(f.fields.collect_completed(), 1);
        assert_eq!(f.fields.state(&target, SMAPS_LIKE), Some(present(1_000, 1)));
        assert_eq!(f.fields.in_flight(), 0);
        // Answered while hidden: discarded, and shown again it is read anew.
        f.clock.advance(Duration::from_secs(1));
        f.fields.demand([(target.clone(), SMAPS_LIKE)]);
        f.fields.demand([]);
        f.jobs.take(0)();
        assert_eq!(f.fields.collect_completed(), 0);
        assert_eq!(
            f.fields.demand([(target.clone(), SMAPS_LIKE)]).dispatched,
            1
        );
        f.jobs.take(0)();
        assert_eq!(f.fields.collect_completed(), 1);
        assert_eq!(f.fields.state(&target, SMAPS_LIKE), Some(present(3_000, 2)));
    }

    /// A newer reading is never replaced by an older completion: a repeated
    /// or stray completion of an earlier read, arriving after a newer read
    /// has answered, is discarded.
    #[test]
    fn an_older_completion_never_overwrites_a_newer_result() {
        let mut f = fixture(limits());
        let target = targets().remove(0);
        f.fields.demand([(target.clone(), SMAPS_LIKE)]);
        f.jobs.take(0)();
        f.fields.collect_completed();
        f.clock.advance(Duration::from_secs(1));
        f.fields.demand([(target.clone(), SMAPS_LIKE)]);
        f.jobs.take(0)();
        assert_eq!(f.fields.collect_completed(), 1);
        let newer = Some(present(2_000, 1));
        assert_eq!(f.fields.state(&target, SMAPS_LIKE), newer);
        for sequence in [1, 2] {
            f.fields
                .sender
                .send(Completion {
                    field: (target.clone(), SMAPS_LIKE),
                    sequence,
                    started: SourceTime(Duration::ZERO),
                    finished: SourceTime(Duration::ZERO),
                    result: Ok(RawValue::UnsignedInteger(7)),
                })
                .unwrap();
        }
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
        )
        .unwrap();
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

    /// Review C1: dispatch follows the oldest reading, not the caller's
    /// order, so with more demanded fields than reads in flight no field
    /// starves. Each tick is one sample: demand, every queued read runs, the
    /// loop drains, one second passes.
    #[test]
    fn every_demanded_field_is_read_within_a_bounded_number_of_ticks() {
        let wanted: Vec<FieldId> = targets()
            .into_iter()
            .flat_map(|key| [(key.clone(), SMAPS_LIKE), (key, TABLE_LIKE)])
            .collect();
        let n = wanted.len();
        for k in [1, 2, 4] {
            let mut f = fixture(Limits {
                max_in_flight: k,
                min_interval: Duration::from_secs(1),
                max_age: Duration::from_secs(60),
                ..limits()
            });
            let rounds = n.div_ceil(k);
            let tick = |f: &mut Fixture| {
                f.fields.demand(wanted.clone());
                while f.jobs.queued() > 0 {
                    f.jobs.take(0)();
                }
                f.fields.collect_completed();
                f.clock.advance(Duration::from_secs(1));
            };
            for _ in 0..rounds {
                tick(&mut f);
            }
            for field in &wanted {
                assert_ne!(
                    f.fields.state(&field.0, field.1),
                    Some(FieldState::WarmingUp),
                    "k={k}: {field:?} still warming up after {rounds} ticks"
                );
            }
            // And afterwards every field keeps being refreshed: reads stay
            // within one of each other, and no value grows older than one
            // full round.
            for _ in 0..4 * rounds {
                tick(&mut f);
                let now = f.clock.now();
                for field in &wanted {
                    let observed_at = match f.fields.state(&field.0, field.1) {
                        Some(FieldState::Present { observed_at, .. }) => observed_at,
                        other => panic!("k={k}: {field:?} is {other:?}"),
                    };
                    assert!(
                        observed_at.age_at(now) <= Duration::from_secs(rounds as u64),
                        "k={k}: {field:?} last read at {observed_at:?}, now {now:?}"
                    );
                }
            }
            let reads = f.source.reads();
            let counts: Vec<usize> = wanted
                .iter()
                .map(|field| reads.iter().filter(|read| *read == field).count())
                .collect();
            let (least, most) = (*counts.iter().min().unwrap(), *counts.iter().max().unwrap());
            assert!(most - least <= 1, "k={k}: unfair reads {counts:?}");
            assert!(least >= 4, "k={k}: {counts:?}");
        }
    }

    /// Review C2: a read that panics still answers, as a failure, and frees
    /// its slot.
    #[test]
    fn a_panicking_read_fails_its_field_and_frees_its_slot() {
        let mut f = fixture(Limits {
            max_in_flight: 1,
            ..limits()
        });
        let target = targets().remove(0);
        f.source.panic_next.store(true, Ordering::SeqCst);
        assert_eq!(
            f.fields.demand([(target.clone(), SMAPS_LIKE)]).dispatched,
            1
        );
        let job = f.jobs.take(0);
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).is_err());
        assert_eq!(f.fields.collect_completed(), 1);
        assert_eq!(
            f.fields.state(&target, SMAPS_LIKE),
            Some(FieldState::Failed)
        );
        assert_eq!(f.fields.in_flight(), 0);
        assert_eq!(
            f.fields.demand([(target.clone(), SMAPS_LIKE)]).dispatched,
            1
        );
        f.jobs.take(0)();
        f.fields.collect_completed();
        assert_eq!(
            f.fields.state(&target, SMAPS_LIKE).unwrap().availability(),
            Availability::Present
        );
    }

    /// Review C2: a job the dispatcher drops unrun (the blocking pool during
    /// shutdown) still answers, as a failure, and frees its slot.
    #[test]
    fn a_dropped_read_fails_its_field_and_frees_its_slot() {
        let mut f = fixture(Limits {
            max_in_flight: 1,
            ..limits()
        });
        let target = targets().remove(0);
        f.fields.demand([(target.clone(), SMAPS_LIKE)]);
        drop(f.jobs.take(0));
        assert_eq!(f.fields.collect_completed(), 1);
        assert_eq!(
            f.fields.state(&target, SMAPS_LIKE),
            Some(FieldState::Failed)
        );
        assert_eq!(f.fields.in_flight(), 0);
        assert!(f.source.reads().is_empty());
        assert_eq!(f.fields.demand([(target, SMAPS_LIKE)]).dispatched, 1);
    }

    /// Review C2: a read that hangs is given up at its deadline — its field
    /// fails, its slot is reclaimed for a newly shown field, and when it does
    /// return, its answer is discarded.
    #[test]
    fn a_hung_read_is_given_up_at_its_deadline() {
        let mut f = fixture(Limits {
            max_in_flight: 1,
            read_deadline: Duration::from_secs(5),
            ..limits()
        });
        let [hung, shown, _] = <[ProcessKey; 3]>::try_from(targets()).unwrap();
        f.fields.demand([(hung.clone(), SMAPS_LIKE)]);
        let late = f.jobs.take(0);
        f.clock.advance(Duration::from_secs(4));
        assert_eq!(f.fields.collect_completed(), 0);
        assert_eq!(
            f.fields.state(&hung, SMAPS_LIKE),
            Some(FieldState::WarmingUp)
        );
        let both = [(hung.clone(), SMAPS_LIKE), (shown.clone(), SMAPS_LIKE)];
        assert_eq!(f.fields.demand(both.clone()).dispatched, 0, "slot held");
        f.clock.advance(Duration::from_secs(1));
        // At the deadline: failed, slot reclaimed, and the newly shown field
        // — never read, so first in line — is dispatched.
        assert_eq!(f.fields.demand(both.clone()).dispatched, 1);
        assert_eq!(f.fields.state(&hung, SMAPS_LIKE), Some(FieldState::Failed));
        assert_eq!((f.fields.in_flight(), f.fields.abandoned()), (1, 1));
        f.jobs.take(0)();
        assert_eq!(f.fields.collect_completed(), 1);
        assert_eq!(
            f.fields.state(&shown, SMAPS_LIKE).unwrap().availability(),
            Availability::Present
        );
        // The hung read finally returns a value: discarded.
        late();
        assert_eq!(f.fields.collect_completed(), 0);
        assert_eq!(f.fields.state(&hung, SMAPS_LIKE), Some(FieldState::Failed));
        assert_eq!((f.fields.in_flight(), f.fields.abandoned()), (0, 0));
    }

    /// Given-up reads may still hold a worker each, so they are bounded too:
    /// at the bound nothing new is dispatched until one returns.
    #[test]
    fn given_up_reads_are_bounded() {
        let mut f = fixture(Limits {
            max_in_flight: 1,
            max_abandoned: 1,
            read_deadline: Duration::from_secs(5),
            ..limits()
        });
        let [first, second, _] = <[ProcessKey; 3]>::try_from(targets()).unwrap();
        let both = [(first.clone(), SMAPS_LIKE), (second.clone(), SMAPS_LIKE)];
        f.fields.demand(both.clone());
        let late = f.jobs.take(0);
        f.clock.advance(Duration::from_secs(5));
        assert_eq!(f.fields.demand(both.clone()).dispatched, 0);
        assert_eq!((f.fields.in_flight(), f.fields.abandoned()), (0, 1));
        late();
        assert_eq!(f.fields.collect_completed(), 0);
        assert_eq!(f.fields.demand(both).dispatched, 1);
        // Dispatched now, so warming up again: it is waiting for a read, not
        // for a closed dispatch.
        assert_eq!(
            f.fields.state(&second, SMAPS_LIKE),
            Some(FieldState::WarmingUp)
        );
        assert_eq!(f.fields.state(&first, SMAPS_LIKE), Some(FieldState::Failed));
        f.jobs.take(0)();
        f.fields.collect_completed();
        assert_eq!(
            f.fields.state(&second, SMAPS_LIKE).unwrap().availability(),
            Availability::Present
        );
    }

    /// Runs every queued job in dispatch order except those whose target is
    /// in `wedged`, which are kept unrun — reads that never return. Each
    /// queued job is matched to its read through the dispatch sequence.
    fn run_all_but(
        f: &mut Fixture,
        wedged: &[&ProcessKey],
        seen: &mut u64,
        kept: &mut Vec<(FieldId, Job)>,
    ) {
        let mut fresh: Vec<(u64, FieldId)> = f
            .fields
            .outstanding
            .iter()
            .filter(|(sequence, _)| **sequence > *seen)
            .map(|(sequence, read)| (*sequence, read.field.clone()))
            .collect();
        fresh.sort_unstable_by_key(|read| read.0);
        assert_eq!(fresh.len(), f.jobs.queued());
        for (sequence, field) in fresh {
            *seen = sequence;
            let job = f.jobs.take(0);
            if wedged.contains(&&field.0) {
                kept.push((field, job));
            } else {
                job();
            }
        }
    }

    /// Outstanding reads of one field, in flight or given up.
    fn outstanding_of(f: &Fixture, field: &FieldId) -> usize {
        f.fields
            .outstanding
            .values()
            .filter(|read| &read.field == field)
            .count()
    }

    /// Review round 3, B2: a wedged row at the viewport's edge, demanded on
    /// even ticks and hidden on odd ones, never holds more than one read, so
    /// it never takes the other fields' slots: they are read on every tick,
    /// and when its one read is given up, at most one read is wedged.
    #[test]
    fn a_flickering_wedged_field_holds_one_read() {
        let mut f = fixture(Limits {
            max_in_flight: 4,
            max_abandoned: 16,
            read_deadline: Duration::from_secs(30),
            min_interval: Duration::from_secs(1),
            max_age: Duration::from_secs(10),
            ..limits()
        });
        let [edge, other, _] = <[ProcessKey; 3]>::try_from(targets()).unwrap();
        let wedged_field = (edge.clone(), SMAPS_LIKE);
        let (mut seen, mut kept) = (0, Vec::new());
        for tick in 0..200u64 {
            let mut wanted = vec![(other.clone(), SMAPS_LIKE)];
            if tick % 2 == 0 {
                wanted.push(wedged_field.clone());
            }
            f.fields.demand(wanted);
            run_all_but(&mut f, &[&edge], &mut seen, &mut kept);
            f.fields.collect_completed();
            assert!(outstanding_of(&f, &wedged_field) <= 1, "tick {tick}");
            assert!(f.fields.abandoned() <= 1, "tick {tick}");
            match f.fields.state(&other, SMAPS_LIKE) {
                Some(FieldState::Present { observed_at, .. }) => {
                    assert_eq!(observed_at, f.clock.now(), "tick {tick}: read this tick")
                }
                state => panic!("tick {tick}: {state:?}"),
            }
            if tick % 2 == 0 {
                let expected = if tick >= 30 {
                    FieldState::Failed
                } else {
                    FieldState::WarmingUp
                };
                assert_eq!(
                    f.fields.state(&edge, SMAPS_LIKE),
                    Some(expected),
                    "tick {tick}"
                );
            }
            f.clock.advance(Duration::from_secs(1));
        }
        assert_eq!(kept.len(), 1, "the flickering field was dispatched once");
        assert_eq!(f.fields.abandoned(), 1);
    }

    /// Review round 3, N3: a healthy field merely queued behind healthy reads
    /// stays warming up past the read deadline; it reads failed only when a
    /// read failed or dispatch is closed by given-up reads (the saturation
    /// test covers that side).
    #[test]
    fn a_field_queued_behind_healthy_reads_keeps_warming_up() {
        let mut f = fixture(Limits {
            max_in_flight: 1,
            max_abandoned: 1,
            read_deadline: Duration::from_secs(3),
            min_interval: Duration::from_secs(1),
            max_age: Duration::from_secs(60),
            ..limits()
        });
        let wanted: Vec<FieldId> = targets()
            .into_iter()
            .flat_map(|key| [(key.clone(), SMAPS_LIKE), (key, TABLE_LIKE)])
            .collect();
        let (mut seen, mut kept) = (0, Vec::new());
        let mut warmed_past_deadline = false;
        for tick in 0..wanted.len() {
            f.fields.demand(wanted.clone());
            run_all_but(&mut f, &[], &mut seen, &mut kept);
            f.fields.collect_completed();
            for field in &wanted {
                let state = f.fields.state(&field.0, field.1).unwrap();
                assert_ne!(state, FieldState::Failed, "tick {tick}: {field:?}");
                warmed_past_deadline |= tick >= 3 && state == FieldState::WarmingUp;
            }
            f.clock.advance(Duration::from_secs(1));
        }
        assert!(warmed_past_deadline, "some field waited past the deadline");
        for field in &wanted {
            assert_eq!(
                f.fields.state(&field.0, field.1).unwrap().availability(),
                Availability::Present
            );
        }
    }

    /// Review round 3, N4: limits under which nothing could be dispatched,
    /// nothing could be current or no read could be given up are refused.
    #[test]
    fn unusable_limits_are_refused() {
        assert_eq!(Limits::default().validate(), Ok(()));
        assert_eq!(limits().validate(), Ok(()));
        for (limits, expected) in [
            (
                Limits {
                    max_fields: 0,
                    ..limits()
                },
                LimitsError::Zero("max_fields"),
            ),
            (
                Limits {
                    max_in_flight: 0,
                    ..limits()
                },
                LimitsError::Zero("max_in_flight"),
            ),
            (
                Limits {
                    max_abandoned: 0,
                    ..limits()
                },
                LimitsError::Zero("max_abandoned"),
            ),
            (
                Limits {
                    max_age: Duration::ZERO,
                    ..limits()
                },
                LimitsError::Zero("max_age"),
            ),
            (
                Limits {
                    read_deadline: Duration::ZERO,
                    ..limits()
                },
                LimitsError::Zero("read_deadline"),
            ),
            (
                Limits {
                    max_in_flight: 5,
                    max_abandoned: 4,
                    ..limits()
                },
                LimitsError::AbandonedBelowInFlight {
                    max_in_flight: 5,
                    max_abandoned: 4,
                },
            ),
        ] {
            assert_eq!(limits.validate(), Err(expected));
            let clock = Arc::new(ManualClock::default());
            let refused = OnDemandFields::new(
                catalogue(),
                Scripted::new(Arc::clone(&clock)) as Arc<dyn OnDemandSource>,
                clock as Arc<dyn MonotonicClock>,
                Manual::default(),
                limits,
            );
            assert_eq!(refused.err(), Some(expected));
        }
        // A zero re-read interval is allowed: re-read once the last answers.
        assert_eq!(
            Limits {
                min_interval: Duration::ZERO,
                ..Limits::default()
            }
            .validate(),
            Ok(())
        );
    }

    /// Review round 2, N1: reads in flight and given up together never
    /// exceed `max_abandoned`, so the given-up count cannot overshoot it when
    /// in-flight reads later expire.
    #[test]
    fn running_reads_never_exceed_the_bound() {
        let mut f = fixture(Limits {
            max_in_flight: 2,
            max_abandoned: 2,
            read_deadline: Duration::from_secs(5),
            ..limits()
        });
        let [a, b, c] = <[ProcessKey; 3]>::try_from(targets()).unwrap();
        let all = [a, b, c].map(|key| (key, SMAPS_LIKE));
        let (mut seen, mut kept) = (0, Vec::new());
        // One field wedges first, so one read is already given up when the
        // others are demanded: only one more may be dispatched.
        for tick in 0..20 {
            let wanted = if tick < 3 { &all[..1] } else { &all[..] };
            f.fields.demand(wanted.to_vec());
            let wedged: Vec<&ProcessKey> = all.iter().map(|field| &field.0).collect();
            run_all_but(&mut f, &wedged, &mut seen, &mut kept);
            f.fields.collect_completed();
            assert!(f.fields.in_flight() + f.fields.abandoned() <= 2);
            assert!(f.fields.outstanding.len() <= 2);
            f.clock.advance(Duration::from_secs(3));
        }
        assert_eq!(f.fields.abandoned(), 2);
        assert_eq!(kept.len(), 2);
    }

    /// Review round 2, B1: one field whose read never returns. It is given up
    /// once, stays failed, and is never dispatched again while that read is
    /// out — not after a deadline, not after being hidden and shown again —
    /// while every other field keeps being read and a newly shown field gets
    /// a value. When the wedged read finally returns, the field is read again.
    #[test]
    fn one_wedged_field_never_stops_the_others() {
        let mut f = fixture(Limits {
            max_in_flight: 2,
            max_abandoned: 3,
            read_deadline: Duration::from_secs(5),
            min_interval: Duration::from_secs(1),
            max_age: Duration::from_secs(10),
            ..limits()
        });
        let [hung, other, third] = <[ProcessKey; 3]>::try_from(targets()).unwrap();
        let (mut seen, mut kept) = (0, Vec::new());
        for tick in 0..1_000u64 {
            let mut wanted = Vec::new();
            // The wedged row is hidden for a while and shown again.
            if !(300..400).contains(&tick) {
                wanted.push((hung.clone(), SMAPS_LIKE));
            }
            wanted.push((other.clone(), SMAPS_LIKE));
            if tick >= 500 {
                wanted.push((third.clone(), SMAPS_LIKE));
            }
            f.fields.demand(wanted);
            run_all_but(&mut f, &[&hung], &mut seen, &mut kept);
            f.fields.collect_completed();
            if tick >= 5 && !(300..400).contains(&tick) {
                assert_eq!(
                    f.fields.state(&hung, SMAPS_LIKE),
                    Some(FieldState::Failed),
                    "tick {tick}"
                );
            }
            assert_eq!(
                f.fields.state(&other, SMAPS_LIKE).unwrap().availability(),
                Availability::Present,
                "tick {tick}"
            );
            if tick >= 500 {
                assert_eq!(
                    f.fields.state(&third, SMAPS_LIKE).unwrap().availability(),
                    Availability::Present,
                    "tick {tick}"
                );
            }
            assert!(f.fields.abandoned() <= 1, "tick {tick}");
            f.clock.advance(Duration::from_secs(1));
        }
        assert_eq!(kept.len(), 1, "the wedged field was dispatched once");
        assert_eq!(f.fields.abandoned(), 1);
        assert!(f.source.reads().iter().all(|read| read.0 != hung));
        // The wedged read returns at last: discarded, and the field is due.
        let (field, late) = kept.pop().unwrap();
        assert_eq!(field, (hung.clone(), SMAPS_LIKE));
        late();
        assert_eq!(f.fields.collect_completed(), 0);
        assert_eq!(f.fields.abandoned(), 0);
        f.fields.demand([(hung.clone(), SMAPS_LIKE)]);
        f.jobs.take(0)();
        f.fields.collect_completed();
        assert_eq!(
            f.fields.state(&hung, SMAPS_LIKE).unwrap().availability(),
            Availability::Present
        );
    }

    /// Review round 2, B1(b): as many distinct wedged fields as the bound
    /// allows stop every dispatch. Nothing then sits warming up forever: a
    /// field without a value reads failed once it has waited a deadline, one
    /// with a value turns stale, and collection resumes by itself as soon as
    /// any wedged read returns.
    #[test]
    fn saturation_by_wedged_fields_is_reported_and_recovers() {
        let mut f = fixture(Limits {
            max_in_flight: 2,
            max_abandoned: 2,
            read_deadline: Duration::from_secs(5),
            min_interval: Duration::from_secs(1),
            max_age: Duration::from_secs(10),
            ..limits()
        });
        let [first, second, shown] = <[ProcessKey; 3]>::try_from(targets()).unwrap();
        let (mut seen, mut kept) = (0, Vec::new());
        let all = [
            (first.clone(), SMAPS_LIKE),
            (second.clone(), SMAPS_LIKE),
            (first.clone(), TABLE_LIKE),
        ];
        // One field gets a value first; then two others wedge.
        f.fields.demand([all[2].clone()]);
        run_all_but(&mut f, &[], &mut seen, &mut kept);
        f.fields.collect_completed();
        f.clock.advance(Duration::from_secs(1));
        f.fields.demand(all.clone());
        run_all_but(&mut f, &[&first, &second], &mut seen, &mut kept);
        // Both slots went to the never-read fields, which wedged.
        assert_eq!(kept.len(), 2);
        let mut with_shown = all.to_vec();
        with_shown.push((shown.clone(), SMAPS_LIKE));
        for tick in 0..30 {
            f.clock.advance(Duration::from_secs(1));
            let report = f.fields.demand(with_shown.clone());
            run_all_but(&mut f, &[&first, &second], &mut seen, &mut kept);
            f.fields.collect_completed();
            if tick >= 4 {
                assert_eq!(report.dispatched, 0, "tick {tick}: saturated");
                assert_eq!(f.fields.abandoned(), 2);
            }
            if tick >= 5 {
                assert_eq!(
                    f.fields.state(&shown, SMAPS_LIKE),
                    Some(FieldState::Failed),
                    "tick {tick}: waited past the deadline"
                );
            }
        }
        assert_eq!(kept.len(), 2, "no field was dispatched twice");
        assert_eq!(
            f.fields.state(&first, TABLE_LIKE).unwrap().availability(),
            Availability::Stale
        );
        // One wedged read returns: collection resumes on the next demand.
        let (_, late) = kept.remove(0);
        late();
        f.fields.collect_completed();
        assert_eq!(f.fields.abandoned(), 1);
        f.clock.advance(Duration::from_secs(1));
        assert_eq!(f.fields.demand(with_shown.clone()).dispatched, 1);
        run_all_but(&mut f, &[&first, &second], &mut seen, &mut kept);
        f.fields.collect_completed();
        assert_eq!(
            f.fields.state(&shown, SMAPS_LIKE).unwrap().availability(),
            Availability::Present,
            "the never-read field is served first"
        );
    }

    /// Review C3: freshness time counts a suspend (the wall clock jumps while
    /// the monotonic clock stood still), never stops or reverses when the wall
    /// clock is set back, and errs towards stale when it is set forward.
    #[test]
    fn freshness_time_counts_a_suspend_and_never_runs_backwards() {
        let s = Duration::from_secs;
        let mut tracker = ElapsedTracker::new(Some(s(1_000)));
        let mut step =
            |monotonic: u64, wall: Option<u64>| tracker.advance(s(monotonic), wall.map(s)).0;
        assert_eq!(step(1, Some(1_001)), s(1));
        // An hour suspended: the monotonic clock moved one second.
        assert_eq!(step(2, Some(4_601)), s(3_601));
        // The wall clock set back: time still advances by the monotonic step.
        assert_eq!(step(3, Some(100)), s(3_602));
        assert_eq!(step(4, Some(101)), s(3_603));
        // Set forward: counted, so values age early, never late.
        assert_eq!(step(5, Some(1_101)), s(4_603));
        // An unreadable wall clock counts the monotonic step only.
        assert_eq!(step(6, None), s(4_604));
        assert_eq!(step(7, Some(1_102)), s(4_605));
        // Neither clock moved.
        assert_eq!(step(7, Some(1_102)), s(4_605));
        let clock = SuspendAwareClock::new();
        let (a, b) = (clock.now(), clock.now());
        assert!(a <= b);
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
        )
        .unwrap();
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
