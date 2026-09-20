//! One-shot Linux `/proc` enumeration with process-instance identity
//! (K1; design §§6.3, 8, 12, 22; PX-003). Read-only: this module opens no process
//! for control, sends no signal and never passes a process name to a shell.
//!
//! The scan root is injected, so every branch — denied records, vanished records,
//! hostile names, missing identity files — is exercised by deterministic fixtures
//! on any platform. Only [`ProcFsSource::live`] reads the real `/proc`, which
//! exists on Linux; on other systems it honestly reports an incomplete scan
//! instead of an authoritative empty result.
use crate::source::{
    BootId, Completeness, CreationToken, DisplayName, EnumerationIssue, HostId, IssueScope,
    MissingReason, Observed, PidNamespaceId, ProcessKey, ProcessRecord, ProcessSnapshot,
    ProcessSource, SnapshotTime, SourceId,
};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The real Linux process filesystem.
pub const DEFAULT_PROC_ROOT: &str = "/proc";
/// Source-owned status label: this data is a live read of a real process
/// filesystem and is never described as a fixture.
pub const LIVE_STATUS_TEXT: &str = "Read-only · Live process snapshot";
/// Bound on records published from one scan; the remainder is reported skipped.
pub const MAX_RECORDS: usize = 65_536;
/// Bound on every single file this scan reads.
pub const MAX_FILE_BYTES: u64 = 64 * 1024;
/// `/proc/<pid>/stat` field 22 (start time) is the 20th field after `comm`.
const STARTTIME_FIELD_AFTER_COMM: usize = 19;

/// One-shot reader over a `/proc`-shaped directory tree.
#[derive(Debug, Clone)]
pub struct ProcFsSource {
    root: PathBuf,
    source: SourceId,
    status: String,
}

impl ProcFsSource {
    /// Reads the host's real process filesystem.
    pub fn live() -> Self {
        Self::with_root(DEFAULT_PROC_ROOT)
    }

    /// Reads a `/proc`-shaped tree. The root is part of the source identity, so
    /// records from different roots can never alias.
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let source = SourceId(format!("procfs:{}", root.display()));
        // Only the real process filesystem may be described as live data.
        let status = if root == Path::new(DEFAULT_PROC_ROOT) {
            LIVE_STATUS_TEXT.to_string()
        } else {
            format!(
                "Read-only · Process filesystem fixture snapshot: {}",
                root.display()
            )
        };
        Self {
            root,
            source,
            status,
        }
    }

    pub fn source_id(&self) -> &SourceId {
        &self.source
    }

    fn identity<T>(
        &self,
        relative: &str,
        scope: IssueScope,
        issues: &mut Vec<EnumerationIssue>,
        wrap: impl FnOnce(String) -> T,
    ) -> Observed<T> {
        match read_bounded(&self.root.join(relative)) {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes).trim().to_string();
                if text.is_empty() {
                    issues.push(EnumerationIssue {
                        scope,
                        reason: MissingReason::Unavailable,
                        detail: format!("{relative} is empty"),
                    });
                    Observed::Missing(MissingReason::Unavailable)
                } else {
                    Observed::Known(wrap(text))
                }
            }
            Err(error) => {
                let reason = reason_for(&error);
                issues.push(EnumerationIssue {
                    scope,
                    reason,
                    detail: format!("{relative}: {error}"),
                });
                Observed::Missing(reason)
            }
        }
    }

    fn pid_namespace(&self, issues: &mut Vec<EnumerationIssue>) -> Observed<PidNamespaceId> {
        let path = self.root.join("self/ns/pid");
        match std::fs::read_link(&path) {
            Ok(target) => match parse_namespace(&target.to_string_lossy()) {
                Some(inode) => Observed::Known(PidNamespaceId(inode)),
                None => {
                    issues.push(EnumerationIssue {
                        scope: IssueScope::PidNamespace,
                        reason: MissingReason::Unavailable,
                        detail: "self/ns/pid is not a pid:[inode] link".into(),
                    });
                    Observed::Missing(MissingReason::Unavailable)
                }
            },
            Err(error) => {
                let reason = reason_for(&error);
                issues.push(EnumerationIssue {
                    scope: IssueScope::PidNamespace,
                    reason,
                    detail: format!("self/ns/pid: {error}"),
                });
                Observed::Missing(reason)
            }
        }
    }
}

impl ProcessSource for ProcFsSource {
    fn status_text(&self) -> &str {
        &self.status
    }

    fn snapshot(&mut self) -> ProcessSnapshot {
        let sampled_at = SnapshotTime(SystemTime::now());
        let mut issues = Vec::new();
        let mut skipped = 0usize;
        let host = self.identity(
            "sys/kernel/hostname",
            IssueScope::HostIdentity,
            &mut issues,
            HostId,
        );
        let boot = self.identity(
            "sys/kernel/random/boot_id",
            IssueScope::BootIdentity,
            &mut issues,
            BootId,
        );
        let pid_namespace = self.pid_namespace(&mut issues);
        let mut records = Vec::new();
        let entries = match std::fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) => {
                // The whole scan failed: an empty list here is explicitly not
                // an authoritative "no processes" answer.
                issues.push(EnumerationIssue {
                    scope: IssueScope::Root,
                    reason: reason_for(&error),
                    detail: format!("{}: {error}", self.root.display()),
                });
                return ProcessSnapshot {
                    source: self.source.clone(),
                    sampled_at,
                    records,
                    completeness: Completeness::from_scan(skipped, issues),
                };
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    skipped += 1;
                    issues.push(EnumerationIssue {
                        scope: IssueScope::Root,
                        reason: reason_for(&error),
                        detail: format!("directory entry: {error}"),
                    });
                    continue;
                }
            };
            let name = entry.file_name();
            // Non-numeric entries are kernel state, not processes; skipping them
            // is not a degradation.
            let Some(pid) = parse_pid(name.as_encoded_bytes()) else {
                continue;
            };
            if records.len() >= MAX_RECORDS {
                skipped += 1;
                if issues.len() < crate::source::MAX_RECORDED_ISSUES {
                    issues.push(EnumerationIssue {
                        scope: IssueScope::Root,
                        reason: MissingReason::Unavailable,
                        detail: format!("record limit {MAX_RECORDS} reached"),
                    });
                }
                continue;
            }
            match read_bounded(&self.root.join(&name).join("stat")) {
                // One unreadable record is skipped with a reason; it never fails
                // the snapshot (PX-003).
                Err(error) => {
                    skipped += 1;
                    issues.push(EnumerationIssue {
                        scope: IssueScope::Process(pid),
                        reason: reason_for(&error),
                        detail: format!("{pid}/stat: {error}"),
                    });
                }
                Ok(bytes) => match parse_stat(pid, &bytes) {
                    None => {
                        skipped += 1;
                        issues.push(EnumerationIssue {
                            scope: IssueScope::Process(pid),
                            reason: MissingReason::Unavailable,
                            detail: format!("{pid}/stat is not parsable for this PID"),
                        });
                    }
                    Some((display_name, ticks)) => records.push(ProcessRecord {
                        key: ProcessKey {
                            source: self.source.clone(),
                            host: host.clone(),
                            boot: boot.clone(),
                            pid_namespace: pid_namespace.clone(),
                            pid: Observed::Known(pid),
                            creation: CreationToken::LinuxBootTicks(ticks),
                        },
                        display_name,
                    }),
                },
            }
        }
        // Directory order is not meaningful; publish a stable ascending order.
        records.sort_by_key(|record| match record.key.pid {
            Observed::Known(pid) => pid,
            Observed::Missing(_) => u32::MAX,
        });
        ProcessSnapshot {
            source: self.source.clone(),
            sampled_at,
            records,
            completeness: Completeness::from_scan(skipped, issues),
        }
    }
}

fn reason_for(error: &io::Error) -> MissingReason {
    match error.kind() {
        io::ErrorKind::PermissionDenied => MissingReason::Denied,
        _ => MissingReason::Unavailable,
    }
}

fn read_bounded(path: &Path) -> io::Result<Vec<u8>> {
    // `/proc` files report size 0, so read through a hard byte bound instead.
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_FILE_BYTES)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Accepts only a plain positive decimal PID directory name.
pub fn parse_pid(name: &[u8]) -> Option<u32> {
    if name.is_empty() || name.len() > 10 || !name.iter().all(u8::is_ascii_digit) {
        return None;
    }
    match std::str::from_utf8(name).ok()?.parse::<u32>().ok()? {
        0 => None,
        pid => Some(pid),
    }
}

fn parse_namespace(link: &str) -> Option<u64> {
    let inner = link.strip_prefix("pid:[")?.strip_suffix(']')?;
    inner.parse().ok()
}

/// Parses `comm` and the creation token out of one `/proc/<pid>/stat` line.
///
/// `comm` is raw bytes wrapped in parentheses and may itself contain spaces,
/// parentheses, control characters and invalid UTF-8 (K1). Fields are therefore
/// located from the first `(` and the **last** `)`, never by splitting on
/// whitespace, so a hostile name cannot shift the field indices. The leading PID
/// field must also match the directory this line came from.
pub fn parse_stat(pid: u32, bytes: &[u8]) -> Option<(DisplayName, u64)> {
    let open = bytes.iter().position(|byte| *byte == b'(')?;
    let close = bytes.iter().rposition(|byte| *byte == b')')?;
    if close < open {
        return None;
    }
    if parse_pid(trim_ascii(&bytes[..open])) != Some(pid) {
        return None;
    }
    let mut fields = bytes[close + 1..]
        .split(u8::is_ascii_whitespace)
        .filter(|field| !field.is_empty());
    let ticks = std::str::from_utf8(fields.nth(STARTTIME_FIELD_AFTER_COMM)?)
        .ok()?
        .parse::<u64>()
        .ok()?;
    Some((DisplayName::sanitize(&bytes[open + 1..close]), ticks))
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(start, |index| index + 1);
    &bytes[start..end]
}
