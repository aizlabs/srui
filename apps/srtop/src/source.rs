//! Typed, injectable one-shot process snapshots and process-instance identity
//! (design §§6.3, 8, 12, 22; PX-002 records, PX-003 identity and completeness,
//! PX-004 retention).
//! This module reads no process state: adapters live in their own modules and the
//! fake adapter below uses constants only, with no OS enumeration or clock read.
//!
//! What a degraded scan carries is bounded twice over, and the two bounds are
//! independent: [`MAX_RECORDED_ISSUES`] bounds the human-readable explanations,
//! and [`SkippedRecords`] bounds the skipped identities by the scan's own record
//! bound. Neither is derived from the other — a scan that dropped explanations
//! still names what it skipped — and only the skipped *count* is unbounded,
//! because it is a number.
use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceId(pub String);

/// Host identity of the machine the source enumerated, as observed by the source.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HostId(pub String);

/// Boot identity: PIDs and creation tokens are only comparable within one boot.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BootId(pub String);

/// PID-namespace identity; the same PID number means different processes in
/// different namespaces (K1, `/proc/<pid>/ns/pid`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PidNamespaceId(pub u64);

/// UTC wall-clock sample time; never a process creation/identity token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotTime(pub SystemTime);

/// The highest-resolution process creation token the source can observe.
/// Rounded start-time seconds are never used as a discriminator (PX-003).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CreationToken {
    /// Linux `/proc/<pid>/stat` field 22: start time in kernel clock ticks since
    /// boot (K1). USER_HZ is normally 100, so this resolves to 10 ms, and it is
    /// not derived from a rounded seconds value.
    LinuxBootTicks(u64),
    /// Opaque token minted by a non-OS source. Never an OS identity claim.
    Opaque(String),
}

/// Full process-instance identity. No single component is an identity: a PID, a
/// row position or a display name is never a process instance (design §4).
/// The creation token is mandatory — a source that cannot observe one must report
/// the record as an enumeration issue instead of emitting an unidentified row.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProcessKey {
    pub source: SourceId,
    pub host: Observed<HostId>,
    pub boot: Observed<BootId>,
    pub pid_namespace: Observed<PidNamespaceId>,
    pub pid: Observed<u32>,
    pub creation: CreationToken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MissingReason {
    Unavailable,
    Denied,
}

impl MissingReason {
    /// Short, publishable wording for a status line. The reason a scan could not
    /// see something is part of the honest answer, not diagnostic-only detail.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Denied => "permission denied",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Observed<T> {
    Known(T),
    Missing(MissingReason),
}

/// Longest display name published to the UI, in characters.
pub const MAX_DISPLAY_NAME_CHARS: usize = 128;
/// Shown when a record's name sanitizes to nothing.
pub const UNNAMED_PROCESS: &str = "(unnamed)";
/// Substituted for every control, bidi-override and zero-width character.
pub const REPLACEMENT: char = '\u{fffd}';

/// A process name that is safe to publish as a plain semantic string value.
/// Names are data: they are sanitized once here and never interpolated into a
/// shell command, a format string or any evaluated content.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DisplayName(String);

impl DisplayName {
    /// Accepts arbitrary OS bytes: invalid UTF-8 becomes U+FFFD, control, bidi
    /// and zero-width characters are replaced, surrounding whitespace is trimmed
    /// and the result is bounded. Interior spaces and parentheses are preserved
    /// literally, because they are ordinary text, not syntax.
    pub fn sanitize(raw: &[u8]) -> Self {
        let decoded = String::from_utf8_lossy(raw);
        let mut name = String::with_capacity(decoded.len());
        let mut truncated = false;
        for (index, character) in decoded.chars().enumerate() {
            if index == MAX_DISPLAY_NAME_CHARS {
                truncated = true;
                break;
            }
            name.push(match character {
                // A space separator is legible and cannot fake a glyph, but a
                // non-ASCII one can fake alignment and width, so it is narrowed
                // to a plain space instead of being destroyed.
                _ if Self::is_space_separator(character) => ' ',
                _ if Self::is_unsafe(character) => REPLACEMENT,
                _ => character,
            });
        }
        if truncated {
            name.push('…');
        }
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Self(UNNAMED_PROCESS.to_string());
        }
        if trimmed.len() == name.len() {
            Self(name)
        } else {
            Self(trimmed.to_string())
        }
    }

    /// True for every character that must never reach a rendered row:
    ///
    /// * Unicode control characters (Cc), through [`char::is_control`].
    /// * The complete `Default_Ignorable_Code_Point` set, transcribed below as
    ///   ranges, including its *reserved* members (U+2065, U+FFF0–U+FFF8 and the
    ///   unassigned parts of the tag plane): a code point the standard says to
    ///   ignore renders as nothing today and must not survive as a name.
    /// * The format characters (Cf) outside that set — the Arabic and Kaithi
    ///   number signs, the interlinear annotation marks, the Egyptian format
    ///   controls — which are not ignorable but still invisible.
    /// * The line and paragraph separators (Zl, Zp).
    /// * Blank glyphs in ordinary categories, which no property describes.
    ///
    /// `char::is_control` covers only Cc, so a `comm` of `a\u{2028}sshd` or
    /// `a\u{3164}sshd` would otherwise reach the table as a line break or an
    /// invisible gap and let one row impersonate another. This predicate is
    /// public so tests assert against the same rule the sanitizer applies.
    ///
    /// Only the last group is open-ended: U+3164 is Lo, U+2800 and U+1D159 are
    /// So, U+13441 is Lo and U+13440 is Mn — ordinary categories whose glyph is
    /// blank, or which silently re-render the glyph beside them. "Renders as
    /// nothing" is a property of the glyph, not of the character class, and
    /// `Default_Ignorable_Code_Point` does not include any of them, so they are
    /// enumerated deliberately and the list grows when a new blank glyph is
    /// assigned. The ignorable set above, by contrast, is complete, and
    /// `every_default_ignorable_code_point_is_unsafe` walks all of it.
    pub fn is_unsafe(character: char) -> bool {
        character.is_control()
            || matches!(character,
                // Cf — format characters.
                '\u{00ad}'
                | '\u{0600}'..='\u{0605}'
                | '\u{061c}'
                | '\u{06dd}'
                | '\u{070f}'
                | '\u{0890}' | '\u{0891}'
                | '\u{08e2}'
                | '\u{200b}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{206f}'
                | '\u{feff}'
                | '\u{fff0}'..='\u{fffb}'
                | '\u{110bd}' | '\u{110cd}'
                // The Egyptian Hieroglyph Format Controls block entire —
                // U+13430–U+1345F — rather than its assigned prefix. Every code
                // point in it is a control, a blank, a lost sign or a damage
                // modifier: invisible on its own and defined to alter the glyph
                // beside it. Unicode 16 assigned U+13447–U+13455 inside what was
                // reserved, so closing the block also closes the next such
                // assignment before it ships.
                | '\u{13430}'..='\u{1345f}'
                | '\u{1bca0}'..='\u{1bca3}'
                | '\u{1d173}'..='\u{1d17a}'
                | '\u{1d159}'
                // The whole tag plane, assigned or not: the language tag, the
                // tag characters and the variation selectors supplement all
                // render as nothing, and no code point here belongs in a name.
                | '\u{e0000}'..='\u{e0fff}'
                // Zl and Zp — line and paragraph separators.
                | '\u{2028}' | '\u{2029}'
                // Invisible or blank code points outside Cc and Cf.
                | '\u{034f}'
                | '\u{115f}' | '\u{1160}'
                | '\u{17b4}' | '\u{17b5}'
                | '\u{180b}'..='\u{180f}'
                | '\u{2800}'
                | '\u{3164}'
                | '\u{fe00}'..='\u{fe0f}'
                | '\u{ffa0}'
                | '\u{fffc}'
                | '\u{1107f}'
                | '\u{16fe4}')
    }

    /// Unicode space separators other than U+0020. They are visible as blank
    /// width, so they are not destroyed, but a non-breaking or ideographic space
    /// can fake the width and wrapping of a name, so the sanitizer narrows every
    /// one of them to a plain space.
    pub fn is_space_separator(character: char) -> bool {
        matches!(
            character,
            '\u{00a0}' | '\u{1680}' | '\u{2000}'
                ..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}'
        )
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for DisplayName {
    fn from(value: &str) -> Self {
        Self::sanitize(value.as_bytes())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessRecord {
    pub key: ProcessKey,
    pub display_name: DisplayName,
}

/// What part of the scan could not be observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueScope {
    /// The enumeration root itself could not be listed, so the record list is
    /// not a partial answer: there is no answer at all.
    Root,
    /// One entry of an otherwise readable root could not be examined.
    Entry,
    /// The scan reached its own record bound. Nothing was inaccessible: the
    /// remaining entries were never read, so they are never reported as denied
    /// or unreadable.
    Limit,
    HostIdentity,
    BootIdentity,
    PidNamespace,
    /// One individual record, which was skipped without failing the snapshot.
    Process(u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumerationIssue {
    pub scope: IssueScope,
    pub reason: MissingReason,
    pub detail: String,
}

/// Bound on retained issue *descriptions*. It bounds the human-readable
/// explanations and nothing else: the skipped-record count and the
/// [`SkippedRecords`] PID set are kept apart from it, so a scan still knows what
/// it skipped after this many explanations have been dropped.
pub const MAX_RECORDED_ISSUES: usize = 32;

/// Records an explanation only while the bound has room. The description is built
/// lazily, so a host whose records are all unreadable cannot make a scan allocate
/// one detail string per process before a later truncation throws them away: the
/// bound holds during collection, not just in the result. What was skipped is
/// counted and named separately in [`SkippedRecords`] and is never bounded by
/// this — a degraded scan still reports how much it could not see, and which
/// identities those were.
pub fn record_issue(
    issues: &mut Vec<EnumerationIssue>,
    describe: impl FnOnce() -> EnumerationIssue,
) {
    if issues.len() < MAX_RECORDED_ISSUES {
        issues.push(describe());
    }
}

/// The records one scan could not read: how many there were, and — for each one
/// whose PID the scan observed — which.
///
/// This set is what scopes retention (PX-004), and it is deliberately *not*
/// derived from the bounded [`EnumerationIssue`] list. A skipped record's PID is
/// known even when [`MAX_RECORDED_ISSUES`] threw its explanation away, so
/// reading the uncertain identities out of the capped explanations would make
/// every scan of a host with more than 32 unreadable records — routine on a
/// shared machine — keep every absent row again, which is exactly the unbounded
/// growth scoped retention exists to end.
///
/// Memory stays bounded by the scan's own record bound rather than by the host:
/// the set holds at most `limit` PIDs — the same bound that limits how many
/// records the scan reads — and a scan that skipped more records than it can
/// name reports itself unenumerable instead of growing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedRecords {
    limit: usize,
    named: BTreeSet<u32>,
    count: usize,
    enumerable: bool,
}

impl SkippedRecords {
    /// A scan that has skipped nothing yet and may name up to `limit` PIDs,
    /// which is the collector's own record bound.
    pub fn with_limit(limit: usize) -> Self {
        Self {
            limit,
            named: BTreeSet::new(),
            count: 0,
            enumerable: true,
        }
    }

    /// A scan that skipped no record at all and accounted for everything it saw.
    pub fn none() -> Self {
        Self::with_limit(0)
    }

    /// A scan whose record list itself is unknown — the root could not be listed
    /// or anchored — so the records it hides have no observable identity.
    pub fn unenumerable() -> Self {
        let mut skipped = Self::with_limit(0);
        skipped.enumerable = false;
        skipped
    }

    /// One record that exists, could not be read, and whose PID this scan knows.
    pub fn record(&mut self, pid: u32) {
        self.count += 1;
        if self.named.len() < self.limit || self.named.contains(&pid) {
            self.named.insert(pid);
        } else {
            // Naming this one would grow the set past the scan's record bound,
            // so the scan reports that it cannot name everything it skipped.
            self.enumerable = false;
        }
    }

    /// One record that exists and could not be read, whose PID this scan never
    /// learned: a directory entry it could not even examine.
    pub fn unnamed(&mut self) {
        self.count += 1;
        self.enumerable = false;
    }

    /// Marks the record list itself unknown, whatever has been counted so far.
    pub fn mark_unenumerable(&mut self) {
        self.enumerable = false;
    }

    /// How many records existed but could not be read. Never bounded: a degraded
    /// scan always reports how much it could not see.
    pub fn count(&self) -> usize {
        self.count
    }

    /// The PIDs of the skipped records, at most the scan's record bound of them.
    pub fn pids(&self) -> &BTreeSet<u32> {
        &self.named
    }

    /// Whether every absence this scan leaves is attributable: no record was
    /// skipped without a PID, nothing overflowed the bound, and the record list
    /// itself was read.
    pub fn is_enumerable(&self) -> bool {
        self.enumerable
    }
}

/// Whether the record list is authoritative. An empty `Complete` snapshot means
/// "no processes are visible"; an `Incomplete` snapshot never means that, however
/// many records it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Completeness {
    Complete,
    Incomplete {
        /// The records that existed but could not be read: counted, and named
        /// wherever the scan observed their PIDs.
        skipped: SkippedRecords,
        /// Bounded explanations, capped at `MAX_RECORDED_ISSUES`. They explain a
        /// degraded scan to a person; they never decide which absences are
        /// uncertain.
        issues: Vec<EnumerationIssue>,
    },
}

impl Completeness {
    /// Complete only when nothing at all was skipped or degraded.
    pub fn from_scan(skipped: SkippedRecords, mut issues: Vec<EnumerationIssue>) -> Self {
        if skipped.count() == 0 && skipped.is_enumerable() && issues.is_empty() {
            return Self::Complete;
        }
        issues.truncate(MAX_RECORDED_ISSUES);
        Self::Incomplete { skipped, issues }
    }

    pub fn is_complete(&self) -> bool {
        matches!(self, Self::Complete)
    }

    pub fn skipped(&self) -> usize {
        match self {
            Self::Complete => 0,
            Self::Incomplete { skipped, .. } => skipped.count(),
        }
    }

    /// The records this scan could not read, or `None` when it read them all.
    pub fn skipped_records(&self) -> Option<&SkippedRecords> {
        match self {
            Self::Complete => None,
            Self::Incomplete { skipped, .. } => Some(skipped),
        }
    }

    pub fn issues(&self) -> &[EnumerationIssue] {
        match self {
            Self::Complete => &[],
            Self::Incomplete { issues, .. } => issues,
        }
    }
}

/// Which absent rows a scan is entitled to delete (PX-004).
///
/// A row the current scan did not confirm is deleted unless *that identity's*
/// absence is genuinely uncertain. A single persistently denied record must not
/// make every unrelated exit unobservable: a scan that named exactly which
/// records it could not read has accounted for every other absence, and a row it
/// did not account for really ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Retention {
    /// This scan enumerated the records it skipped, so only rows whose PID it
    /// named are kept; every other absent row is deleted. An empty set is a scan
    /// that accounted for everything, including a complete one.
    Skipped(BTreeSet<u32>),
    /// The uncertain identities cannot be enumerated — the root could not be
    /// listed or entered, or the scan stopped at its own record bound without
    /// reading the remaining PIDs — so every absent row is kept. This is about
    /// records that were never read, never about explanations that were dropped:
    /// a skipped record's PID is known whether or not its [`EnumerationIssue`]
    /// survived [`MAX_RECORDED_ISSUES`].
    Unenumerable,
}

impl Retention {
    /// Whether a published row this scan did not confirm must be kept rather than
    /// deleted.
    ///
    /// A skipped record is matched by the PID its issue names, which is the only
    /// identity component an unread record has. A row whose PID was never
    /// observed can therefore never match, and a row carrying a PID from an
    /// earlier boot or another namespace may match one that names the same
    /// number: matching errs towards keeping a row, never towards deleting one
    /// whose fate is unknown.
    pub fn keeps(&self, key: &ProcessKey) -> bool {
        match self {
            Self::Unenumerable => true,
            Self::Skipped(pids) => match key.pid {
                Observed::Known(pid) => pids.contains(&pid),
                Observed::Missing(_) => false,
            },
        }
    }

    /// True when this scan cannot say which identities are uncertain, so every
    /// absent row is retained.
    pub fn is_global(&self) -> bool {
        matches!(self, Self::Unenumerable)
    }

    /// The PIDs whose absence this scan left uncertain, or `None` when it could
    /// not enumerate them. Bounded by the scan's own record bound when it is
    /// `Some` (see [`SkippedRecords`]), never by the host.
    pub fn uncertain_pids(&self) -> Option<&BTreeSet<u32>> {
        match self {
            Self::Skipped(pids) => Some(pids),
            Self::Unenumerable => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSnapshot {
    pub source: SourceId,
    pub sampled_at: SnapshotTime,
    /// Authoritative display order, supplied by the server-side source.
    pub records: Vec<ProcessRecord>,
    /// Records that no longer existed when the scan reached them. A process that
    /// exits between listing and reading was not *unreadable* — it simply does
    /// not exist at sample time — so it is counted here and never degrades
    /// [`Completeness`]. On a busy host this happens on nearly every scan, and
    /// folding it into `skipped` would leave the shell permanently labeled
    /// "incomplete scan" and make a genuinely denied record indistinguishable
    /// from ordinary churn.
    pub vanished: usize,
    /// Records the scan saw but never read, because it had already published as
    /// many as its own bound allows. They exist and may be perfectly readable,
    /// so counting them as `skipped` would publish a read failure or permission
    /// problem that never happened. The list is still not authoritative, which
    /// [`Completeness`] reports through the recorded [`IssueScope::Limit`].
    pub capped: usize,
    pub completeness: Completeness,
}

impl ProcessSnapshot {
    /// Which absences this snapshot leaves uncertain (PX-004).
    ///
    /// A complete scan accounts for every process it can see, so nothing it did
    /// not list is uncertain. An incomplete one is scoped to the records it
    /// actually named, because a scan that could read all but three records has
    /// still proven that the *other* processes are gone: keeping every absent
    /// row while one record stays permanently denied — the steady state on any
    /// shared host — would accumulate a stale row per exit forever, until the
    /// model's own item bound refuses the next refresh.
    ///
    /// Retention stays whole only where the uncertainty really is whole — where
    /// the records were never read, so their PIDs are genuinely unknown:
    ///
    /// * the root could not be listed or anchored, or one of its entries could
    ///   not be examined ([`SkippedRecords::is_enumerable`] is then false);
    /// * the scan stopped at its own record bound ([`Self::capped`], recorded as
    ///   [`IssueScope::Limit`]), so the entries beyond it were never read;
    /// * more records were skipped than that same bound lets the scan name.
    ///
    /// The bounded [`EnumerationIssue`] list decides none of this. It explains a
    /// degraded scan to a person, and a host with more than
    /// [`MAX_RECORDED_ISSUES`] unreadable records — routine on a shared machine
    /// — drops explanations while still knowing every PID it skipped.
    ///
    /// A record that merely [vanished](Self::vanished) is not uncertain: it did
    /// not exist at sample time, and its row is deleted like any other exit. An
    /// identity file the scan could not read degrades every record equally and
    /// hides none, so it skips nothing and scopes nothing.
    pub fn retention(&self) -> Retention {
        let Completeness::Incomplete { skipped, .. } = &self.completeness else {
            return Retention::Skipped(BTreeSet::new());
        };
        if self.capped > 0 || !skipped.is_enumerable() {
            return Retention::Unenumerable;
        }
        Retention::Skipped(skipped.pids().clone())
    }
}

/// Status label of the deterministic fixture source. Real sources state their own.
pub const FAKE_STATUS_TEXT: &str = "Read-only · Fake process snapshot";

pub trait ProcessSource {
    /// Truthful, source-owned description of this source's provenance, shown as the shell
    /// status. There is no default: a source must state what its data is, so injected
    /// non-fixture data can never be mislabeled as synthetic.
    fn status_text(&self) -> &str;

    fn snapshot(&mut self) -> ProcessSnapshot;
}

#[derive(Debug, Default)]
pub struct FakeProcessSource;

impl FakeProcessSource {
    pub const SOURCE: &'static str = "fake-processes-v1";

    fn key(token: &str, pid: Observed<u32>) -> ProcessKey {
        ProcessKey {
            source: SourceId(Self::SOURCE.into()),
            host: Observed::Known(HostId("fake-host".into())),
            boot: Observed::Known(BootId("fake-boot-0001".into())),
            pid_namespace: Observed::Known(PidNamespaceId(4_026_531_836)),
            pid,
            creation: CreationToken::Opaque(token.into()),
        }
    }
}

impl ProcessSource for FakeProcessSource {
    fn status_text(&self) -> &str {
        FAKE_STATUS_TEXT
    }

    fn snapshot(&mut self) -> ProcessSnapshot {
        ProcessSnapshot {
            source: SourceId(Self::SOURCE.into()),
            sampled_at: SnapshotTime(SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
            records: vec![
                ProcessRecord {
                    key: Self::key("worker-a", Observed::Known(4101)),
                    display_name: "worker".into(),
                },
                ProcessRecord {
                    key: Self::key("worker-b", Observed::Known(4102)),
                    display_name: "worker".into(),
                },
                ProcessRecord {
                    key: Self::key("helper", Observed::Missing(MissingReason::Unavailable)),
                    display_name: "helper".into(),
                },
            ],
            vanished: 0,
            capped: 0,
            completeness: Completeness::Complete,
        }
    }
}

/// Status label of the scripted fixture sequence. Like every source, it states
/// what its data is: a script, never a host.
pub const SEQUENCE_STATUS_TEXT: &str = "Read-only · Fake process sequence";

/// A deterministic, cyclic script of fake snapshots covering every refresh case
/// the collection has to survive (PX-004):
///
/// | step | snapshot |
/// | ---- | -------- |
/// | 0 | three processes: 4101 `worker`, 4102 `worker`, 4103 `helper` |
/// | 1 | the same three processes, sampled one second later |
/// | 2 | 4102 ended, 4103 renamed to `helper-tool`, 4104 `builder` appeared |
/// | 3 | a failed scan: the process list could not be read at all |
/// | 4 | the step 2 processes again |
///
/// The script then repeats from step 0, so a client that attaches at any moment
/// observes every case within one cycle. Step 1 differs from step 0 only in its
/// sample time, which is exactly the thing that must never reach the wire; step
/// 3 must retain the step 2 rows rather than empty the table; and step 4 must
/// converge back onto them without moving a row.
///
/// This is a fixture, not an observation: it reads no process, no clock and no
/// file, and it names itself as synthetic in every published status.
#[derive(Debug, Default)]
pub struct ScriptedFakeSource {
    step: usize,
}

impl ScriptedFakeSource {
    pub const SOURCE: &'static str = "fake-process-sequence-v1";
    pub const STATUS_TEXT: &'static str = SEQUENCE_STATUS_TEXT;
    /// Length of one cycle of the script.
    pub const STEPS: usize = 5;
    /// Sample time of step 0 of the first cycle; each step is one second later.
    pub const EPOCH_SECONDS: u64 = 1_800_000_000;

    /// The step this source will publish next.
    pub fn step(&self) -> usize {
        self.step
    }

    fn key(token: &str, pid: u32) -> ProcessKey {
        ProcessKey {
            source: SourceId(Self::SOURCE.into()),
            host: Observed::Known(HostId("fake-host".into())),
            boot: Observed::Known(BootId("fake-boot-0001".into())),
            pid_namespace: Observed::Known(PidNamespaceId(4_026_531_836)),
            pid: Observed::Known(pid),
            creation: CreationToken::Opaque(token.into()),
        }
    }

    fn record(token: &str, pid: u32, name: &str) -> ProcessRecord {
        ProcessRecord {
            key: Self::key(token, pid),
            display_name: name.into(),
        }
    }

    /// The records and completeness of one step, independent of the sample time.
    pub fn script(step: usize) -> (Vec<ProcessRecord>, Completeness) {
        let settled = || {
            vec![
                Self::record("worker-a", 4101, "worker"),
                Self::record("helper", 4103, "helper-tool"),
                Self::record("builder", 4104, "builder"),
            ]
        };
        match step % Self::STEPS {
            0 | 1 => (
                vec![
                    Self::record("worker-a", 4101, "worker"),
                    Self::record("worker-b", 4102, "worker"),
                    Self::record("helper", 4103, "helper"),
                ],
                Completeness::Complete,
            ),
            2 | 4 => (settled(), Completeness::Complete),
            // The process list itself could not be read: this is not a
            // collection that emptied, and nothing here may be deleted.
            _ => (
                Vec::new(),
                Completeness::from_scan(
                    SkippedRecords::unenumerable(),
                    vec![EnumerationIssue {
                        scope: IssueScope::Root,
                        reason: MissingReason::Denied,
                        detail: "scripted failed scan".into(),
                    }],
                ),
            ),
        }
    }
}

impl ProcessSource for ScriptedFakeSource {
    fn status_text(&self) -> &str {
        Self::STATUS_TEXT
    }

    fn snapshot(&mut self) -> ProcessSnapshot {
        let step = self.step;
        self.step += 1;
        let (records, completeness) = Self::script(step);
        ProcessSnapshot {
            source: SourceId(Self::SOURCE.into()),
            sampled_at: SnapshotTime(
                // The sample time advances on every step, including the two
                // steps whose records are identical. A tick must publish
                // nothing at all for those, which it cannot do if the sample
                // time reaches the UI.
                SystemTime::UNIX_EPOCH + Duration::from_secs(Self::EPOCH_SECONDS + step as u64),
            ),
            records,
            vanished: 0,
            capped: 0,
            completeness,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scripted_sequence_is_cyclic_and_only_its_sample_time_always_moves() {
        let mut source = ScriptedFakeSource::default();
        let cycle: Vec<ProcessSnapshot> = (0..ScriptedFakeSource::STEPS * 2)
            .map(|_| source.snapshot())
            .collect();
        for step in 0..ScriptedFakeSource::STEPS {
            let repeated = &cycle[step + ScriptedFakeSource::STEPS];
            assert_eq!(cycle[step].records, repeated.records, "step {step}");
            assert_eq!(
                cycle[step].completeness, repeated.completeness,
                "step {step}"
            );
            assert_ne!(cycle[step].sampled_at, repeated.sampled_at);
        }
        // Steps 0 and 1 are the same observation taken a second apart.
        assert_eq!(cycle[0].records, cycle[1].records);
        assert_ne!(cycle[0].sampled_at, cycle[1].sampled_at);
        // One insertion, one deletion and one rename.
        let before: Vec<_> = cycle[1].records.iter().map(|r| r.key.clone()).collect();
        let after: Vec<_> = cycle[2].records.iter().map(|r| r.key.clone()).collect();
        assert_eq!(after.iter().filter(|key| !before.contains(key)).count(), 1);
        assert_eq!(before.iter().filter(|key| !after.contains(key)).count(), 1);
        let renamed = cycle[2]
            .records
            .iter()
            .find(|record| record.key == cycle[1].records[2].key)
            .expect("the renamed process keeps its identity");
        assert_eq!(cycle[1].records[2].display_name.as_str(), "helper");
        assert_eq!(renamed.display_name.as_str(), "helper-tool");
        // The failed step is never an authoritative empty result.
        assert!(cycle[3].records.is_empty());
        assert!(!cycle[3].completeness.is_complete());
        assert_eq!(cycle[4].records, cycle[2].records);
    }

    #[test]
    fn issue_recording_stops_allocating_once_the_bound_is_reached() {
        let mut issues = Vec::new();
        let mut described = 0usize;
        let flood = MAX_RECORDED_ISSUES + 500;
        for pid in 0..flood as u32 {
            record_issue(&mut issues, || {
                described += 1;
                EnumerationIssue {
                    scope: IssueScope::Process(pid),
                    reason: MissingReason::Denied,
                    detail: format!("{pid}/stat: denied"),
                }
            });
            assert!(
                issues.len() <= MAX_RECORDED_ISSUES,
                "the buffer must stay bounded during collection, not only after truncation"
            );
        }
        assert_eq!(issues.len(), MAX_RECORDED_ISSUES);
        assert_eq!(
            described, MAX_RECORDED_ISSUES,
            "no explanation may be built once the bound is reached"
        );
    }

    #[test]
    fn hostile_names_are_sanitized_without_losing_literal_text() {
        let name = DisplayName::sanitize(b"we\x1b[31mird ) na\xffme (x");
        assert_eq!(name.as_str(), "we\u{fffd}[31mird ) na\u{fffd}me (x");
        assert!(!name.as_str().chars().any(DisplayName::is_unsafe));
        // Shell metacharacters stay literal data; nothing here is ever evaluated.
        assert_eq!(
            DisplayName::sanitize(b"$(reboot); rm -rf /").as_str(),
            "$(reboot); rm -rf /"
        );
    }

    #[test]
    fn blank_bidi_and_oversized_names_are_bounded_and_never_empty() {
        assert_eq!(DisplayName::sanitize(b"   ").as_str(), UNNAMED_PROCESS);
        assert_eq!(DisplayName::sanitize(b"").as_str(), UNNAMED_PROCESS);
        assert_eq!(
            DisplayName::sanitize("\u{202e}gpj.exe".as_bytes()).as_str(),
            "\u{fffd}gpj.exe"
        );
        assert_eq!(DisplayName::sanitize(b"  pad  ").as_str(), "pad");
        let long = DisplayName::sanitize(&b"n".repeat(MAX_DISPLAY_NAME_CHARS + 50));
        assert_eq!(long.as_str().chars().count(), MAX_DISPLAY_NAME_CHARS + 1);
        assert!(long.as_str().ends_with('…'));
    }

    #[test]
    fn invisible_and_format_characters_outside_cc_are_replaced_too() {
        // Every one of these fits in TASK_COMM_LEN and is invisible or breaks the
        // line when rendered, so each would let a row impersonate another.
        // `char::is_control` matches none of them.
        for spoof in [
            '\u{2028}',
            '\u{2029}',
            '\u{00ad}',
            '\u{034f}',
            '\u{061c}',
            '\u{180e}',
            '\u{2060}',
            '\u{2800}',
            '\u{3164}',
            '\u{fe0f}',
            '\u{ffa0}',
            '\u{fffc}',
            '\u{e0001}',
            '\u{e0020}',
            '\u{e0100}',
            '\u{1107f}',
            '\u{16fe4}',
            '\u{1d159}',
            // Blank glyphs and invisible marks in ordinary categories: `Lo`
            // letters whose rendering is empty space, and an `Mn` mark that
            // silently re-renders its neighbour. No Unicode property
            // distinguishes either from a visible letter or an ordinary accent.
            '\u{13440}',
            '\u{13441}',
            '\u{13446}',
            '\u{13447}',
            '\u{13455}',
        ] {
            assert!(
                !spoof.is_control(),
                "{spoof:?} is the Cc case already covered"
            );
            assert!(DisplayName::is_unsafe(spoof), "{spoof:?} must be replaced");
            let name = DisplayName::sanitize(format!("a{spoof}sshd").as_bytes());
            assert_eq!(name.as_str(), format!("a{REPLACEMENT}sshd"));
            assert!(!name.as_str().chars().any(DisplayName::is_unsafe));
        }
        // Ordinary text, including non-ASCII and shell metacharacters, survives.
        for kept in ["sshd", "ЖУК", "my app (2)", "$(reboot)", "日本語"] {
            assert_eq!(DisplayName::sanitize(kept.as_bytes()).as_str(), kept);
        }
    }

    /// `Default_Ignorable_Code_Point`, Unicode 15.1, in full — including the
    /// reserved ranges, which is where every "adjacent gap" in a hand-written
    /// list comes from.
    const DEFAULT_IGNORABLE: &[(char, char)] = &[
        ('\u{00ad}', '\u{00ad}'),
        ('\u{034f}', '\u{034f}'),
        ('\u{061c}', '\u{061c}'),
        ('\u{115f}', '\u{1160}'),
        ('\u{17b4}', '\u{17b5}'),
        ('\u{180b}', '\u{180f}'),
        ('\u{200b}', '\u{200f}'),
        ('\u{202a}', '\u{202e}'),
        ('\u{2060}', '\u{206f}'),
        ('\u{3164}', '\u{3164}'),
        ('\u{fe00}', '\u{fe0f}'),
        ('\u{feff}', '\u{feff}'),
        ('\u{ffa0}', '\u{ffa0}'),
        ('\u{fff0}', '\u{fff8}'),
        ('\u{1bca0}', '\u{1bca3}'),
        ('\u{1d173}', '\u{1d17a}'),
        ('\u{e0000}', '\u{e0fff}'),
    ];

    #[test]
    fn the_egyptian_format_controls_block_is_closed_whole() {
        // U+13430–U+1345F is controls, blanks, lost signs and damage modifiers:
        // nothing in it is visible on its own. Unicode 16 assigned U+13447–
        // U+13455 inside what this block previously left reserved, which is how
        // a prefix-shaped rule falls behind the standard.
        for point in 0x1_3430..=0x1_345f_u32 {
            let character = char::from_u32(point).expect("the block is valid scalar values");
            assert!(
                DisplayName::is_unsafe(character),
                "U+{point:04X} must never reach a row"
            );
        }
        // The hieroglyphs themselves are ordinary letters and stay legible.
        assert_eq!(
            DisplayName::sanitize("\u{13000}\u{1342f}".as_bytes()).as_str(),
            "\u{13000}\u{1342f}"
        );
    }

    #[test]
    fn every_default_ignorable_code_point_is_unsafe() {
        // Walked exhaustively rather than sampled: the findings this closes were
        // all one code point beside a range end.
        let mut checked = 0usize;
        for (first, last) in DEFAULT_IGNORABLE {
            for point in u32::from(*first)..=u32::from(*last) {
                let Some(character) = char::from_u32(point) else {
                    continue;
                };
                assert!(
                    DisplayName::is_unsafe(character),
                    "U+{point:04X} is default-ignorable and must never reach a row"
                );
                checked += 1;
            }
        }
        assert!(checked > 4_000, "the table must cover the tag plane too");
    }

    #[test]
    fn exotic_spaces_are_narrowed_to_a_plain_space_rather_than_destroyed() {
        // These are visible as blank width, so replacing them would mangle a
        // legible name; keeping them would let a row fake width and wrapping.
        for space in [
            '\u{00a0}', '\u{1680}', '\u{2000}', '\u{2009}', '\u{202f}', '\u{205f}', '\u{3000}',
        ] {
            assert!(DisplayName::is_space_separator(space), "{space:?}");
            assert!(!DisplayName::is_unsafe(space), "{space:?}");
            assert_eq!(
                DisplayName::sanitize(format!("a{space}sshd").as_bytes()).as_str(),
                "a sshd"
            );
        }
        // A name made only of exotic spaces is still never empty.
        assert_eq!(
            DisplayName::sanitize("\u{3000}\u{00a0}".as_bytes()).as_str(),
            UNNAMED_PROCESS
        );
    }

    #[test]
    fn completeness_separates_authoritative_empty_from_incomplete() {
        assert!(Completeness::from_scan(SkippedRecords::none(), vec![]).is_complete());
        let mut two = SkippedRecords::with_limit(8);
        two.record(7);
        two.record(8);
        let degraded = Completeness::from_scan(
            two,
            vec![EnumerationIssue {
                scope: IssueScope::Process(7),
                reason: MissingReason::Denied,
                detail: "denied".into(),
            }],
        );
        assert!(!degraded.is_complete());
        assert_eq!(degraded.skipped(), 2);
        assert_eq!(degraded.issues().len(), 1);
        let mut many = SkippedRecords::with_limit(2_000);
        for pid in 0..1_000u32 {
            many.record(pid);
        }
        let flood = Completeness::from_scan(
            many,
            (0..MAX_RECORDED_ISSUES + 10)
                .map(|pid| EnumerationIssue {
                    scope: IssueScope::Process(pid as u32),
                    reason: MissingReason::Unavailable,
                    detail: "gone".into(),
                })
                .collect(),
        );
        assert_eq!(flood.issues().len(), MAX_RECORDED_ISSUES);
        assert_eq!(flood.skipped(), 1_000);
        assert_eq!(
            flood.skipped_records().map(|skipped| skipped.pids().len()),
            Some(1_000),
            "the skipped identities are known however few explanations survived"
        );
        // A scan whose record list itself failed is never mistaken for a clean
        // one, even though it counted no skipped record.
        let lost = Completeness::from_scan(SkippedRecords::unenumerable(), vec![]);
        assert!(!lost.is_complete());
        assert_eq!(lost.skipped(), 0);
    }

    /// The skipped identities are the scan's own knowledge, not a reading of the
    /// explanations it kept: past `MAX_RECORDED_ISSUES` the explanations stop and
    /// the PIDs do not, so a host with many unreadable records still scopes its
    /// retention instead of falling back to keeping every absent row.
    #[test]
    fn skipped_identities_survive_the_explanation_bound() {
        let denied = MAX_RECORDED_ISSUES * 4;
        let mut skipped = SkippedRecords::with_limit(crate::procfs::MAX_RECORDS);
        let mut issues = Vec::new();
        for pid in 0..denied as u32 {
            skipped.record(4000 + pid);
            record_issue(&mut issues, || EnumerationIssue {
                scope: IssueScope::Process(4000 + pid),
                reason: MissingReason::Denied,
                detail: "denied".into(),
            });
        }
        assert!(skipped.is_enumerable());
        assert_eq!(skipped.count(), denied);
        assert_eq!(skipped.pids().len(), denied);
        assert_eq!(
            issues.len(),
            MAX_RECORDED_ISSUES,
            "explanations are bounded"
        );

        let mut snapshot = FakeProcessSource.snapshot();
        snapshot.completeness = Completeness::from_scan(skipped, issues);
        let Retention::Skipped(uncertain) = snapshot.retention() else {
            panic!("a scan that named every record it skipped can enumerate them")
        };
        assert_eq!(uncertain.len(), denied);
        assert!(uncertain.contains(&4000));
        assert!(uncertain.contains(&(4000 + denied as u32 - 1)));
    }

    /// The set is bounded by the scan's own record bound, never by the host: a
    /// scan that skipped more records than it may name says so instead of
    /// growing, and that answer keeps every absent row.
    #[test]
    fn the_skipped_identity_set_never_grows_past_the_scans_record_bound() {
        let bound = 4;
        let mut skipped = SkippedRecords::with_limit(bound);
        for pid in 0..1_000u32 {
            skipped.record(pid);
        }
        assert_eq!(skipped.pids().len(), bound);
        assert_eq!(skipped.count(), 1_000, "the count is still the truth");
        assert!(!skipped.is_enumerable());

        let mut snapshot = FakeProcessSource.snapshot();
        snapshot.completeness = Completeness::from_scan(skipped, vec![]);
        assert_eq!(snapshot.retention(), Retention::Unenumerable);

        // Re-skipping a PID already named neither grows the set nor gives up on
        // naming it.
        let mut repeated = SkippedRecords::with_limit(1);
        repeated.record(11);
        repeated.record(11);
        assert_eq!(repeated.pids().len(), 1);
        assert_eq!(repeated.count(), 2);
        assert!(repeated.is_enumerable());
    }

    /// Only records that were never read make retention global: a whole-scan
    /// failure, an entry that could not be examined, or the collector's own
    /// record bound.
    #[test]
    fn only_unread_records_make_retention_global() {
        let mut snapshot = FakeProcessSource.snapshot();
        snapshot.completeness = Completeness::from_scan(SkippedRecords::unenumerable(), vec![]);
        assert_eq!(snapshot.retention(), Retention::Unenumerable);

        let mut entry = SkippedRecords::with_limit(16);
        entry.record(4101);
        entry.unnamed();
        let mut snapshot = FakeProcessSource.snapshot();
        snapshot.completeness = Completeness::from_scan(entry, vec![]);
        assert_eq!(snapshot.retention(), Retention::Unenumerable);

        let mut named = SkippedRecords::with_limit(16);
        named.record(4101);
        let mut snapshot = FakeProcessSource.snapshot();
        snapshot.completeness = Completeness::from_scan(named, vec![]);
        assert_eq!(
            snapshot.retention(),
            Retention::Skipped(BTreeSet::from([4101]))
        );
        // The same scan that stopped at its own record bound cannot attribute
        // anything, because the entries beyond it were never read.
        snapshot.capped = 1;
        assert_eq!(snapshot.retention(), Retention::Unenumerable);
    }
}
