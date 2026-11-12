//! Last-observed records of processes that left the collection (PX-021;
//! design §§6.2, 20.4, 26; D2 T21).
//!
//! When a *watched* process instance leaves the server-side collection, a
//! bounded tombstone keeps what was last observed of it: its whole identity
//! ([`ProcessKey`]), its last row (name, owner, and every published cell,
//! among them its last CPU share and resident memory), the state, parent and
//! start time the inspector last read for it, the time of the last scan that
//! listed it, and why it left. Its command line, executable and working
//! directory are not kept: arguments can carry secrets, and PX-014 shows them
//! only while the process is listed. Watched means one of three things the user chose, and nothing
//! else: the inspector's target (PX-013), the followed process (PX-017), and
//! a tagged process (PX-020). A historical tag *is* a tagged tombstone: the
//! tags status names the tagged ones this store keeps, so a tag and the
//! inspector can never disagree about how a process left.
//!
//! A tombstone says one of two things, and only what a scan established:
//!
//! * **Exited** — positively established. On Linux, `/proc/<pid>` exists from
//!   the moment a process is created until it is reaped, and a zombie keeps its
//!   directory (`proc(5)`: "a numerical subdirectory for each running
//!   process"; a zombie is listed with state `Z`, `proc_pid_stat(5)`). So an
//!   instance has exited, and been reaped, when a scan that read the process
//!   list and accounted for every entry it did not publish
//!   ([`Retention::Skipped`] without its PID) no longer lists its PID, or lists
//!   it with another start time (field 22 of `/proc/<pid>/stat`, the instance's
//!   creation token): a PID is only reused once its previous holder is gone. A
//!   record that vanished between listing and reading (`ENOENT`/`ESRCH`) is
//!   the same absence. The inspector's detail read establishes it too when it
//!   finds the record gone or its PID naming another start time
//!   ([`crate::inspector::DetailRead::Ended`]). A thread exited when its
//!   process's `task` listing was read and no longer lists its TID, or lists
//!   it with another start time, or when its process exited.
//! * **No longer observed** — the row left but no scan established an exit: it
//!   was kept while scans could not account for it (an unreadable or capped
//!   record, an unenumerable listing: [`Retention`]) and the row limit then
//!   dropped it; or the scan's host, boot or PID namespace is not known to be
//!   the one the instance was listed in, so its listing proves nothing about
//!   it; or, for a thread, the sample read no threads.
//!
//! Nothing else is claimed: no exit status, no cause, no signal and no
//! history between samples. Fixture and synthetic sources state their own
//! completeness, so their absences are exits by their own statement, under the
//! same rule; a source that cannot account for absences reports itself
//! unenumerable and never produces an exit. srtop has no non-Linux live
//! collector.
//!
//! A scan that could not read the process list, or that this app refused,
//! keeps every row ([`Retention::Unenumerable`]), so it records no tombstone at
//! all: an inaccessible scan never turns any process into a historical one.
//!
//! Retention is bounded three ways (§20.4): at most [`MAX_TOMBSTONES`] records,
//! each kept at most [`MAX_TOMBSTONE_AGE`] after it left by the injected
//! monotonic clock, and at most [`MAX_TOMBSTONE_BYTES`] of estimated content,
//! the oldest dropped first; a record no longer watched — the inspector moved
//! on, follow stopped, the tag was cleared — is dropped at the next sample.
//! A tombstone is never an action target: it changes no process, and every
//! request that acts on the selection refuses it.

use crate::command::CommandContext;
use crate::inspector::ProcessDetails;
use crate::metric::format_utc;
use crate::projection::Row;
use crate::source::{
    MissingReason, Observed, ProcessKey, ProcessSnapshot, Retention, ThreadRetention,
};
use std::cell::OnceCell;
use std::collections::{HashSet, VecDeque};
use std::time::{Duration, SystemTime};

/// The most tombstones kept; the oldest is dropped first.
pub const MAX_TOMBSTONES: usize = 64;
/// How long a tombstone is kept after its process left, by the view's
/// injected monotonic clock. It is dropped by the first sample at or after
/// this age.
pub const MAX_TOMBSTONE_AGE: Duration = Duration::from_secs(600);
/// The most estimated bytes all tombstones hold ([`Tombstone::bytes`]); the
/// oldest are dropped first.
pub const MAX_TOMBSTONE_BYTES: usize = 32 * 1024;
/// What one tombstone costs beyond its row, its identity strings and its
/// details.
pub const TOMBSTONE_OVERHEAD_BYTES: usize = 256;
/// What a tombstone's read state, parent and start time cost.
const DETAILS_BYTES: usize = 128;

/// How an exit was established.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// A scan that accounts for every entry it did not publish no longer
    /// lists the PID (or, for a thread, its process's `task` no longer lists
    /// the TID).
    Unlisted,
    /// The PID (or TID) now names another instance in the same host, boot and
    /// PID namespace.
    Replaced,
    /// The inspector's detail read found its record gone or naming another
    /// instance.
    DetailRead,
    /// A thread whose process exited.
    WithProcess,
}

/// Why a departure is not an established exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unobserved {
    /// Kept while scans could not account for it, then dropped at the row
    /// limit.
    Evicted,
    /// The scan's host, boot or PID namespace is not known to be the one it
    /// was listed in.
    IdentityUnconfirmed,
    /// A thread, when the sample read no threads.
    ThreadsNotRead,
    /// A thread whose process is no longer observed.
    WithProcess,
}

/// Why a watched instance left the collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Departure {
    Exited(Exit),
    NotObserved(Unobserved),
}

impl Departure {
    /// Whether an exit was positively established.
    pub fn exited(self) -> bool {
        matches!(self, Self::Exited(_))
    }

    /// The verdict alone: `exited` or `no longer observed`.
    pub fn verdict(self) -> &'static str {
        match self {
            Self::Exited(_) => "exited",
            Self::NotObserved(_) => "no longer observed",
        }
    }

    /// The verdict after a subject: `exited` or `is no longer observed`.
    pub fn predicate(self) -> &'static str {
        match self {
            Self::Exited(_) => "exited",
            Self::NotObserved(_) => "is no longer observed",
        }
    }

    /// The verdict and what established it, for a `noun` (process or thread).
    pub fn explained(self, noun: &str) -> String {
        let unknown = "whether it exited is unknown";
        match self {
            Self::Exited(Exit::Unlisted) if noun == "thread" => {
                "exited: a scan that read its process's threads no longer lists it".to_string()
            }
            Self::Exited(Exit::Unlisted) => {
                "exited: a scan that accounts for its PID no longer lists it".to_string()
            }
            Self::Exited(Exit::Replaced) if noun == "thread" => {
                "exited: its TID now names a newer thread".to_string()
            }
            Self::Exited(Exit::Replaced) => "exited: its PID now names a newer process".to_string(),
            Self::Exited(Exit::DetailRead) => {
                "exited: its record was gone when its details were read".to_string()
            }
            Self::Exited(Exit::WithProcess) => "exited: its process exited".to_string(),
            Self::NotObserved(Unobserved::Evicted) => format!(
                "no longer observed: kept while scans could not account for it, then left out at the row limit; {unknown}"
            ),
            Self::NotObserved(Unobserved::IdentityUnconfirmed) => format!(
                "no longer observed: the scan could not confirm the host, boot and PID namespace it was listed in; {unknown}"
            ),
            Self::NotObserved(Unobserved::ThreadsNotRead) => {
                format!("no longer observed: the sample read no threads; {unknown}")
            }
            Self::NotObserved(Unobserved::WithProcess) => {
                format!("no longer observed: its process is no longer observed; {unknown}")
            }
        }
    }
}

/// What one scan establishes about the instances that left the collection
/// with it (PX-021).
pub(crate) struct Evidence<'a> {
    pub(crate) snapshot: &'a ProcessSnapshot,
    pub(crate) retention: &'a Retention,
    pub(crate) thread_retention: &'a ThreadRetention,
    /// The process instances this scan confirmed.
    pub(crate) confirmed: &'a HashSet<ProcessKey>,
    /// The PIDs this scan listed, built once, and only if something left.
    listed: OnceCell<HashSet<u32>>,
}

impl<'a> Evidence<'a> {
    pub(crate) fn new(
        snapshot: &'a ProcessSnapshot,
        retention: &'a Retention,
        thread_retention: &'a ThreadRetention,
        confirmed: &'a HashSet<ProcessKey>,
    ) -> Self {
        Self {
            snapshot,
            retention,
            thread_retention,
            confirmed,
            listed: OnceCell::new(),
        }
    }

    /// Why `key`, a row this scan neither confirmed nor kept, left.
    pub(crate) fn departure(&self, key: &ProcessKey) -> Departure {
        let Some(thread) = &key.thread else {
            return self.process(key);
        };
        let process = key.process_key();
        if !self.confirmed.contains(&process) {
            return match self.process(&process) {
                Departure::Exited(_) => Departure::Exited(Exit::WithProcess),
                Departure::NotObserved(_) => Departure::NotObserved(Unobserved::WithProcess),
            };
        }
        if self.snapshot.threads.read().is_none() {
            return Departure::NotObserved(Unobserved::ThreadsNotRead);
        }
        // A TID this scan read under the same process instance names a newer
        // thread: the earlier one is gone, whatever else of that process's
        // threads the scan could not read (merge with main: PX-019's
        // superseded-TID rule, which never retains such a row).
        let replaced = self.snapshot.threads.records().iter().any(|record| {
            record.key.is_thread_of(&process)
                && record
                    .key
                    .thread
                    .as_ref()
                    .is_some_and(|other| other.tid == thread.tid)
        });
        if replaced {
            return Departure::Exited(Exit::Replaced);
        }
        if self.thread_retention.covers(&key.pid) {
            return Departure::NotObserved(Unobserved::Evicted);
        }
        Departure::Exited(Exit::Unlisted)
    }

    fn process(&self, key: &ProcessKey) -> Departure {
        if self.retention.keeps(key) {
            return Departure::NotObserved(Unobserved::Evicted);
        }
        let Observed::Known(pid) = key.pid else {
            return Departure::NotObserved(Unobserved::IdentityUnconfirmed);
        };
        if !self.same_namespace(key) {
            return Departure::NotObserved(Unobserved::IdentityUnconfirmed);
        }
        let listed = self.listed.get_or_init(|| {
            self.snapshot
                .records
                .iter()
                .filter_map(|record| match record.key.pid {
                    Observed::Known(pid) => Some(pid),
                    Observed::Missing(_) => None,
                })
                .collect()
        });
        Departure::Exited(if listed.contains(&pid) {
            Exit::Replaced
        } else {
            Exit::Unlisted
        })
    }

    /// Whether this scan listed the PID namespace `key` was listed in: the
    /// same source, and a known host, boot and PID namespace equal to those of
    /// the scan's records. A scan with no records, or whose identity could not
    /// be read, says nothing about any instance.
    fn same_namespace(&self, key: &ProcessKey) -> bool {
        let Some(first) = self.snapshot.records.first() else {
            return false;
        };
        let scan = &first.key;
        key.source == self.snapshot.source
            && key.source == scan.source
            && matches!((&key.host, &scan.host), (Observed::Known(a), Observed::Known(b)) if a == b)
            && matches!((&key.boot, &scan.boot), (Observed::Known(a), Observed::Known(b)) if a == b)
            && matches!(
                (&key.pid_namespace, &scan.pid_namespace),
                (Observed::Known(a), Observed::Known(b)) if a == b
            )
    }
}

/// What was last observed of one watched instance that left the collection.
#[derive(Debug, Clone)]
pub struct Tombstone {
    /// The row as the server last held it: its identity, name, owner and
    /// every cell, among them its last CPU share and resident memory.
    pub(crate) row: Row,
    /// The state, parent and start time the inspector last read for it, if
    /// it read any. Its command line, executable and working directory are
    /// not kept: arguments can carry secrets, and PX-014 shows them only for
    /// a listed process.
    pub(crate) details: Option<ProcessDetails>,
    /// The sample time of the last scan that listed it, if known.
    pub(crate) last_observed: Option<SystemTime>,
    /// The sample time of the scan it left with.
    pub(crate) left_at: SystemTime,
    /// When it left, by the view's monotonic clock: what its age is measured
    /// from.
    pub(crate) left_clock: Duration,
    pub(crate) departure: Departure,
    /// Whether it is a historical tag (PX-020).
    pub(crate) tagged: bool,
    bytes: usize,
}

impl Tombstone {
    pub(crate) fn new(
        row: Row,
        details: Option<ProcessDetails>,
        last_observed: Option<SystemTime>,
        left_at: SystemTime,
        left_clock: Duration,
        departure: Departure,
    ) -> Self {
        let details = details.map(|mut details| {
            details.command = CommandContext::missing(MissingReason::Unavailable);
            details
        });
        let key = &row.key;
        let identity = key.source.0.len()
            + match &key.host {
                Observed::Known(host) => host.0.len(),
                Observed::Missing(_) => 0,
            }
            + match &key.boot {
                Observed::Known(boot) => boot.0.len(),
                Observed::Missing(_) => 0,
            }
            + match &key.creation {
                crate::source::CreationToken::Opaque(token) => token.len(),
                crate::source::CreationToken::LinuxBootTicks(_) => 8,
            };
        let bytes = TOMBSTONE_OVERHEAD_BYTES
            + identity
            + crate::refresh::row_wire_bytes(&row)
            + row.display_name.as_str().len()
            + details.as_ref().map_or(0, |_| DETAILS_BYTES);
        Self {
            row,
            details,
            last_observed,
            left_at,
            left_clock,
            departure,
            tagged: false,
            bytes,
        }
    }

    pub fn key(&self) -> &ProcessKey {
        &self.row.key
    }

    pub fn departure(&self) -> Departure {
        self.departure
    }

    pub fn last_observed(&self) -> Option<SystemTime> {
        self.last_observed
    }

    /// The sample time of the scan it left with.
    pub fn left_at(&self) -> SystemTime {
        self.left_at
    }

    pub fn is_tagged(&self) -> bool {
        self.tagged
    }

    /// How it is named: its PID or TID and its name, as last shown.
    pub fn subject(&self) -> String {
        crate::tags::who(&self.row)
    }

    /// What this record is charged against [`MAX_TOMBSTONE_BYTES`]: a fixed
    /// overhead, its identity strings, its row as it would be published and
    /// its name, and a fixed charge for read details.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// `last observed <UTC time>`, or that it is not known.
    pub(crate) fn observed_text(&self) -> String {
        match self.last_observed.and_then(format_utc) {
            Some(at) => format!("last observed {at}"),
            None => "last observed: time unknown".to_string(),
        }
    }
}

/// The tombstones of watched instances that left the collection, oldest
/// first, bounded by [`MAX_TOMBSTONES`], [`MAX_TOMBSTONE_AGE`] and
/// [`MAX_TOMBSTONE_BYTES`].
#[derive(Debug, Clone, Default)]
pub struct LastSeen {
    entries: VecDeque<Tombstone>,
    bytes: usize,
    /// Historical tags dropped by a bound since tags were last cleared.
    forgotten_tags: u64,
    /// Tombstones dropped by a bound, ever.
    dropped: u64,
}

impl LastSeen {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The estimated bytes held ([`Tombstone::bytes`]).
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Tombstones dropped by the count, age or byte bound.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// The tombstones kept, oldest first.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Tombstone> {
        self.entries.iter()
    }

    pub fn get(&self, key: &ProcessKey) -> Option<&Tombstone> {
        self.entries.iter().find(|tombstone| tombstone.key() == key)
    }

    /// The historical tags kept, oldest first.
    pub fn tagged(&self) -> impl DoubleEndedIterator<Item = &Tombstone> {
        self.entries.iter().filter(|tombstone| tombstone.tagged)
    }

    /// Historical tags dropped by a bound since tags were last cleared.
    pub fn forgotten_tags(&self) -> u64 {
        self.forgotten_tags
    }

    /// Keeps `tombstone`, the newest, replacing any earlier record of the
    /// same instance: a second departure means the collection held it again
    /// in between, so only the latest departure's sample, time and verdict
    /// are true. Then drops the oldest beyond the count and byte bounds.
    pub(crate) fn record(&mut self, tombstone: Tombstone) {
        if let Some(at) = self
            .entries
            .iter()
            .position(|kept| kept.key() == tombstone.key())
        {
            if let Some(earlier) = self.entries.remove(at) {
                self.bytes -= earlier.bytes;
            }
        }
        self.bytes += tombstone.bytes;
        self.entries.push_back(tombstone);
        while self.entries.len() > MAX_TOMBSTONES || self.bytes > MAX_TOMBSTONE_BYTES {
            self.drop_oldest();
        }
    }

    fn drop_oldest(&mut self) {
        if let Some(oldest) = self.entries.pop_front() {
            self.bytes -= oldest.bytes;
            self.dropped += 1;
            if oldest.tagged {
                self.forgotten_tags += 1;
            }
        }
    }

    /// Drops, and returns, every tombstone that said only "no longer
    /// observed" of an instance a scan lists again (`listed`): it was
    /// observed after all, so its last-observed time and verdict are no
    /// longer true. A historical tag returned so is not forgotten: the caller
    /// makes it live again, since it is the same instance under the same
    /// whole key. An established exit is final: the kernel never reuses a PID
    /// with the same start time within a boot, so only a source that reuses
    /// its own tokens lists an exited instance again, and that record stays.
    pub(crate) fn drop_relisted(&mut self, listed: impl Fn(&ProcessKey) -> bool) -> Vec<Tombstone> {
        let mut kept = VecDeque::with_capacity(self.entries.len());
        let mut dropped = Vec::new();
        for tombstone in std::mem::take(&mut self.entries) {
            if !tombstone.departure.exited() && listed(tombstone.key()) {
                self.bytes -= tombstone.bytes;
                dropped.push(tombstone);
            } else {
                kept.push_back(tombstone);
            }
        }
        self.entries = kept;
        dropped
    }

    /// Upgrades to `exited` every record that says only "no longer observed"
    /// of an instance whose exit a later scan establishes: `exit(key)` is that
    /// scan's verdict on the absent instance, or `None` when the scan cannot
    /// account for it (merge with main: PX-013's review round ends a row the
    /// row limit dropped once a scan entitled to account for it no longer
    /// lists it). The record keeps its row, details and last-observed time:
    /// only the verdict was unknown. The caller has already dropped the
    /// records of instances this scan lists again ([`Self::drop_relisted`]),
    /// so every record asked about is absent from it. An established exit is
    /// final and is never asked about again.
    pub(crate) fn establish_exits(&mut self, exit: impl Fn(&ProcessKey) -> Option<Exit>) {
        for tombstone in &mut self.entries {
            if tombstone.departure.exited() {
                continue;
            }
            if let Some(exit) = exit(tombstone.key()) {
                tombstone.departure = Departure::Exited(exit);
            }
        }
    }

    /// The inspector's detail read found the record of `key`, this exact
    /// instance (the whole [`ProcessKey`]), gone after it had already left
    /// the collection: a read still loading when a scan recorded its
    /// departure. A record that says only "no longer observed" turns
    /// `exited` through the detail read; an established exit keeps the
    /// reason first established. Says whether a record changed.
    pub(crate) fn establish_detail_exit(&mut self, key: &ProcessKey) -> bool {
        match self.entries.iter_mut().find(|kept| kept.key() == key) {
            Some(kept) if !kept.departure.exited() => {
                kept.departure = Departure::Exited(Exit::DetailRead);
                true
            }
            _ => false,
        }
    }

    /// Counts `count` historical tags forgotten without a bound dropping
    /// their records: their records were dropped for another reason.
    pub(crate) fn forget_tags(&mut self, count: u64) {
        self.forgotten_tags += count;
    }

    /// Makes the tombstone of `key` a historical tag, if one is kept, and
    /// says whether it was.
    pub(crate) fn tag(&mut self, key: &ProcessKey) -> bool {
        match self.entries.iter_mut().find(|kept| kept.key() == key) {
            Some(kept) => {
                kept.tagged = true;
                true
            }
            None => false,
        }
    }

    /// Makes the tombstone of `key` no historical tag: its tag is live again.
    pub(crate) fn untag(&mut self, key: &ProcessKey) {
        if let Some(kept) = self.entries.iter_mut().find(|kept| kept.key() == key) {
            kept.tagged = false;
        }
    }

    /// Removes every historical tag, and says how many there were, counting
    /// the forgotten ones. The tombstones stay while something else watches
    /// them.
    pub(crate) fn untag_all(&mut self) -> u64 {
        let mut cleared = self.forgotten_tags;
        for kept in &mut self.entries {
            if kept.tagged {
                kept.tagged = false;
                cleared += 1;
            }
        }
        self.forgotten_tags = 0;
        cleared
    }

    /// Drops every tombstone at least [`MAX_TOMBSTONE_AGE`] old at `now`,
    /// and every one no longer watched: neither tagged nor accepted by
    /// `watched`.
    pub(crate) fn expire(&mut self, now: Duration, watched: impl Fn(&ProcessKey) -> bool) {
        let mut kept = VecDeque::with_capacity(self.entries.len());
        for tombstone in std::mem::take(&mut self.entries) {
            if now.saturating_sub(tombstone.left_clock) >= MAX_TOMBSTONE_AGE {
                self.bytes -= tombstone.bytes;
                self.dropped += 1;
                if tombstone.tagged {
                    self.forgotten_tags += 1;
                }
            } else if !tombstone.tagged && !watched(tombstone.key()) {
                self.bytes -= tombstone.bytes;
            } else {
                kept.push_back(tombstone);
            }
        }
        self.entries = kept;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue::ColumnLayout;
    use crate::projection::SessionItemIds;
    use crate::source::{
        BootId, CappedRecords, Completeness, CpuUsage, CreationToken, DisplayName,
        FakeProcessSource, HostId, Lineage, PidNamespaceId, ProcessRecord, SnapshotTime, SourceId,
        ThreadScan, KIND_NOT_COLLECTED, OWNER_NOT_COLLECTED,
    };

    fn record(pid: u32, name: &str) -> ProcessRecord {
        ProcessRecord {
            kind: KIND_NOT_COLLECTED,
            lineage: Lineage::NOT_COLLECTED,
            owner: OWNER_NOT_COLLECTED,
            key: ProcessKey {
                source: SourceId("lastseen".into()),
                host: Observed::Known(HostId("h".into())),
                boot: Observed::Known(BootId("b".into())),
                pid_namespace: Observed::Known(PidNamespaceId(1)),
                pid: Observed::Known(pid),
                creation: CreationToken::Opaque(name.into()),
                thread: None,
            },
            display_name: DisplayName::from(name),
            resident: Observed::Known(4096),
            cpu: CpuUsage::WarmingUp,
        }
    }

    fn rows(records: Vec<ProcessRecord>) -> Vec<Row> {
        let snapshot = ProcessSnapshot {
            source: SourceId("lastseen".into()),
            sampled_at: SnapshotTime(SystemTime::UNIX_EPOCH),
            records,
            vanished: 0,
            capped: CappedRecords::none(),
            completeness: Completeness::Complete,
            system: FakeProcessSource::system(),
            threads: ThreadScan::NotProvided,
        };
        SessionItemIds::default()
            .project(&snapshot, &ColumnLayout::default())
            .unwrap()
    }

    fn tombstone(row: Row, left: Duration) -> Tombstone {
        Tombstone::new(
            row,
            None,
            Some(SystemTime::UNIX_EPOCH),
            SystemTime::UNIX_EPOCH,
            left,
            Departure::Exited(Exit::Unlisted),
        )
    }

    #[test]
    fn the_count_bound_keeps_the_newest_and_counts_forgotten_tags() {
        let all = rows(
            (0..=MAX_TOMBSTONES as u32)
                .map(|pid| record(pid, "p"))
                .collect(),
        );
        let mut seen = LastSeen::default();
        for row in all.iter().take(MAX_TOMBSTONES) {
            let mut stone = tombstone(row.clone(), Duration::ZERO);
            stone.tagged = true;
            seen.record(stone);
        }
        assert_eq!(
            seen.len(),
            MAX_TOMBSTONES,
            "at the bound, every one is kept"
        );
        assert_eq!(seen.dropped(), 0);
        seen.record(tombstone(all[MAX_TOMBSTONES].clone(), Duration::ZERO));
        assert_eq!(seen.len(), MAX_TOMBSTONES, "past it, the oldest goes");
        assert_eq!(seen.dropped(), 1);
        assert_eq!(seen.forgotten_tags(), 1);
        assert!(seen.get(&all[0].key).is_none());
        assert!(seen.get(&all[MAX_TOMBSTONES].key).is_some());
        assert_eq!(
            seen.bytes(),
            seen.iter().map(Tombstone::bytes).sum::<usize>()
        );
    }

    #[test]
    fn the_byte_bound_drops_the_oldest_exactly_past_it() {
        // Names long enough that the byte bound binds before the count bound.
        let long = "n".repeat(crate::source::MAX_DISPLAY_NAME_CHARS);
        let all = rows(
            (0..MAX_TOMBSTONES as u32)
                .map(|pid| record(pid, &format!("{long}{pid:04}")))
                .collect(),
        );
        let each = tombstone(all[0].clone(), Duration::ZERO).bytes();
        assert!(all
            .iter()
            .all(|row| tombstone(row.clone(), Duration::ZERO).bytes() == each));
        let fit = MAX_TOMBSTONE_BYTES / each;
        assert!(fit < MAX_TOMBSTONES, "{each} bytes each: {fit} fit");
        let mut seen = LastSeen::default();
        for row in all.iter().take(fit) {
            seen.record(tombstone(row.clone(), Duration::ZERO));
        }
        assert_eq!(seen.len(), fit, "at the bound, every one is kept");
        assert!(seen.bytes() <= MAX_TOMBSTONE_BYTES);
        assert!(seen.bytes() + each > MAX_TOMBSTONE_BYTES);
        seen.record(tombstone(all[fit].clone(), Duration::ZERO));
        assert_eq!(seen.len(), fit, "past it, the oldest goes");
        assert_eq!(seen.dropped(), 1);
        assert!(seen.get(&all[0].key).is_none());
        assert!(seen.bytes() <= MAX_TOMBSTONE_BYTES);
    }

    #[test]
    fn age_expires_exactly_at_the_bound_and_unwatched_records_go() {
        let all = rows(vec![record(1, "a"), record(2, "b"), record(3, "c")]);
        let mut seen = LastSeen::default();
        let mut tagged = tombstone(all[0].clone(), Duration::from_secs(5));
        tagged.tagged = true;
        seen.record(tagged);
        seen.record(tombstone(all[1].clone(), Duration::from_secs(5)));
        seen.record(tombstone(all[2].clone(), Duration::from_secs(5)));
        let watched = |key: &ProcessKey| key == &all[1].key;
        // Just under the bound: only the unwatched record goes.
        seen.expire(
            Duration::from_secs(5) + MAX_TOMBSTONE_AGE - Duration::from_nanos(1),
            watched,
        );
        assert_eq!(seen.len(), 2);
        assert!(seen.get(&all[2].key).is_none());
        assert_eq!(seen.dropped(), 0);
        // At the bound: both go, and the tag is counted forgotten.
        seen.expire(Duration::from_secs(5) + MAX_TOMBSTONE_AGE, watched);
        assert!(seen.is_empty());
        assert_eq!(seen.bytes(), 0);
        assert_eq!(seen.dropped(), 2);
        assert_eq!(seen.forgotten_tags(), 1);
        assert_eq!(seen.untag_all(), 1, "a clear counts the forgotten tag");
        assert_eq!(seen.forgotten_tags(), 0);
    }

    #[test]
    fn a_tombstone_keeps_the_read_state_but_never_the_command_line() {
        use crate::command::{CommandContext, PathRead};
        use crate::inspector::{ProcessDetails, ProcessState};
        let all = rows(vec![record(1, "a")]);
        let secret = CommandContext::fixture(
            &[b"tool", b"--password=hunter2-SECRET"],
            PathRead::server(b"/usr/bin/tool"),
            PathRead::server(b"/srv"),
        );
        assert!(secret.text().contains("hunter2-SECRET"));
        let details = ProcessDetails {
            state: Observed::Known(ProcessState::from_code(b'S').unwrap()),
            parent: Observed::Known(1),
            started: Observed::Missing(MissingReason::Unavailable),
            command: secret,
        };
        let stone = Tombstone::new(
            all[0].clone(),
            Some(details.clone()),
            None,
            SystemTime::UNIX_EPOCH,
            Duration::ZERO,
            Departure::Exited(Exit::DetailRead),
        );
        let kept = stone.details.as_ref().unwrap();
        assert_eq!(kept.state, details.state);
        assert_eq!(kept.parent, details.parent);
        assert!(!kept.command.text().contains("hunter2-SECRET"));
        assert_eq!(stone.observed_text(), "last observed: time unknown");
    }

    /// F1 (correction round 1): the tags status counts every historical tag
    /// once, whether none, one, three or more are named, and after the age,
    /// count and byte bounds forgot some or all of them.
    #[test]
    fn the_tags_status_counts_every_historical_tag_once() {
        use crate::tags::{TagCounts, Tags};
        let status = |seen: &LastSeen| Tags::default().status(TagCounts::default(), seen);
        let named =
            |pid: u32| format!("PID {pid} · p exited, last observed 1970-01-01 00:00:00 UTC");
        let tagged = |row: &Row, left: Duration| {
            let mut stone = tombstone(row.clone(), left);
            stone.tagged = true;
            stone
        };
        let all = rows(
            (0..=MAX_TOMBSTONES as u32)
                .map(|pid| record(pid, "p"))
                .collect(),
        );
        let mut seen = LastSeen::default();
        assert_eq!(
            status(&seen),
            "Tags: none · a tag marks a process instance on this server and changes no process"
        );
        seen.record(tagged(&all[0], Duration::ZERO));
        assert_eq!(
            status(&seen),
            format!("Tags: 0 tagged · 1 historical: {}", named(0))
        );
        seen.record(tagged(&all[1], Duration::ZERO));
        seen.record(tagged(&all[2], Duration::ZERO));
        assert_eq!(
            status(&seen),
            format!(
                "Tags: 0 tagged · 3 historical: {}; {}; {}",
                named(2),
                named(1),
                named(0)
            )
        );
        seen.record(tagged(&all[3], Duration::from_secs(1)));
        seen.record(tagged(&all[4], Duration::from_secs(1)));
        assert_eq!(
            status(&seen),
            format!(
                "Tags: 0 tagged · 5 historical: {}; {}; {} and 2 more",
                named(4),
                named(3),
                named(2)
            )
        );
        // Age: the three that left first go; two are still named.
        seen.expire(MAX_TOMBSTONE_AGE, |_| false);
        assert_eq!(
            status(&seen),
            format!(
                "Tags: 0 tagged · 5 historical: {}; {} and 3 more (3 forgotten)",
                named(4),
                named(3)
            )
        );
        // Age again: none is left to name, and the count is not doubled.
        seen.expire(MAX_TOMBSTONE_AGE + Duration::from_secs(1), |_| false);
        assert_eq!(
            status(&seen),
            "Tags: 0 tagged · 5 historical, all forgotten"
        );
        assert_eq!(seen.untag_all(), 5);
        // One forgotten alone.
        seen.record(tagged(&all[0], Duration::ZERO));
        seen.expire(MAX_TOMBSTONE_AGE, |_| false);
        assert_eq!(status(&seen), "Tags: 0 tagged · 1 historical, forgotten");
        assert_eq!(seen.untag_all(), 1);
        // Count: one past the bound.
        for row in &all {
            seen.record(tagged(row, Duration::ZERO));
        }
        let total = MAX_TOMBSTONES + 1;
        assert_eq!(
            status(&seen),
            format!(
                "Tags: 0 tagged · {total} historical: {}; {}; {} and {} more (1 forgotten)",
                named(MAX_TOMBSTONES as u32),
                named(MAX_TOMBSTONES as u32 - 1),
                named(MAX_TOMBSTONES as u32 - 2),
                total - 3
            )
        );
        assert_eq!(seen.untag_all(), total as u64);
        // Bytes: the widest names, one past what fits.
        let long = "n".repeat(crate::source::MAX_DISPLAY_NAME_CHARS);
        let wide = rows(
            (0..MAX_TOMBSTONES as u32)
                .map(|pid| record(pid, &format!("{long}{pid:04}")))
                .collect(),
        );
        let mut seen = LastSeen::default();
        let fit = MAX_TOMBSTONE_BYTES / tombstone(wide[0].clone(), Duration::ZERO).bytes();
        for row in wide.iter().take(fit + 1) {
            seen.record(tagged(row, Duration::ZERO));
        }
        assert_eq!(seen.forgotten_tags(), 1);
        let text = status(&seen);
        assert!(
            text.starts_with(&format!(
                "Tags: 0 tagged · {} historical: PID {fit} · ",
                fit + 1
            )),
            "{text}"
        );
        // The long names cut the status to its bound, so its tail is cut too.
        assert!(text.len() <= crate::tags::MAX_TAGS_STATUS_BYTES + '…'.len_utf8());
    }

    #[test]
    fn a_second_departure_of_the_same_instance_replaces_the_first_record() {
        let all = rows(vec![record(1, "a"), record(2, "b")]);
        let mut seen = LastSeen::default();
        let mut first = tombstone(all[0].clone(), Duration::ZERO);
        first.departure = Departure::NotObserved(Unobserved::Evicted);
        first.tagged = true;
        seen.record(first);
        seen.record(tombstone(all[1].clone(), Duration::ZERO));
        let mut again = tombstone(all[0].clone(), Duration::from_secs(1));
        again.last_observed = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(9));
        seen.record(again);
        assert_eq!(seen.len(), 2);
        assert_eq!(
            seen.bytes(),
            seen.iter().map(Tombstone::bytes).sum::<usize>()
        );
        let kept = seen.get(&all[0].key).unwrap();
        assert_eq!(kept.departure(), Departure::Exited(Exit::Unlisted));
        assert_eq!(kept.left_clock, Duration::from_secs(1));
        assert_eq!(
            kept.last_observed(),
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(9))
        );
        assert!(!kept.is_tagged(), "the latest departure's own watch");
        assert_eq!(
            seen.iter().last().map(|t| t.key().clone()),
            Some(all[0].key.clone()),
            "the newest record is the newest entry"
        );
    }

    #[test]
    fn a_relisted_instance_drops_only_a_no_longer_observed_record() {
        let all = rows(vec![record(1, "a"), record(2, "b"), record(3, "c")]);
        let mut seen = LastSeen::default();
        let mut unobserved = tombstone(all[0].clone(), Duration::ZERO);
        unobserved.departure = Departure::NotObserved(Unobserved::Evicted);
        unobserved.tagged = true;
        seen.record(unobserved);
        seen.record(tombstone(all[1].clone(), Duration::ZERO));
        seen.record(tombstone(all[2].clone(), Duration::ZERO));
        let listed = |key: &ProcessKey| key == &all[0].key || key == &all[1].key;
        let dropped = seen.drop_relisted(listed);
        assert_eq!(dropped.len(), 1);
        assert!(dropped[0].is_tagged(), "handed back, to be tagged again");
        assert!(seen.get(&all[0].key).is_none(), "observed again");
        assert!(
            seen.get(&all[1].key).is_some(),
            "an established exit is final"
        );
        assert!(seen.get(&all[2].key).is_some(), "not listed");
        assert_eq!(seen.forgotten_tags(), 0, "not forgotten: observed");
        assert_eq!(seen.dropped(), 0);
        assert_eq!(
            seen.bytes(),
            seen.iter().map(Tombstone::bytes).sum::<usize>()
        );
    }
}
