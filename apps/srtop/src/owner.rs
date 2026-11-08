//! Owner and PID-set filtering, and the bounded account-name cache (PX-012;
//! design §§6–8, 12, 22, 29).
//!
//! Every rule here is server-side: the client sends the text of two standard
//! text fields and the clicks of two standard buttons, and receives rows and
//! one status line. Four rules shape it:
//!
//! * **The UID is the owner.** A record's owner is the UID its source read
//!   ([`crate::source::ProcessRecord::owner`]). An account name is only a
//!   display of a UID, looked up after the fact and never used as identity,
//!   and an owner the scan could not read is never UID 0 or "root": a specific
//!   user filter hides it and says how many it hid.
//! * **Name lookup never blocks sampling.** `getpwuid_r(3)` can wait on a
//!   network directory for as long as it likes, so it runs only on this cache's
//!   own worker thread. Publication asks the cache, which answers at once from
//!   what it holds — a name, "no account", "pending", "timed out" or "failed"
//!   — and queues at most [`MAX_QUEUED_LOOKUPS`] questions for the worker. A
//!   lookup unanswered after the source's timeout ([`LOOKUP_TIMEOUT`] by
//!   default) is published as timed out; a late answer is still recorded. The
//!   cache holds at most [`MAX_CACHED_ACCOUNTS`] UIDs, evicting the least
//!   recently used answered one, and asks again after [`ACCOUNT_NAME_TTL`].
//! * **"All visible" is what the daemon can see.** The unrestricted choice is
//!   every process the daemon's own OS account can enumerate — the collection
//!   itself — and is not a promise to bypass host permissions: a process the
//!   kernel hides from that account is not in the collection to begin with.
//! * **Filters combine by conjunction.** A row is shown when the user filter,
//!   the PID set *and* the text filter (PX-010) all keep it, and then only
//!   within the published-row ceiling. Search (PX-011) still counts matches over
//!   the whole collection and reports the ones these filters hide as not shown.
//!
//! What a user filter means:
//!
//! | Choice | Rows kept |
//! | ------ | --------- |
//! | all visible users (the default; the user field empty) | every row |
//! | my processes | rows owned by the daemon's effective UID, as the source states it |
//! | a number in the user field | rows owned by exactly that UID |
//! | a name in the user field | rows whose owner UID the cache has resolved to exactly that name; several UIDs may share one name, and every one of them is kept |
//!
//! A PID set is the PIDs typed into its field, separated by commas or spaces,
//! at most [`MAX_PID_SET`] of them. It selects by number on every scan, so a
//! reused PID's new process matches it; identity is still the row's item ID,
//! and changing any filter never mints one.

use crate::filter::FilterQuery;
use crate::projection::Row;
use crate::source::{DisplayName, MissingReason, Observed, REPLACEMENT};
use std::collections::{BTreeSet, HashMap};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// The most bytes of the user field's text the server keeps.
pub const MAX_OWNER_INPUT_BYTES: usize = 64;
/// The most bytes of a resolved account name the server keeps and publishes.
pub const MAX_ACCOUNT_NAME_BYTES: usize = 64;
/// The most bytes of the PID-set field's text the server keeps.
pub const MAX_PID_SET_INPUT_BYTES: usize = 512;
/// The most distinct PIDs one PID set holds; further ones are counted and
/// ignored.
pub const MAX_PID_SET: usize = 64;
/// The most UIDs the account-name cache holds.
pub const MAX_CACHED_ACCOUNTS: usize = 1024;
/// The most lookups queued for the worker at once. A UID that finds the queue
/// full is asked again on a later publication.
pub const MAX_QUEUED_LOOKUPS: usize = 64;
/// How long a lookup may stay unanswered before it is published as timed out.
pub const LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);
/// How long an answer is used before the cache asks again, so a renamed
/// account does not keep its old name for the daemon's whole life.
pub const ACCOUNT_NAME_TTL: Duration = Duration::from_secs(600);
/// The most bytes the user-filter status line publishes.
pub const MAX_OWNER_STATUS_BYTES: usize = 1024;
/// The most PIDs or UIDs one status clause lists.
const MAX_LISTED: usize = 8;
/// The most bytes of one malformed PID-set entry the status quotes.
const MAX_ENTRY_ECHO_BYTES: usize = 24;

/// The status of a shell that collects nothing.
pub const UNCOLLECTED_STATUS: &str = "User filter disabled: process collection not started";

/// A blocking account-name lookup (PX-012). Only ever called on the
/// [`AccountNames`] worker thread, never on a sampling or publishing path.
pub trait AccountLookup: Send + Sync + 'static {
    /// The account name the database gives `uid`, as raw bytes, or `None` when
    /// it has no entry for it.
    fn account_name(&self, uid: u32) -> Result<Option<Vec<u8>>, MissingReason>;
}

/// The system user database, through `getpwuid_r(3)`
/// ([`srui_unix_security::account_name`]).
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemAccountLookup;

impl AccountLookup for SystemAccountLookup {
    fn account_name(&self, uid: u32) -> Result<Option<Vec<u8>>, MissingReason> {
        srui_unix_security::account_name(uid).map_err(|error| {
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                MissingReason::Denied
            } else {
                MissingReason::Unavailable
            }
        })
    }
}

/// A fixed table of account names: a fixture's own, deterministic database.
#[derive(Debug, Clone, Default)]
pub struct FixedAccountLookup {
    names: HashMap<u32, String>,
}

impl FixedAccountLookup {
    pub fn new(names: &[(u32, &str)]) -> Self {
        Self {
            names: names
                .iter()
                .map(|(uid, name)| (*uid, (*name).to_string()))
                .collect(),
        }
    }
}

impl AccountLookup for FixedAccountLookup {
    fn account_name(&self, uid: u32) -> Result<Option<Vec<u8>>, MissingReason> {
        Ok(self.names.get(&uid).map(|name| name.as_bytes().to_vec()))
    }
}

/// The monotonic clock the name cache measures timeouts and answer ages on
/// (PX-012). `Instant::now` unless a test injects another.
pub type AccountClock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// Whose processes "my processes" means, and how owner UIDs are named, for one
/// source's records (PX-012).
#[derive(Clone)]
pub struct Accounts {
    /// The daemon's own account, as the source states it.
    pub current: Observed<u32>,
    /// The database that names the source's owner UIDs, if the source has one.
    pub lookup: Option<Arc<dyn AccountLookup>>,
    /// How long one lookup may stay unanswered before it is published as
    /// timed out.
    pub timeout: Duration,
    /// The clock timeouts and answer ages are measured on.
    pub clock: AccountClock,
}

impl std::fmt::Debug for Accounts {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Accounts")
            .field("current", &self.current)
            .field("lookup", &self.lookup.is_some())
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl Accounts {
    /// A source that states neither the daemon's account nor any names.
    pub fn not_provided() -> Self {
        Self {
            current: Observed::Missing(MissingReason::Unavailable),
            lookup: None,
            timeout: LOOKUP_TIMEOUT,
            clock: Arc::new(Instant::now),
        }
    }

    /// This host: the daemon's effective UID and the system user database.
    pub fn system() -> Self {
        Self::with_lookup(
            Observed::Known(srui_unix_security::effective_uid()),
            Arc::new(SystemAccountLookup),
        )
    }

    /// A fixture's own account and fixed names.
    pub fn fixed(current: u32, names: &[(u32, &str)]) -> Self {
        Self::with_lookup(
            Observed::Known(current),
            Arc::new(FixedAccountLookup::new(names)),
        )
    }

    pub fn with_lookup(current: Observed<u32>, lookup: Arc<dyn AccountLookup>) -> Self {
        Self {
            current,
            lookup: Some(lookup),
            timeout: LOOKUP_TIMEOUT,
            clock: Arc::new(Instant::now),
        }
    }

    /// The same accounts with another lookup timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The same accounts with timeouts and answer ages measured on `clock`.
    #[must_use]
    pub fn with_clock(mut self, clock: AccountClock) -> Self {
        self.clock = clock;
        self
    }
}

/// What the cache can say about one UID's account name, at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountName {
    /// The database's name, sanitized and bounded to [`MAX_ACCOUNT_NAME_BYTES`].
    Named(String),
    /// The database has no entry for this UID.
    NoEntry,
    /// Asked, and not yet answered within the timeout.
    Pending,
    /// Asked, and not answered within the timeout.
    TimedOut,
    /// The lookup failed.
    Failed(MissingReason),
    /// The source names no accounts.
    NotProvided,
}

impl AccountName {
    /// Whether this is an answer rather than a wait.
    pub fn is_known(&self) -> bool {
        !matches!(self, Self::Pending | Self::TimedOut)
    }
}

enum Slot {
    /// Asked; `queued` is false while the worker's queue had no room, so the
    /// question is put again on the next lookup.
    Waiting { since: Instant, queued: bool },
    /// Answered at `at`; `refreshing` while the worker is asked again.
    Answered {
        name: AccountName,
        at: Instant,
        refreshing: bool,
    },
}

struct Entry {
    slot: Slot,
    /// Recency stamp, for eviction.
    used: u64,
}

#[derive(Default)]
struct Shared {
    entries: HashMap<u32, Entry>,
    clock: u64,
    /// Questions sent to the worker and not yet answered.
    in_flight: usize,
}

type Cell = Arc<(Mutex<Shared>, Condvar)>;

fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    // The worker never panics while holding the lock, and the state is valid
    // after any panic elsewhere: every update is one assignment.
    shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The bounded, non-blocking account-name cache and its one worker thread
/// (PX-012). Dropping the cache closes the worker's queue, which ends the
/// worker once its current lookup returns; a lookup that never returns keeps
/// only that one thread, never the cache or the stream.
pub struct AccountNames {
    shared: Cell,
    requests: Option<SyncSender<u32>>,
    timeout: Duration,
    now: AccountClock,
}

impl std::fmt::Debug for AccountNames {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = lock(&self.shared.0);
        formatter
            .debug_struct("AccountNames")
            .field("cached", &state.entries.len())
            .field("in_flight", &state.in_flight)
            .finish()
    }
}

/// A database answer as the cache keeps it.
fn answer_of(answer: Result<Option<Vec<u8>>, MissingReason>) -> AccountName {
    match answer {
        Ok(Some(bytes)) if !bytes.is_empty() => AccountName::Named(crate::bounded(
            DisplayName::sanitize(&bytes).as_str(),
            MAX_ACCOUNT_NAME_BYTES,
        )),
        Ok(_) => AccountName::NoEntry,
        Err(reason) => AccountName::Failed(reason),
    }
}

fn work(lookup: Arc<dyn AccountLookup>, requests: Receiver<u32>, shared: Cell, now: AccountClock) {
    while let Ok(uid) = requests.recv() {
        let answer = answer_of(lookup.account_name(uid));
        let (state, changed) = &*shared;
        let mut state = lock(state);
        state.in_flight = state.in_flight.saturating_sub(1);
        // An entry evicted while its question waited is not brought back.
        if let Some(entry) = state.entries.get_mut(&uid) {
            entry.slot = Slot::Answered {
                name: answer,
                at: now(),
                refreshing: false,
            };
        }
        drop(state);
        changed.notify_all();
    }
}

impl AccountNames {
    /// A cache over `accounts`' lookup, with its worker started. A source with
    /// no lookup gets a cache that answers [`AccountName::NotProvided`].
    pub fn new(accounts: &Accounts) -> Self {
        let shared: Cell = Arc::default();
        let Some(lookup) = accounts.lookup.clone() else {
            return Self {
                shared,
                requests: None,
                timeout: accounts.timeout,
                now: Arc::clone(&accounts.clock),
            };
        };
        let (sender, receiver) = mpsc::sync_channel(MAX_QUEUED_LOOKUPS);
        let worker = Arc::clone(&shared);
        let clock = Arc::clone(&accounts.clock);
        let started = std::thread::Builder::new()
            .name("srtop-accounts".into())
            .spawn(move || work(lookup, receiver, worker, clock));
        Self {
            shared,
            // A worker that could not start leaves a closed queue, and every
            // lookup then fails as unavailable instead of waiting.
            requests: started.ok().map(|_| sender),
            timeout: accounts.timeout,
            now: Arc::clone(&accounts.clock),
        }
    }

    /// What the cache says about `uid` now. Never waits for the worker: an
    /// unknown UID is queued (when the queue has room) and answered later.
    pub fn name(&self, uid: u32) -> AccountName {
        let Some(requests) = &self.requests else {
            return AccountName::NotProvided;
        };
        let now = (self.now)();
        let failed = || Slot::Answered {
            name: AccountName::Failed(MissingReason::Unavailable),
            at: now,
            refreshing: false,
        };
        let mut guard = lock(&self.shared.0);
        let Shared {
            entries,
            clock,
            in_flight,
        } = &mut *guard;
        *clock += 1;
        let used = *clock;
        if let Some(entry) = entries.get_mut(&uid) {
            entry.used = used;
            // A question that found the queue full is put again, and so is
            // one whose answer is older than the TTL.
            let resend = match &entry.slot {
                Slot::Waiting { queued, .. } => !queued,
                Slot::Answered { at, refreshing, .. } => {
                    !refreshing && now.saturating_duration_since(*at) >= ACCOUNT_NAME_TTL
                }
            };
            if resend {
                match requests.try_send(uid) {
                    Ok(()) => {
                        *in_flight += 1;
                        match &mut entry.slot {
                            Slot::Waiting { queued, .. } => *queued = true,
                            Slot::Answered { refreshing, .. } => *refreshing = true,
                        }
                    }
                    Err(TrySendError::Full(_)) => {}
                    Err(TrySendError::Disconnected(_)) => entry.slot = failed(),
                }
            }
            return match &entry.slot {
                Slot::Waiting { since, .. }
                    if now.saturating_duration_since(*since) >= self.timeout =>
                {
                    AccountName::TimedOut
                }
                Slot::Waiting { .. } => AccountName::Pending,
                Slot::Answered { name, .. } => name.clone(),
            };
        }
        if entries.len() >= MAX_CACHED_ACCOUNTS {
            evict(entries);
        }
        let slot = match requests.try_send(uid) {
            Ok(()) => {
                *in_flight += 1;
                Slot::Waiting {
                    since: now,
                    queued: true,
                }
            }
            Err(TrySendError::Full(_)) => Slot::Waiting {
                since: now,
                queued: false,
            },
            Err(TrySendError::Disconnected(_)) => failed(),
        };
        let name = match &slot {
            Slot::Waiting { .. } => AccountName::Pending,
            Slot::Answered { name, .. } => name.clone(),
        };
        entries.insert(uid, Entry { slot, used });
        name
    }

    /// How many UIDs the cache holds.
    pub fn cached(&self) -> usize {
        lock(&self.shared.0).entries.len()
    }

    /// Waits up to `timeout` until every question put to the worker has been
    /// answered and none is left waiting for room, and says whether that
    /// happened. For tests and fixtures: publication never calls it.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let (state, changed) = &*self.shared;
        let mut state = lock(state);
        loop {
            let unqueued = state
                .entries
                .values()
                .any(|entry| matches!(entry.slot, Slot::Waiting { queued: false, .. }));
            if state.in_flight == 0 && !unqueued {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            state = changed
                .wait_timeout(state, deadline - now)
                .map(|(guard, _)| guard)
                .unwrap_or_else(|poisoned| poisoned.into_inner().0);
        }
    }
}

/// Drops the least recently used entry whose question is not with the worker.
/// At most [`MAX_QUEUED_LOOKUPS`] plus the one in progress are with it, far
/// fewer than [`MAX_CACHED_ACCOUNTS`], so there is always one.
///
/// lc-debt: one linear scan of at most MAX_CACHED_ACCOUNTS entries per insertion into a full
/// cache; an ordered recency index would make it logarithmic if the bound grows.
fn evict(entries: &mut HashMap<u32, Entry>) {
    let victim = entries
        .iter()
        .filter(|(_, entry)| {
            !matches!(
                entry.slot,
                Slot::Waiting { queued: true, .. }
                    | Slot::Answered {
                        refreshing: true,
                        ..
                    }
            )
        })
        .min_by_key(|(_, entry)| entry.used)
        .map(|(uid, _)| *uid);
    if let Some(uid) = victim {
        entries.remove(&uid);
    }
}

/// Who the user filter keeps (PX-012).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerChoice {
    /// Every process the daemon's account can see: no user restriction.
    AllVisible,
    /// The daemon's own account, as the source states it.
    Current,
    /// Exactly this UID.
    Uid(u32),
    /// Every owner whose resolved account name is exactly this.
    Name(String),
    /// Digits that are no UID (beyond `u32`): keeps nothing, and says so.
    NotAUid(String),
    /// Text longer than [`MAX_OWNER_INPUT_BYTES`]: no UID or name is read from
    /// a cut field. Keeps nothing, and says so.
    TooLong,
}

/// `text` cut to at most `limit` bytes on a character boundary.
fn cut(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        return text;
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The longest prefix of `text` within `limit` bytes that ends on an entry
/// boundary, and whether anything was cut (PX-012 review round, F2).
///
/// Cutting a field's text and then parsing it would turn a cut entry into a
/// different, valid one: 510 spaces and `4102` cut at 512 bytes reads `41`. So
/// text within the bound is kept whole, and text past it is cut on a
/// character boundary and then back to the last `separator`, dropping the
/// entry the bound cut through. Callers treat a cut field as refused or
/// incomplete and say so; they never read the cut text as what was typed.
pub(crate) fn whole_entries(
    text: &str,
    limit: usize,
    separator: impl Fn(char) -> bool,
) -> (&str, bool) {
    if text.len() <= limit {
        return (text, false);
    }
    let kept = cut(text, limit);
    let rest = &text[kept.len()..];
    let through_an_entry =
        !kept.ends_with(&separator) && rest.chars().next().is_some_and(|next| !separator(next));
    if !through_an_entry {
        return (kept, true);
    }
    let end = kept.rfind(&separator).map_or(0, |index| {
        index + kept[index..].chars().next().map_or(0, char::len_utf8)
    });
    (&kept[..end], true)
}

/// `text` with every control or unsafe character made inert, as a display
/// name is ([`DisplayName::is_unsafe`]).
fn inert(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() || DisplayName::is_unsafe(character) {
                REPLACEMENT
            } else if DisplayName::is_space_separator(character) {
                ' '
            } else {
                character
            }
        })
        .collect()
}

impl OwnerChoice {
    /// The choice the user field's `text` makes: empty is every visible user,
    /// digits are a UID, anything else is an account name, and text longer
    /// than [`MAX_OWNER_INPUT_BYTES`] is refused whole rather than cut into
    /// another UID or name.
    pub fn parse(text: &str) -> Self {
        if text.len() > MAX_OWNER_INPUT_BYTES {
            return Self::TooLong;
        }
        let text = inert(text);
        let text = text.trim();
        if text.is_empty() {
            Self::AllVisible
        } else if text.bytes().all(|byte| byte.is_ascii_digit()) {
            match text.parse() {
                Ok(uid) => Self::Uid(uid),
                Err(_) => Self::NotAUid(text.to_string()),
            }
        } else {
            Self::Name(text.to_string())
        }
    }
}

/// The PIDs typed into the PID-set field (PX-012), bounded.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PidSet {
    /// The field's text as the server keeps it.
    text: String,
    pids: BTreeSet<u32>,
    /// Non-empty entries in the field, valid or not.
    entries: usize,
    /// Entries that are not PIDs, and the first of them, bounded.
    invalid: usize,
    first_invalid: Option<String>,
    /// Valid, distinct PIDs beyond [`MAX_PID_SET`].
    omitted: usize,
    /// Whether the field was longer than [`MAX_PID_SET_INPUT_BYTES`]: the
    /// rest, and any entry the bound cut through, were dropped.
    overlong: bool,
}

impl PidSet {
    /// The set `text` names: entries separated by commas or white space, each
    /// a PID as Go to PID reads one ([`crate::search::parse_pid`]). Text past
    /// [`MAX_PID_SET_INPUT_BYTES`] is dropped together with any entry the
    /// bound cuts through, so a cut PID is never read as a shorter one.
    pub fn parse(text: &str) -> Self {
        let (text, overlong) = whole_entries(text, MAX_PID_SET_INPUT_BYTES, |character: char| {
            character == ',' || character.is_whitespace()
        });
        let mut set = Self {
            text: text.to_string(),
            overlong,
            ..Self::default()
        };
        for entry in set
            .text
            .split(|character: char| character == ',' || character.is_whitespace())
            .filter(|entry| !entry.is_empty())
        {
            set.entries += 1;
            match crate::search::parse_pid(entry) {
                Some(pid) if set.pids.contains(&pid) => {}
                Some(pid) if set.pids.len() < MAX_PID_SET => {
                    set.pids.insert(pid);
                }
                Some(_) => set.omitted += 1,
                None => {
                    set.invalid += 1;
                    if set.first_invalid.is_none() {
                        set.first_invalid =
                            Some(crate::bounded(&inert(entry), MAX_ENTRY_ECHO_BYTES));
                    }
                }
            }
        }
        set
    }

    /// The field's text as the server keeps it.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether the field holds any entry, or was cut. A field of entries none
    /// of which is a PID, like one cut down to none, is active and keeps
    /// nothing: a mistyped or overlong set never shows everything.
    pub fn is_active(&self) -> bool {
        self.entries > 0 || self.overlong
    }

    /// Whether the field was longer than [`MAX_PID_SET_INPUT_BYTES`].
    pub fn is_overlong(&self) -> bool {
        self.overlong
    }

    /// The valid PIDs, at most [`MAX_PID_SET`].
    pub fn pids(&self) -> &BTreeSet<u32> {
        &self.pids
    }

    fn keeps(&self, row: &Row) -> bool {
        !self.is_active() || matches!(row.key.pid, Observed::Known(pid) if self.pids.contains(&pid))
    }
}

/// The user filter as it applies to one collection.
#[derive(Debug, Clone, PartialEq, Eq)]
enum OwnerMatch {
    All,
    Uids(BTreeSet<u32>),
}

impl OwnerMatch {
    fn keeps(&self, row: &Row) -> bool {
        match self {
            Self::All => true,
            Self::Uids(uids) => matches!(row.owner, Observed::Known(uid) if uids.contains(&uid)),
        }
    }
}

/// Every filter that decides which rows are shown, resolved for one
/// collection (PX-012): user, PID set and text, combined by conjunction.
pub(crate) struct Scope<'a> {
    owner: OwnerMatch,
    pids: &'a PidSet,
    text: &'a FilterQuery,
    /// How the user filter was resolved, for its status line.
    who: String,
}

impl Scope<'_> {
    /// Whether every active filter keeps `row`.
    pub(crate) fn keeps(&self, row: &Row) -> bool {
        self.owner.keeps(row) && self.pids.keeps(row) && self.text.matches_row(row)
    }

    /// Whether any filter is active, so some row may be hidden.
    pub(crate) fn is_restricted(&self) -> bool {
        self.restricts_beyond_text() || !self.text.is_empty()
    }

    /// Whether the user filter or the PID set is active.
    pub(crate) fn restricts_beyond_text(&self) -> bool {
        self.owner != OwnerMatch::All || self.pids.is_active()
    }
}

/// What the user-filter controls publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OwnerView {
    pub(crate) status: String,
    /// Whether "Only my processes" is enabled: not while it is the choice.
    pub(crate) mine: bool,
    /// Whether "All visible users" is enabled: not while it is the choice.
    pub(crate) all: bool,
}

impl OwnerView {
    /// What [`crate::initialize_rows`] publishes for a shell that collects
    /// nothing.
    pub(crate) fn uncollected() -> Self {
        Self {
            status: UNCOLLECTED_STATUS.to_string(),
            mine: false,
            all: false,
        }
    }
}

/// The server's user filter and PID set (PX-012), and the cache that names
/// owners. Bounded: one choice, at most [`MAX_OWNER_INPUT_BYTES`] and
/// [`MAX_PID_SET_INPUT_BYTES`] of field text, and the cache's own bounds.
#[derive(Debug)]
pub struct OwnerFilter {
    choice: OwnerChoice,
    /// The user field's text as the server keeps it.
    input: String,
    pids: PidSet,
    current: Observed<u32>,
    names: AccountNames,
}

fn plural<'a>(count: usize, one: &'a str, many: &'a str) -> &'a str {
    if count == 1 {
        one
    } else {
        many
    }
}

/// `values`, at most [`MAX_LISTED`] of them, comma-separated, and how many
/// more there were.
fn listed(values: impl ExactSizeIterator<Item = u32>) -> String {
    let total = values.len();
    let mut text = values
        .take(MAX_LISTED)
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    if total > MAX_LISTED {
        text.push_str(&format!(" and {} more", total - MAX_LISTED));
    }
    text
}

/// How the status names `uid`, with what the cache knows of its name.
fn describe_uid(uid: u32, name: &AccountName) -> String {
    match name {
        AccountName::Named(name) => format!("UID {uid} ({name})"),
        AccountName::NoEntry => format!("UID {uid} (no account name)"),
        AccountName::Pending => format!("UID {uid} (name pending)"),
        AccountName::TimedOut => format!("UID {uid} (name lookup timed out)"),
        AccountName::Failed(reason) => format!("UID {uid} (name {})", reason.describe()),
        AccountName::NotProvided => format!("UID {uid}"),
    }
}

impl OwnerFilter {
    /// Every visible user and no PID set, naming owners through `accounts`.
    /// The daemon's own name is asked for at once, so "my processes" usually
    /// has it by the time it is chosen.
    pub fn new(accounts: &Accounts) -> Self {
        let names = AccountNames::new(accounts);
        if let Observed::Known(uid) = accounts.current {
            let _ = names.name(uid);
        }
        Self {
            choice: OwnerChoice::AllVisible,
            input: String::new(),
            pids: PidSet::default(),
            current: accounts.current.clone(),
            names,
        }
    }

    pub fn choice(&self) -> &OwnerChoice {
        &self.choice
    }

    /// The user field's text as the server keeps it.
    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn pid_set(&self) -> &PidSet {
        &self.pids
    }

    /// The daemon's account, as the source stated it.
    pub fn current(&self) -> &Observed<u32> {
        &self.current
    }

    pub fn names(&self) -> &AccountNames {
        &self.names
    }

    /// A button's choice: replaces the user field's, which is emptied.
    pub fn choose(&mut self, choice: OwnerChoice) {
        self.choice = choice;
        self.input.clear();
    }

    /// The user field's new text; returns the text the server keeps. Text
    /// past [`MAX_OWNER_INPUT_BYTES`] is refused ([`OwnerChoice::TooLong`]);
    /// the field keeps it only up to the last whole entry, so a later edit of
    /// what the field shows never reads a cut UID.
    pub fn set_input(&mut self, text: &str) -> &str {
        let (kept, overlong) = whole_entries(text, MAX_OWNER_INPUT_BYTES, char::is_whitespace);
        self.input = kept.to_string();
        self.choice = if overlong {
            OwnerChoice::TooLong
        } else {
            OwnerChoice::parse(&self.input)
        };
        &self.input
    }

    /// The PID-set field's new text; returns the text the server keeps.
    pub fn set_pid_set(&mut self, text: &str) -> &str {
        self.pids = PidSet::parse(text);
        self.pids.text()
    }

    /// The filters resolved against `collection`, with `text` the committed
    /// text filter. Asks the cache only for the names this choice needs, and
    /// never waits for one.
    pub(crate) fn scope<'a>(&'a self, collection: &[Row], text: &'a FilterQuery) -> Scope<'a> {
        let (owner, who) = match &self.choice {
            OwnerChoice::AllVisible => (
                OwnerMatch::All,
                "all users visible to this daemon's account".to_string(),
            ),
            OwnerChoice::Current => match self.current {
                Observed::Known(uid) => (
                    OwnerMatch::Uids(BTreeSet::from([uid])),
                    format!("mine, {}", describe_uid(uid, &self.names.name(uid))),
                ),
                Observed::Missing(_) => (
                    OwnerMatch::Uids(BTreeSet::new()),
                    "mine, but this source does not state the daemon's account".to_string(),
                ),
            },
            OwnerChoice::Uid(uid) => (
                OwnerMatch::Uids(BTreeSet::from([*uid])),
                describe_uid(*uid, &self.names.name(*uid)),
            ),
            OwnerChoice::NotAUid(text) => (
                OwnerMatch::Uids(BTreeSet::new()),
                format!(
                    "\"{}\" is not a UID (at most {})",
                    crate::bounded(text, MAX_ENTRY_ECHO_BYTES),
                    u32::MAX
                ),
            ),
            OwnerChoice::TooLong => (
                OwnerMatch::Uids(BTreeSet::new()),
                format!(
                    "the user field is longer than {MAX_OWNER_INPUT_BYTES} bytes, so it names \
                     no UID or account; shorten it"
                ),
            ),
            OwnerChoice::Name(name) => self.by_name(name, collection),
        };
        Scope {
            owner,
            pids: &self.pids,
            text,
            who,
        }
    }

    /// The owners of `collection` whose resolved name is exactly `name`.
    ///
    /// lc-debt: asks the cache for every distinct owner of the collection, bounded by the
    /// collection; a host with more distinct owners than MAX_CACHED_ACCOUNTS cycles the cache.
    /// A reverse (`getpwnam_r`) lookup on the worker would make it one question.
    fn by_name(&self, name: &str, collection: &[Row]) -> (OwnerMatch, String) {
        let owners: BTreeSet<u32> = collection
            .iter()
            .filter_map(|row| match row.owner {
                Observed::Known(uid) => Some(uid),
                Observed::Missing(_) => None,
            })
            .collect();
        let quoted = format!("\"{}\"", crate::bounded(name, MAX_ACCOUNT_NAME_BYTES));
        if self.names.requests.is_none() {
            return (
                OwnerMatch::Uids(BTreeSet::new()),
                format!("name {quoted}, but this source names no accounts: type a UID"),
            );
        }
        let mut matched = BTreeSet::new();
        let mut unresolved = 0usize;
        for uid in owners {
            match self.names.name(uid) {
                AccountName::Named(named) if named == name => {
                    matched.insert(uid);
                }
                AccountName::Pending | AccountName::TimedOut | AccountName::Failed(_) => {
                    unresolved += 1
                }
                _ => {}
            }
        }
        let mut who = match matched.len() {
            0 if unresolved > 0 => {
                format!("name {quoted}: no resolved process owner has this account name yet")
            }
            0 => format!("name {quoted}: no visible process owner has this account name"),
            1 => format!("name {quoted}, UID {}", listed(matched.iter().copied())),
            count => format!(
                "name {quoted}, UIDs {} ({count} accounts share this name)",
                listed(matched.iter().copied())
            ),
        };
        if unresolved > 0 {
            who.push_str(&format!(
                "; {unresolved} {} not resolved yet, so {} processes stay hidden until resolved",
                plural(unresolved, "owner's name is", "owners' names are"),
                plural(unresolved, "its", "their"),
            ));
        }
        (OwnerMatch::Uids(matched), who)
    }

    /// What the user-filter controls publish for `collection`, when `shown`
    /// rows of the `matched` that every filter keeps are published.
    pub(crate) fn view(
        &self,
        scope: &Scope<'_>,
        collection: &[Row],
        shown: usize,
        matched: usize,
    ) -> OwnerView {
        let total = collection.len();
        let mut status = format!("User: {}", scope.who);
        if self.pids.is_active() {
            let count = self.pids.pids.len();
            status.push_str(&format!(
                " · PID set: {count} {}",
                plural(count, "PID", "PIDs")
            ));
            let present: BTreeSet<u32> = collection
                .iter()
                .filter_map(|row| match row.key.pid {
                    Observed::Known(pid) if self.pids.pids.contains(&pid) => Some(pid),
                    _ => None,
                })
                .collect();
            let absent: Vec<u32> = self.pids.pids.difference(&present).copied().collect();
            if !absent.is_empty() {
                status.push_str(&format!(
                    " · no visible process has {} {}",
                    plural(absent.len(), "PID", "PIDs"),
                    listed(absent.into_iter())
                ));
            }
            if let Some(first) = &self.pids.first_invalid {
                status.push_str(&format!(
                    " · {} {} not {}: \"{first}\"",
                    self.pids.invalid,
                    plural(self.pids.invalid, "entry is", "entries are"),
                    plural(self.pids.invalid, "a PID", "PIDs"),
                ));
            }
            if self.pids.overlong {
                status.push_str(&format!(
                    " · text past {MAX_PID_SET_INPUT_BYTES} bytes ignored, with any entry the \
                     bound cut through"
                ));
            }
            if self.pids.omitted > 0 {
                status.push_str(&format!(
                    " · bounded to {MAX_PID_SET} PIDs, {} more ignored",
                    self.pids.omitted
                ));
            }
        }
        if !scope.is_restricted() {
            status.push_str(&format!(
                " · showing all {total} {}",
                plural(total, "process", "processes")
            ));
        } else if matched > 0 {
            status.push_str(&format!(
                " · {matched} of {total} {} match",
                plural(total, "process", "processes")
            ));
            if shown < matched {
                status.push_str(&format!(" ({shown} shown)"));
            }
        } else if total == 0 {
            status.push_str(" · no process matches: the collection is empty");
        } else {
            // The empty result explains itself: what each active filter
            // would keep on its own.
            let mut alone = Vec::new();
            if scope.owner != OwnerMatch::All {
                let kept = collection
                    .iter()
                    .filter(|row| scope.owner.keeps(row))
                    .count();
                alone.push(format!("the user filter alone keeps {kept}"));
            }
            if self.pids.is_active() {
                let kept = collection.iter().filter(|row| self.pids.keeps(row)).count();
                alone.push(format!("the PID set alone keeps {kept}"));
            }
            if !scope.text.is_empty() {
                let kept = collection
                    .iter()
                    .filter(|row| scope.text.matches_row(row))
                    .count();
                alone.push(format!("the text filter alone keeps {kept}"));
            }
            status.push_str(&format!(
                " · no process of {total} matches every filter: {}",
                alone.join(", ")
            ));
        }
        if scope.owner != OwnerMatch::All {
            let unreadable = collection
                .iter()
                .filter(|row| matches!(row.owner, Observed::Missing(_)))
                .count();
            if unreadable > 0 {
                status.push_str(&format!(
                    " · {unreadable} {} with an unreadable owner hidden",
                    plural(unreadable, "process", "processes")
                ));
            }
        }
        OwnerView {
            status: crate::bounded(&status, MAX_OWNER_STATUS_BYTES),
            mine: self.choice != OwnerChoice::Current,
            all: self.choice != OwnerChoice::AllVisible,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_user_field_reads_digits_as_a_uid_and_anything_else_as_a_name() {
        assert_eq!(OwnerChoice::parse(""), OwnerChoice::AllVisible);
        assert_eq!(OwnerChoice::parse("   "), OwnerChoice::AllVisible);
        assert_eq!(OwnerChoice::parse(" 1000 "), OwnerChoice::Uid(1000));
        assert_eq!(OwnerChoice::parse("0"), OwnerChoice::Uid(0));
        assert_eq!(
            OwnerChoice::parse("4294967296"),
            OwnerChoice::NotAUid("4294967296".into())
        );
        assert_eq!(
            OwnerChoice::parse("alice"),
            OwnerChoice::Name("alice".into())
        );
        assert_eq!(OwnerChoice::parse("-1"), OwnerChoice::Name("-1".into()));
        // Control characters are made inert, never matched or echoed raw.
        assert_eq!(
            OwnerChoice::parse("a\u{1b}[2Jb"),
            OwnerChoice::Name(format!("a{REPLACEMENT}[2Jb"))
        );
        // At the bound a name is read whole; past it nothing is read.
        let at = "x".repeat(MAX_OWNER_INPUT_BYTES);
        assert_eq!(OwnerChoice::parse(&at), OwnerChoice::Name(at.clone()));
        let past = "x".repeat(MAX_OWNER_INPUT_BYTES + 1);
        assert_eq!(OwnerChoice::parse(&past), OwnerChoice::TooLong);
    }

    #[test]
    fn a_pid_set_is_bounded_deduplicated_and_names_what_it_ignored() {
        let set = PidSet::parse("4101, 4102 4102,,9999 abc -3");
        assert!(set.is_active());
        assert_eq!(set.pids(), &BTreeSet::from([4101, 4102, 9999]));
        assert_eq!(set.invalid, 2);
        assert_eq!(set.first_invalid.as_deref(), Some("abc"));
        assert!(!PidSet::parse(" , ").is_active());
        // Only entries that are not PIDs: active, and keeps nothing.
        let wrong = PidSet::parse("abc");
        assert!(wrong.is_active() && wrong.pids().is_empty());
        let many: String = (1..=200).map(|pid| format!("{pid} ")).collect();
        let bounded = PidSet::parse(&many);
        assert_eq!(bounded.pids().len(), MAX_PID_SET);
        assert!(bounded.omitted > 0);
        assert!(bounded.text().len() <= MAX_PID_SET_INPUT_BYTES);
        let wide = "\u{20000}".repeat(MAX_PID_SET_INPUT_BYTES);
        assert!(PidSet::parse(&wide).text().len() <= MAX_PID_SET_INPUT_BYTES);
    }

    /// PX-012 review round, F2: the bound never cuts a PID into a shorter one.
    #[test]
    fn a_pid_set_past_its_bound_drops_the_entry_the_bound_cuts_through() {
        let spaces = |count: usize| " ".repeat(count);
        // At the bound: read whole.
        let at = PidSet::parse(&format!("{}4102", spaces(MAX_PID_SET_INPUT_BYTES - 4)));
        assert_eq!(at.pids(), &BTreeSet::from([4102]));
        assert!(!at.is_overlong());
        // The reported case: 510 spaces and 4102 is not PID 41.
        let past = PidSet::parse(&format!("{}4102", spaces(510)));
        assert!(past.pids().is_empty(), "{:?}", past.pids());
        assert!(
            past.is_overlong() && past.is_active(),
            "keeps nothing, never everything"
        );
        assert_eq!(past.text(), spaces(510));
        // A separator right at the bound: the entries before it are whole.
        let edge = PidSet::parse(&format!("{}4102 7", spaces(MAX_PID_SET_INPUT_BYTES - 4)));
        assert_eq!(edge.pids(), &BTreeSet::from([4102]));
        assert!(edge.is_overlong());
        // Commas separate too, and earlier whole entries survive the cut.
        let commas = PidSet::parse(&format!("4101,{},99999", ",".repeat(503)));
        assert_eq!(commas.pids(), &BTreeSet::from([4101]));
        // A multi-byte character at the cut stays on a character boundary.
        let wide = PidSet::parse(&format!("{}é4102", spaces(MAX_PID_SET_INPUT_BYTES - 1)));
        assert!(wide.pids().is_empty() && wide.text().len() <= MAX_PID_SET_INPUT_BYTES);
    }

    #[test]
    fn the_user_field_past_its_bound_names_no_uid() {
        let mut filter = OwnerFilter::new(&Accounts::not_provided());
        let at = format!("{}1001", " ".repeat(MAX_OWNER_INPUT_BYTES - 4));
        filter.set_input(&at);
        assert_eq!(filter.choice(), &OwnerChoice::Uid(1001));
        let past = format!("{}10001", " ".repeat(MAX_OWNER_INPUT_BYTES - 4));
        assert_eq!(
            filter.set_input(&past),
            " ".repeat(MAX_OWNER_INPUT_BYTES - 4)
        );
        assert_eq!(filter.choice(), &OwnerChoice::TooLong, "not UID 1000");
    }

    #[test]
    fn a_database_answer_is_sanitized_bounded_and_an_empty_name_is_no_entry() {
        assert_eq!(
            answer_of(Ok(Some(b"alice".to_vec()))),
            AccountName::Named("alice".into())
        );
        assert_eq!(answer_of(Ok(Some(Vec::new()))), AccountName::NoEntry);
        assert_eq!(answer_of(Ok(None)), AccountName::NoEntry);
        assert_eq!(
            answer_of(Err(MissingReason::Denied)),
            AccountName::Failed(MissingReason::Denied)
        );
        let AccountName::Named(hostile) = answer_of(Ok(Some(b"a\x1b]0;x\x07\xffb".to_vec())))
        else {
            panic!("a hostile name is still a name")
        };
        assert!(!hostile.chars().any(|c| c.is_control()), "{hostile:?}");
        let AccountName::Named(long) = answer_of(Ok(Some(vec![b'n'; 4096]))) else {
            panic!("a long name is a name")
        };
        assert!(long.len() <= MAX_ACCOUNT_NAME_BYTES + '…'.len_utf8());
    }

    /// A database that records every question and answers `name-<uid>-<n>`,
    /// `n` counting the questions about that UID so far.
    #[derive(Default)]
    struct Counting {
        asked: Mutex<Vec<u32>>,
    }

    impl Counting {
        fn times(&self, uid: u32) -> usize {
            self.asked
                .lock()
                .unwrap()
                .iter()
                .filter(|asked| **asked == uid)
                .count()
        }
    }

    impl AccountLookup for Counting {
        fn account_name(&self, uid: u32) -> Result<Option<Vec<u8>>, MissingReason> {
            let mut asked = self.asked.lock().unwrap();
            asked.push(uid);
            let n = asked.iter().filter(|asked| **asked == uid).count();
            Ok(Some(format!("name-{uid}-{n}").into_bytes()))
        }
    }

    fn holds(names: &AccountNames, uid: u32) -> bool {
        lock(&names.shared.0).entries.contains_key(&uid)
    }

    const SETTLE: Duration = Duration::from_secs(10);

    /// PX-012 review round, W1: a full cache evicts the least recently used
    /// answer, not the most recent one, and an evicted UID is asked again.
    #[test]
    fn a_full_cache_evicts_the_least_recently_used_answer() {
        let lookup = Arc::new(Counting::default());
        let names = AccountNames::new(&Accounts::with_lookup(
            Observed::Known(0),
            Arc::clone(&lookup) as Arc<dyn AccountLookup>,
        ));
        for uid in 0..MAX_CACHED_ACCOUNTS as u32 {
            let _ = names.name(uid);
            if uid % 32 == 31 {
                assert!(names.wait_idle(SETTLE));
            }
        }
        assert!(names.wait_idle(SETTLE));
        assert_eq!(names.cached(), MAX_CACHED_ACCOUNTS);
        // UID 0 is used again, so UID 1 is now the least recently used.
        assert_eq!(names.name(0), AccountName::Named("name-0-1".into()));
        let _ = names.name(5000);
        assert!(names.wait_idle(SETTLE));
        assert_eq!(names.cached(), MAX_CACHED_ACCOUNTS);
        assert!(holds(&names, 0), "the most recently used answer is kept");
        assert!(
            !holds(&names, 1),
            "the least recently used answer is evicted"
        );
        assert!(holds(&names, 2) && holds(&names, 5000));
        // An evicted UID is a new question.
        assert_eq!(names.name(1), AccountName::Pending);
        assert!(names.wait_idle(SETTLE));
        assert_eq!(lookup.times(1), 2);
        assert_eq!(names.name(1), AccountName::Named("name-1-2".into()));
    }

    /// PX-012 review round, W1: an answer is used, without a new question,
    /// until it is [`ACCOUNT_NAME_TTL`] old; then the cache asks again once,
    /// keeps publishing the old answer meanwhile, and uses the new one.
    #[test]
    fn an_answer_is_asked_again_once_it_is_ten_minutes_old() {
        let start = Instant::now();
        let elapsed = Arc::new(Mutex::new(Duration::ZERO));
        let clock: AccountClock = {
            let elapsed = Arc::clone(&elapsed);
            Arc::new(move || start + *elapsed.lock().unwrap())
        };
        let advance = |by: Duration| *elapsed.lock().unwrap() += by;
        let lookup = Arc::new(Counting::default());
        let names = AccountNames::new(
            &Accounts::with_lookup(
                Observed::Known(7),
                Arc::clone(&lookup) as Arc<dyn AccountLookup>,
            )
            .with_clock(clock),
        );
        assert_eq!(names.name(7), AccountName::Pending);
        assert!(names.wait_idle(SETTLE));
        assert_eq!(names.name(7), AccountName::Named("name-7-1".into()));
        advance(ACCOUNT_NAME_TTL - Duration::from_secs(1));
        assert_eq!(names.name(7), AccountName::Named("name-7-1".into()));
        assert!(names.wait_idle(SETTLE));
        assert_eq!(lookup.times(7), 1, "a fresh answer is not asked again");
        advance(Duration::from_secs(1));
        assert_eq!(
            names.name(7),
            AccountName::Named("name-7-1".into()),
            "the old answer is published while it is asked again"
        );
        let _ = names.name(7);
        assert!(names.wait_idle(SETTLE));
        assert_eq!(lookup.times(7), 2, "asked again exactly once");
        assert_eq!(names.name(7), AccountName::Named("name-7-2".into()));
        assert!(names.wait_idle(SETTLE));
        assert_eq!(lookup.times(7), 2, "the new answer is fresh");
    }

    /// The cache never holds more than its bound, and the worker's queue never
    /// more than its own.
    #[test]
    fn the_cache_evicts_the_least_recently_used_answer_beyond_its_bound() {
        let names = AccountNames::new(&Accounts::fixed(1, &[(1, "one")]));
        for uid in 0..(MAX_CACHED_ACCOUNTS as u32 + 200) {
            let _ = names.name(uid);
            if uid % (MAX_QUEUED_LOOKUPS as u32 / 2) == 0 {
                assert!(names.wait_idle(Duration::from_secs(10)));
            }
            assert!(names.cached() <= MAX_CACHED_ACCOUNTS);
        }
        assert!(names.wait_idle(Duration::from_secs(10)));
        assert!(names.cached() <= MAX_CACHED_ACCOUNTS);
    }
}
