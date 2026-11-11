//! The selected-process inspector (PX-013; design §§6.2–6.3, 7.6, 8, 12,
//! 22.5, 29).
//!
//! A compact region under the table describes one process instance: its name,
//! PID and owner, its state, parent and start time, and its current CPU share
//! and resident memory. Five rules shape it:
//!
//! * **The selection is untrusted input.** A `SELECTION_CHANGED` event names an
//!   `ItemId` and nothing else is read from it. The item is resolved only
//!   against the rows this server currently publishes; an item it does not
//!   publish — unknown, stale, or not an item at all — is refused with a
//!   defined state, and nothing is read for it. An item is never turned into a
//!   PID: the process instance is the published row's own [`ProcessKey`].
//!   A `SELECTION_CHANGED` with no `value` is the user leaving the table with
//!   nothing selected (PX-017-G02): the inspector says the selection was
//!   cleared, and nothing is read.
//! * **One current target, one generation.** Each accepted selection — a
//!   user's click, or a search promotion (PX-011, PX-011-G01) — and each
//!   refusal advances a monotonic [`SelectionGeneration`]. The latest one wins,
//!   in the order the server applies them: a click after a promotion replaces
//!   it, and a later promotion replaces a click, exactly as the native table's
//!   highlight does.
//! * **Details load asynchronously and are bound to their target.** The state,
//!   parent and start time are read by one bounded worker
//!   ([`DetailReads`]), never while the view is locked. Every read is tagged
//!   with the target's `(ProcessKey, SelectionGeneration)` and its result is
//!   dropped unless both still name the current target, so a slow read of an
//!   earlier selection can never overwrite a later one.
//! * **Identity, not position.** The inspector follows its item ID wherever a
//!   sort, filter or refresh moves its row. A process that ended, a row a
//!   filter hides, and a record whose PID now names another instance each
//!   publish their own state; a reused PID's new record is never shown.
//!   "Ended" is said only on evidence of an exit: the detail reader found the
//!   instance gone, or a scan entitled to account for it no longer listed it.
//!   A row that retention dropped unconfirmed under the published-row ceiling
//!   is not shown, and why, never ended. Every field the inspector shows is
//!   the latest scan's (PX-013 review round).
//! * **Unavailable is not zero.** A field that could not be read says why; a
//!   parent PID 0 is the kernel's "no parent", never "root"; a state the
//!   kernel did not document is shown as an unrecognized code.
//!
//! The native highlight stays local presentation state (§22.5): the server
//! never selects a row because a user clicked one.
//!
//! Since PX-014 the same read also returns the target's command line,
//! executable and working directory ([`crate::command`]), shown in one
//! selectable `RichText` under the four lines. They are read only here, for
//! the current target, and are bound to its `(ProcessKey, SelectionGeneration)`
//! exactly like its state.
//!
//! Since PX-015 a paused view reads nothing live. While the view is paused a
//! new selection describes its frozen row and asks for no details, a read
//! already in flight is dropped when it returns, and details read before the
//! pause stay shown: everything the inspector shows is from the frozen view,
//! never a live read beside frozen rows. Resume view asks for the current
//! target's details again.
use crate::catalogue::ProcessField;
use crate::command::{CommandContext, COMMAND_HEAD, NO_SELECTION_COMMAND, UNCOLLECTED_COMMAND};
use crate::metric::format_utc;
use crate::owner::{AccountName, AccountNames};
use crate::projection::Row;
use crate::source::{CreationToken, MissingReason, Observed, ProcessKey, Retention, SourceId};
use srui_sdk::{ItemId, Value};
use std::collections::HashSet;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

/// The most bytes one inspector line holds before it is cut and marked
/// ([`crate::bounded`] adds its ellipsis beyond this).
pub const MAX_INSPECTOR_LINE_BYTES: usize = 1024;
/// The status of an inspector with nothing selected.
pub const NO_SELECTION_STATUS: &str = "Selected process: none · select a row to inspect it";
/// The status after the user left the table with nothing selected
/// (PX-017-G02).
pub const CLEARED_SELECTION_STATUS: &str =
    "Selected process: none · the selection was cleared; select a row to inspect it";
/// The status of the uncollected shell's inspector.
pub const UNCOLLECTED_INSPECTOR_STATUS: &str = "Inspector disabled: process collection not started";

/// The monotonic number of the inspector's current target. Advanced by every
/// accepted selection and every refusal, never reused.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SelectionGeneration(u64);

impl SelectionGeneration {
    pub fn get(self) -> u64 {
        self.0
    }

    fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

/// A process's scheduling state as `/proc/<pid>/stat` field 3 reports it: one
/// printable ASCII character (`proc_pid_stat(5)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProcessState(u8);

impl ProcessState {
    /// The state with this code, if the code is one printable ASCII character.
    pub fn from_code(code: u8) -> Option<Self> {
        code.is_ascii_graphic().then_some(Self(code))
    }

    pub fn code(self) -> char {
        char::from(self.0)
    }

    /// What `proc_pid_stat(5)` says the code means, for the codes it
    /// documents on any kernel version.
    pub fn meaning(self) -> Option<&'static str> {
        Some(match self.0 {
            b'R' => "running",
            b'S' => "sleeping",
            b'D' => "waiting in uninterruptible disk sleep",
            b'Z' => "zombie",
            b'T' => "stopped",
            b't' => "tracing stop",
            b'X' | b'x' => "dead",
            b'K' => "wakekill",
            b'W' => "waking or paging",
            b'P' => "parked",
            b'I' => "idle",
            _ => return None,
        })
    }

    fn text(self) -> String {
        match self.meaning() {
            Some(meaning) => format!("{} ({meaning})", self.code()),
            None => format!("{} (unrecognized state)", self.code()),
        }
    }
}

/// When a process instance started: the time after boot its creation token
/// states, and the wall-clock time that is, where the boot time is known.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StartTime {
    pub after_boot: Duration,
    pub at: Observed<SystemTime>,
}

impl StartTime {
    /// The start of an instance whose creation token is `ticks` clock ticks
    /// after boot, at `ticks_per_second`, on a host that booted `boot_time`
    /// seconds after the epoch. `ticks_per_second` must be positive.
    pub fn from_ticks(ticks: u64, ticks_per_second: u64, boot_time: Observed<u64>) -> Self {
        let rate = ticks_per_second.max(1);
        let nanos = (ticks % rate) * 1_000_000_000 / rate;
        let after_boot = Duration::new(ticks / rate, nanos as u32);
        let at = match boot_time {
            Observed::Known(seconds) => SystemTime::UNIX_EPOCH
                .checked_add(Duration::from_secs(seconds))
                .and_then(|boot| boot.checked_add(after_boot))
                .map_or(
                    Observed::Missing(MissingReason::Unavailable),
                    Observed::Known,
                ),
            Observed::Missing(reason) => Observed::Missing(reason),
        };
        Self { after_boot, at }
    }

    fn text(&self) -> String {
        match &self.at {
            Observed::Known(at) => match format_utc(*at) {
                Some(utc) => format!("{utc} (server clock)"),
                None => "before 1970-01-01 00:00:00 UTC (server clock)".to_string(),
            },
            Observed::Missing(reason) => format!(
                "{}.{:02} s after boot (boot time {})",
                self.after_boot.as_secs(),
                self.after_boot.subsec_millis() / 10,
                reason.describe()
            ),
        }
    }
}

/// The fields the inspector reads on demand for one process instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProcessDetails {
    pub state: Observed<ProcessState>,
    /// The parent PID; 0 is the kernel's "no parent" (PID 1 and kernel
    /// threads' parent), never "root" and never "unavailable".
    pub parent: Observed<u32>,
    pub started: Observed<StartTime>,
    /// The command line, executable and working directory (PX-014), read in
    /// the same visit to the same record.
    pub command: CommandContext,
}

/// What one detail read found.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DetailRead {
    Read(ProcessDetails),
    /// The instance is gone: its record no longer exists, or its PID now names
    /// another instance (a different creation token).
    Ended,
    /// The record could not be read, and why.
    Unreadable(MissingReason),
}

/// Reads the details of one process instance. It may block for as long as the
/// read takes: it is only ever called on [`DetailReads`]' worker, never while
/// the view is locked.
pub trait DetailSource: Send + Sync + 'static {
    fn read_details(&self, target: &ProcessKey) -> DetailRead;
}

/// A fixture's details: constants keyed by the opaque creation token of a
/// key its own source minted. Any other key is unreadable.
#[derive(Debug, Clone)]
pub struct FixtureDetails {
    source: SourceId,
    entries: Vec<(&'static str, DetailRead)>,
}

impl FixtureDetails {
    pub fn new(source: &str, entries: Vec<(&'static str, DetailRead)>) -> Self {
        Self {
            source: SourceId(source.into()),
            entries,
        }
    }
}

impl DetailSource for FixtureDetails {
    fn read_details(&self, target: &ProcessKey) -> DetailRead {
        let CreationToken::Opaque(token) = &target.creation else {
            return DetailRead::Unreadable(MissingReason::Unavailable);
        };
        if target.source != self.source {
            return DetailRead::Unreadable(MissingReason::Unavailable);
        }
        self.entries.iter().find(|(name, _)| name == token).map_or(
            DetailRead::Unreadable(MissingReason::Unavailable),
            |(_, read)| read.clone(),
        )
    }
}

/// One detail read: the instance and the generation it was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailRequest {
    pub key: ProcessKey,
    pub generation: SelectionGeneration,
}

#[derive(Default)]
struct Mailbox {
    /// The one read waiting for the worker; a newer request replaces it.
    pending: Option<DetailRequest>,
    /// Whether the worker is reading now.
    reading: bool,
    closed: bool,
    /// Requests replaced before the worker took them.
    superseded: u64,
}

type Shared = Arc<(Mutex<Mailbox>, Condvar)>;

fn lock(shared: &Mutex<Mailbox>) -> MutexGuard<'_, Mailbox> {
    // Every update is one assignment, so the state is valid after any panic.
    shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The inspector's one detail worker and its one-slot queue (§20.4: every
/// queue is bounded).
///
/// At most one read runs and at most one waits; a newer request replaces the
/// waiting one, so selecting many rows quickly reads only the last of them
/// after the read in progress. Dropping this closes the queue: the worker ends
/// once its current read returns, and that read's result is delivered to
/// nothing. A read that never returns holds this one thread and nothing else.
// lc-debt: one worker and no read deadline, so a wedged read leaves every later
// selection loading; upgrade path: dispatch through `crate::on_demand`'s
// deadline and abandoned-read bounds once a second detail field needs them.
pub struct DetailReads {
    shared: Shared,
}

/// Observes a [`DetailReads`] without keeping its worker alive. For tests and
/// fixtures: publication never waits for the worker.
#[derive(Clone)]
pub struct DetailReadsIdle {
    shared: Shared,
}

impl DetailReadsIdle {
    /// Waits up to `timeout` until no read is waiting or running, and says
    /// whether that happened. Must not be called while holding the view's
    /// lock, which the worker takes to publish.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let (state, changed) = &*self.shared;
        let mut state = lock(state);
        while state.pending.is_some() || state.reading {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            state = changed
                .wait_timeout(state, left)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        true
    }

    /// How many requests a newer one replaced before the worker took them.
    pub fn superseded(&self) -> u64 {
        lock(&self.shared.0).superseded
    }
}

fn work(
    source: Arc<dyn DetailSource>,
    shared: Shared,
    deliver: impl Fn(DetailRequest, DetailRead),
) {
    let (state, changed) = &*shared;
    loop {
        let request = {
            let mut mailbox = lock(state);
            loop {
                if mailbox.closed {
                    return;
                }
                if let Some(request) = mailbox.pending.take() {
                    mailbox.reading = true;
                    break request;
                }
                mailbox = changed
                    .wait(mailbox)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        };
        // A read that panics is an unreadable record, never a dead worker.
        let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            source.read_details(&request.key)
        }))
        .unwrap_or(DetailRead::Unreadable(MissingReason::Unavailable));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| deliver(request, read)));
        lock(state).reading = false;
        changed.notify_all();
    }
}

impl DetailReads {
    /// Starts the worker over `source`. Each result is handed to `deliver` on
    /// the worker. `None` if no thread could be started.
    pub fn start(
        source: Arc<dyn DetailSource>,
        deliver: impl Fn(DetailRequest, DetailRead) + Send + 'static,
    ) -> Option<Self> {
        let shared: Shared = Arc::default();
        let worker = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("srtop-inspector".into())
            .spawn(move || work(source, worker, deliver))
            .ok()?;
        Some(Self { shared })
    }

    /// Asks for `request`, replacing a request still waiting.
    pub fn request(&self, request: DetailRequest) {
        let (state, changed) = &*self.shared;
        let mut mailbox = lock(state);
        if mailbox.pending.replace(request).is_some() {
            mailbox.superseded += 1;
        }
        drop(mailbox);
        changed.notify_all();
    }

    pub fn idle(&self) -> DetailReadsIdle {
        DetailReadsIdle {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl Drop for DetailReads {
    fn drop(&mut self) {
        let (state, changed) = &*self.shared;
        let mut mailbox = lock(state);
        mailbox.closed = true;
        mailbox.pending = None;
        drop(mailbox);
        changed.notify_all();
    }
}

/// The details of the current target as the inspector holds them.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Details {
    /// No detail worker is installed: this source provides no details.
    NotProvided,
    Loading,
    /// Not read, because the view is paused (PX-015).
    Deferred,
    Read(ProcessDetails),
    Ended,
    Unreadable(MissingReason),
}

#[derive(Debug, Clone)]
struct Selected {
    /// The row as the latest scan that listed it projected it: shown once the
    /// row is gone, so it never reverts to an older sample.
    row: Row,
    generation: SelectionGeneration,
    details: Details,
    /// Retention dropped the row unconfirmed under the published-row ceiling
    /// (`evict_unconfirmed_beyond`): no scan has said the process ended, and
    /// the row's item ID is gone, so it is not shown, never called ended.
    evicted: bool,
}

#[derive(Debug, Clone)]
enum Selection {
    None,
    /// A selection that resolved to no published row: the item it named, if
    /// it named one at all.
    Refused(Option<ItemId>),
    /// The user left the table with nothing selected (PX-017-G02).
    Cleared,
    Process(Box<Selected>),
}

/// The inspector's server-side state: the current target, its generation and
/// its details, and the detail worker.
pub struct Inspector {
    generation: SelectionGeneration,
    selection: Selection,
    reads: Option<DetailReads>,
    /// Results dropped because their target was no longer current, or
    /// because they returned while the view was paused (PX-015).
    stale: u64,
    /// Whether the view is paused: nothing is read live (PX-015).
    paused: bool,
}

impl Default for Inspector {
    fn default() -> Self {
        Self {
            generation: SelectionGeneration::default(),
            selection: Selection::None,
            reads: None,
            stale: 0,
            paused: false,
        }
    }
}

/// The inspector's four lines and its command-line view, as published.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InspectorView {
    pub status: String,
    pub process: String,
    pub details: String,
    pub metrics: String,
    /// The command-line view's text (PX-014), at most
    /// [`crate::command::MAX_COMMAND_TEXT_BYTES`].
    pub command: String,
}

impl InspectorView {
    /// What the shell starts with: nothing selected, or a disabled inspector
    /// where nothing is collected.
    pub fn initial(collected: bool) -> Self {
        Self {
            status: if collected {
                NO_SELECTION_STATUS
            } else {
                UNCOLLECTED_INSPECTOR_STATUS
            }
            .to_string(),
            command: if collected {
                NO_SELECTION_COMMAND
            } else {
                UNCOLLECTED_COMMAND
            }
            .to_string(),
            ..Self::default()
        }
    }

    fn bounded(
        status: String,
        process: String,
        details: String,
        metrics: String,
        command: String,
    ) -> Self {
        let cut = |text: String| crate::bounded(&text, MAX_INSPECTOR_LINE_BYTES);
        Self {
            status: cut(status),
            process: cut(process),
            details: cut(details),
            metrics: cut(metrics),
            command: crate::bounded(&command, crate::command::MAX_COMMAND_TEXT_BYTES),
        }
    }
}

fn pid_text(pid: &Observed<u32>) -> String {
    match pid {
        Observed::Known(pid) => format!("PID {pid}"),
        Observed::Missing(reason) => format!("PID {}", reason.describe()),
    }
}

fn owner_text(owner: &Observed<u32>, names: &AccountNames) -> String {
    match owner {
        Observed::Known(uid) => match names.name(*uid) {
            AccountName::Named(name) => format!("UID {uid} ({name})"),
            AccountName::NoEntry => format!("UID {uid} (no account name)"),
            AccountName::Pending => format!("UID {uid} (name pending)"),
            AccountName::TimedOut => format!("UID {uid} (name lookup timed out)"),
            AccountName::Failed(_) => format!("UID {uid} (name lookup failed)"),
            AccountName::NotProvided => format!("UID {uid}"),
        },
        Observed::Missing(reason) => reason.describe().to_string(),
    }
}

fn cell_text(row: &Row, field: ProcessField) -> String {
    match &row.cells[field.index()] {
        Value::String(text) => text.clone(),
        Value::UnsignedInt(value) => value.to_string(),
        other => format!("{other:?}"),
    }
}

fn details_text(details: &ProcessDetails) -> String {
    let state = match &details.state {
        Observed::Known(state) => state.text(),
        Observed::Missing(reason) => reason.describe().to_string(),
    };
    let parent = match &details.parent {
        Observed::Known(0) => "none (PID 0: started by the kernel)".to_string(),
        Observed::Known(pid) => format!("PID {pid}"),
        Observed::Missing(reason) => reason.describe().to_string(),
    };
    let started = match &details.started {
        Observed::Known(started) => started.text(),
        Observed::Missing(reason) => reason.describe().to_string(),
    };
    format!("State: {state} · Parent: {parent} · Started: {started}")
}

const DETAILS_HEAD: &str = "State, parent and start time";
const METRICS_HEAD: &str = "CPU and resident memory";

impl Inspector {
    /// Installs the detail worker. Until one is installed, a selected process
    /// is shown without details, as from a source that provides none.
    pub(crate) fn install(&mut self, reads: DetailReads) {
        self.reads = Some(reads);
    }

    pub fn generation(&self) -> SelectionGeneration {
        self.generation
    }

    /// The item the inspector describes, if it describes one.
    pub fn selected(&self) -> Option<ItemId> {
        match &self.selection {
            Selection::Process(selected) => Some(selected.row.item_id),
            _ => None,
        }
    }

    /// The row the inspector describes, as it was when selected (PX-017).
    pub(crate) fn selected_row(&self) -> Option<&Row> {
        match &self.selection {
            Selection::Process(selected) => Some(&selected.row),
            _ => None,
        }
    }

    /// Detail results dropped because their target was no longer current.
    pub fn stale_results(&self) -> u64 {
        self.stale
    }

    /// Makes the published `row` the target, under a new generation, and asks
    /// for its details.
    pub(crate) fn select(&mut self, row: &Row) {
        self.generation = self.generation.next();
        let details = if self.reads.is_none() {
            Details::NotProvided
        } else if self.paused {
            Details::Deferred
        } else {
            Details::Loading
        };
        self.selection = Selection::Process(Box::new(Selected {
            row: row.clone(),
            generation: self.generation,
            details,
            evicted: false,
        }));
        self.ask();
    }

    /// Records a selection that resolved to no published row, under a new
    /// generation. Nothing is read.
    pub(crate) fn refuse(&mut self, item: Option<ItemId>) {
        self.generation = self.generation.next();
        self.selection = Selection::Refused(item);
    }

    /// Records that the user left the table with nothing selected, under a
    /// new generation, so no earlier read can land (PX-017-G02). Nothing is
    /// read.
    pub(crate) fn clear(&mut self) {
        self.generation = self.generation.next();
        self.selection = Selection::Cleared;
    }

    /// The view is paused (PX-015): a read still loading is deferred, and
    /// the current target moves to a new generation, so the result of any
    /// read asked for before the pause is dropped whenever it returns —
    /// during the pause or after a resume (PR #95 review, F1).
    pub(crate) fn pause(&mut self) {
        self.paused = true;
        self.renew();
        if let Selection::Process(selected) = &mut self.selection {
            if selected.details == Details::Loading {
                selected.details = Details::Deferred;
            }
        }
    }

    /// The view is live again (PX-015): the current target moves to another
    /// new generation, and a deferred read is asked for under it. Only a
    /// read asked for from here on can land.
    pub(crate) fn resume(&mut self) {
        self.paused = false;
        self.renew();
        if let Selection::Process(selected) = &mut self.selection {
            if selected.details == Details::Deferred {
                selected.details = Details::Loading;
                self.ask();
            }
        }
    }

    /// Moves the current target to a new generation, keeping its row and
    /// details: every read asked for under an earlier one is stale.
    fn renew(&mut self) {
        if let Selection::Process(selected) = &mut self.selection {
            self.generation = self.generation.next();
            selected.generation = self.generation;
        }
    }

    fn ask(&self) {
        if self.paused {
            return;
        }
        if let (Some(reads), Selection::Process(selected)) = (&self.reads, &self.selection) {
            reads.request(DetailRequest {
                key: selected.row.key.clone(),
                generation: selected.generation,
            });
        }
    }

    /// Reads the current target's details again, if its row is shown and the
    /// latest scan confirmed it, and its details are worth reading again.
    pub(crate) fn reread(&self, rows: &[Row], confirmed: &HashSet<ItemId>) {
        let Selection::Process(selected) = &self.selection else {
            return;
        };
        let item = selected.row.item_id;
        if matches!(selected.details, Details::Ended | Details::NotProvided)
            || !confirmed.contains(&item)
            || !rows.iter().any(|row| row.item_id == item)
        {
            return;
        }
        self.ask();
    }

    /// Records what one accepted scan established about the target (PX-013
    /// review round), before its publication is planned: `latest` is the
    /// server-side collection the scan leaves, `evicted` the unconfirmed rows
    /// retention dropped under the published-row ceiling, and `retention` the
    /// absences the scan could not account for.
    ///
    /// * Listed: the stored row takes the latest projection, so a row shown
    ///   after it is gone carries the newest owner and name, never an older one.
    /// * Evicted: marked, not ended. Nothing established an exit.
    /// * Previously evicted and now absent from a scan entitled to account for
    ///   its instance: that is a confirmed exit, and the target has ended.
    pub(crate) fn observe_scan(
        &mut self,
        latest: &[Row],
        evicted: &HashSet<ItemId>,
        retention: &Retention,
    ) {
        let Selection::Process(selected) = &mut self.selection else {
            return;
        };
        let item = selected.row.item_id;
        if let Some(row) = latest.iter().find(|row| row.item_id == item) {
            selected.row.adopt_sample(row);
        } else if evicted.contains(&item) {
            selected.evicted = true;
        } else if selected.evicted
            && !retention.keeps(&selected.row.key)
            && !latest.iter().any(|row| row.key == selected.row.key)
        {
            selected.details = Details::Ended;
        }
    }

    /// Accepts `read` if it answers the current target's key and generation,
    /// and says whether it did. Any other result is dropped and counted.
    pub(crate) fn completed(&mut self, request: &DetailRequest, read: DetailRead) -> bool {
        match &mut self.selection {
            Selection::Process(selected)
                if !self.paused
                    && selected.generation == request.generation
                    && selected.row.key == request.key
                    && selected.details != Details::Ended =>
            {
                selected.details = match read {
                    DetailRead::Read(details) => Details::Read(details),
                    DetailRead::Ended => Details::Ended,
                    DetailRead::Unreadable(reason) => Details::Unreadable(reason),
                };
                true
            }
            _ => {
                self.stale += 1;
                false
            }
        }
    }

    /// The four lines for a client that holds `rows`, out of the server-side
    /// collection `unfiltered`.
    pub(crate) fn view(
        &self,
        rows: &[Row],
        unfiltered: &[Row],
        unconfirmed: &HashSet<ItemId>,
        names: &AccountNames,
    ) -> InspectorView {
        let selected = match &self.selection {
            Selection::None => return InspectorView::initial(true),
            Selection::Cleared => {
                return InspectorView::bounded(
                    CLEARED_SELECTION_STATUS.to_string(),
                    String::new(),
                    String::new(),
                    String::new(),
                    NO_SELECTION_COMMAND.to_string(),
                );
            }
            Selection::Refused(item) => {
                let status = match item {
                    Some(item) => format!(
                        "Selected process: none · item {} is not a row this server shows; nothing was read",
                        item.get()
                    ),
                    None => "Selected process: none · the selection named no row; nothing was read"
                        .to_string(),
                };
                return InspectorView::bounded(
                    status,
                    String::new(),
                    String::new(),
                    String::new(),
                    format!("{COMMAND_HEAD}: nothing was read"),
                );
            }
            Selection::Process(selected) => selected,
        };
        let item = selected.row.item_id;
        let shown = rows.iter().find(|row| row.item_id == item);
        let kept = shown.or_else(|| unfiltered.iter().find(|row| row.item_id == item));
        let row = kept.unwrap_or(&selected.row);
        let head = format!(
            "Selected process: {} · {}",
            pid_text(&row.key.pid),
            row.display_name.as_str()
        );
        let process = format!(
            "Name: {} · {} · Owner: {}",
            row.display_name.as_str(),
            pid_text(&row.key.pid),
            owner_text(&row.owner, names)
        );
        if kept.is_none() && selected.evicted && selected.details != Details::Ended {
            // Dropped unconfirmed under the row ceiling: not evidence of an
            // exit. If a later scan lists the same instance again, it does so
            // under a new item ID, which this target does not adopt.
            let listed_again = unfiltered.iter().any(|row| row.key == selected.row.key);
            let why = if listed_again {
                "listed again as a new row after the row limit dropped it unconfirmed; select it again"
            } else {
                "not confirmed by the latest scan; not shown (dropped under the row limit)"
            };
            return InspectorView::bounded(
                format!("{head} · {why}"),
                process,
                format!("{DETAILS_HEAD}: not shown"),
                format!("{METRICS_HEAD}: not shown"),
                format!("{COMMAND_HEAD}: not shown"),
            );
        }
        let ended = kept.is_none() || selected.details == Details::Ended;
        if ended {
            return InspectorView::bounded(
                format!("{head} · this process has ended; select another row"),
                process,
                format!("{DETAILS_HEAD}: none: the process has ended"),
                format!("{METRICS_HEAD}: none: the process has ended"),
                format!("{COMMAND_HEAD}: none: the process has ended"),
            );
        }
        let Some(shown) = shown else {
            return InspectorView::bounded(
                format!("{head} · not shown (filtered out or beyond the row limit)"),
                process,
                format!("{DETAILS_HEAD}: not shown"),
                format!("{METRICS_HEAD}: not shown"),
                format!("{COMMAND_HEAD}: not shown"),
            );
        };
        let (state, details, command) = match &selected.details {
            Details::Read(details) => {
                (String::new(), details_text(details), details.command.text())
            }
            Details::Loading => (
                " · loading details".to_string(),
                format!("{DETAILS_HEAD}: loading"),
                format!("{COMMAND_HEAD}: loading"),
            ),
            Details::Deferred => (
                " · details not read while the view is paused".to_string(),
                format!(
                    "{DETAILS_HEAD}: not read while the view is paused; Resume view reads them"
                ),
                format!(
                    "{COMMAND_HEAD}: not read while the view is paused; Resume view reads them"
                ),
            ),
            Details::NotProvided => (
                " · this source provides no details".to_string(),
                format!("{DETAILS_HEAD}: not provided by this source"),
                format!("{COMMAND_HEAD}: not provided by this source"),
            ),
            Details::Unreadable(MissingReason::Denied) => (
                " · details: permission denied".to_string(),
                format!("{DETAILS_HEAD}: permission denied"),
                format!("{COMMAND_HEAD}: permission denied"),
            ),
            Details::Unreadable(MissingReason::Unavailable) => (
                " · details unavailable".to_string(),
                format!("{DETAILS_HEAD}: unavailable"),
                format!("{COMMAND_HEAD}: unavailable"),
            ),
            Details::Ended => unreachable!("an ended target returned above"),
        };
        let earlier = if unconfirmed.contains(&item) {
            " · row kept from an earlier scan"
        } else {
            ""
        };
        InspectorView::bounded(
            format!("{head}{state}{earlier}"),
            process,
            details,
            format!(
                "CPU: {} · Resident: {}",
                cell_text(shown, ProcessField::Cpu),
                cell_text(shown, ProcessField::Resident)
            ),
            command,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_documented_state_is_named_and_any_other_code_is_unrecognized() {
        for (code, meaning) in [
            (b'R', "running"),
            (b'S', "sleeping"),
            (b'D', "waiting in uninterruptible disk sleep"),
            (b'Z', "zombie"),
            (b'T', "stopped"),
            (b't', "tracing stop"),
            (b'X', "dead"),
            (b'x', "dead"),
            (b'I', "idle"),
            (b'P', "parked"),
        ] {
            let state = ProcessState::from_code(code).unwrap();
            assert_eq!(state.meaning(), Some(meaning));
            assert_eq!(state.text(), format!("{} ({meaning})", char::from(code)));
        }
        assert_eq!(
            ProcessState::from_code(b'Q').unwrap().text(),
            "Q (unrecognized state)"
        );
        for refused in [b' ', b'\n', 0, 0x7f, 0xc3] {
            assert_eq!(ProcessState::from_code(refused), None, "{refused}");
        }
    }

    #[test]
    fn a_start_time_is_the_boot_time_plus_the_creation_ticks() {
        let started = StartTime::from_ticks(12_345, 100, Observed::Known(1_800_000_000));
        assert_eq!(started.after_boot, Duration::from_millis(123_450));
        assert_eq!(
            started.at,
            Observed::Known(SystemTime::UNIX_EPOCH + Duration::from_millis(1_800_000_123_450))
        );
        assert_eq!(started.text(), "2027-01-15 08:02:03 UTC (server clock)");
        let unbooted = StartTime::from_ticks(12_345, 100, Observed::Missing(MissingReason::Denied));
        assert_eq!(
            unbooted.text(),
            "123.45 s after boot (boot time permission denied)"
        );
        // A rate that does not divide a second still converts exactly.
        let odd =
            StartTime::from_ticks(1_025, 1_024, Observed::Missing(MissingReason::Unavailable));
        assert_eq!(odd.after_boot, Duration::new(1, 976_562));
    }

    #[test]
    fn a_parent_pid_zero_is_the_kernel_and_an_unread_field_says_why() {
        let details = ProcessDetails {
            state: Observed::Missing(MissingReason::Denied),
            parent: Observed::Known(0),
            started: Observed::Missing(MissingReason::Unavailable),
            command: CommandContext::missing(MissingReason::Denied),
        };
        assert_eq!(
            details_text(&details),
            "State: permission denied · Parent: none (PID 0: started by the kernel) · Started: unavailable"
        );
    }
}
