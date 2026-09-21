//! Typed, injectable one-shot process snapshots and process-instance identity
//! (design §§6.3, 8, 12, 22; PX-002 records, PX-003 identity and completeness).
//! This module reads no process state: adapters live in their own modules and the
//! fake adapter below uses constants only, with no OS enumeration or clock read.
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
            name.push(if Self::is_unsafe(character) {
                REPLACEMENT
            } else {
                character
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

    /// True for every character that must never reach a rendered row: Unicode
    /// control characters (Cc), format characters (Cf — soft hyphen, the bidi
    /// marks and overrides, the zero-width joiners, the tag characters), the
    /// line and paragraph separators, and the code points outside those
    /// categories that render as nothing at all.
    ///
    /// `char::is_control` covers only Cc, so a `comm` of `a\u{2028}sshd` or
    /// `a\u{3164}sshd` would otherwise reach the table as a line break or an
    /// invisible gap and let one row impersonate another. This predicate is
    /// public so tests assert against the same rule the sanitizer applies.
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
                | '\u{fff9}'..='\u{fffb}'
                | '\u{110bd}' | '\u{110cd}'
                | '\u{13430}'..='\u{1343f}'
                | '\u{1bca0}'..='\u{1bca3}'
                | '\u{1d173}'..='\u{1d17a}'
                | '\u{e0001}'
                | '\u{e0020}'..='\u{e007f}'
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
                | '\u{e0100}'..='\u{e01ef}')
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

/// Bound on retained issue descriptions; `skipped` still counts every record.
pub const MAX_RECORDED_ISSUES: usize = 32;

/// Records an explanation only while the bound has room. The description is built
/// lazily, so a host whose records are all unreadable cannot make a scan allocate
/// one detail string per process before a later truncation throws them away: the
/// bound holds during collection, not just in the result. `skipped` is counted by
/// the caller and is never bounded — a degraded scan still reports how much it
/// could not see.
pub fn record_issue(
    issues: &mut Vec<EnumerationIssue>,
    describe: impl FnOnce() -> EnumerationIssue,
) {
    if issues.len() < MAX_RECORDED_ISSUES {
        issues.push(describe());
    }
}

/// Whether the record list is authoritative. An empty `Complete` snapshot means
/// "no processes are visible"; an `Incomplete` snapshot never means that, however
/// many records it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Completeness {
    Complete,
    Incomplete {
        /// Number of records that existed but could not be read.
        skipped: usize,
        /// Bounded explanations, capped at `MAX_RECORDED_ISSUES`.
        issues: Vec<EnumerationIssue>,
    },
}

impl Completeness {
    /// Complete only when nothing at all was skipped or degraded.
    pub fn from_scan(skipped: usize, mut issues: Vec<EnumerationIssue>) -> Self {
        if skipped == 0 && issues.is_empty() {
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
            Self::Incomplete { skipped, .. } => *skipped,
        }
    }

    pub fn issues(&self) -> &[EnumerationIssue] {
        match self {
            Self::Complete => &[],
            Self::Incomplete { issues, .. } => issues,
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

#[cfg(test)]
mod tests {
    use super::*;

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
            '\u{e0001}',
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

    #[test]
    fn completeness_separates_authoritative_empty_from_incomplete() {
        assert!(Completeness::from_scan(0, vec![]).is_complete());
        let degraded = Completeness::from_scan(
            2,
            vec![EnumerationIssue {
                scope: IssueScope::Process(7),
                reason: MissingReason::Denied,
                detail: "denied".into(),
            }],
        );
        assert!(!degraded.is_complete());
        assert_eq!(degraded.skipped(), 2);
        assert_eq!(degraded.issues().len(), 1);
        let flood = Completeness::from_scan(
            1_000,
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
    }
}
