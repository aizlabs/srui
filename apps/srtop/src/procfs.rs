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
    record_issue, BootId, Completeness, CreationToken, DisplayName, EnumerationIssue, HostId,
    IssueScope, MissingReason, Observed, PidNamespaceId, ProcessKey, ProcessRecord,
    ProcessSnapshot, ProcessSource, SnapshotTime, SourceId,
};
use std::fmt::Write as _;
use std::fs::File;
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The real Linux process filesystem.
pub const DEFAULT_PROC_ROOT: &str = "/proc";
/// Source-owned status label: this data is a live read of a real process
/// filesystem and is never described as a fixture.
pub const LIVE_STATUS_TEXT: &str = "Read-only · Live process snapshot";
/// Default bound on records published from one scan; entries beyond it are
/// counted as capped, never as unreadable.
pub const MAX_RECORDS: usize = 65_536;
/// Bound on every single file this scan reads.
pub const MAX_FILE_BYTES: u64 = 64 * 1024;
/// `/proc/<pid>/stat` field 22 (start time) is the 20th field after `comm`.
const STARTTIME_FIELD_AFTER_COMM: usize = 19;

/// What a scanned root is, as far as an unprivileged scan can prove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MountKind {
    /// Not a procfs mount: no `self` symlink. A fixture tree, whose own identity
    /// files are the only thing that can answer for it.
    Tree,
    /// The procfs this process runs under, so the kernel interfaces read through
    /// it describe the processes it lists.
    Own,
    /// A procfs mount this scan cannot prove is its own — or a root it cannot
    /// even classify. Identity read through it belongs to the reader, not to the
    /// records, and is reported unavailable rather than guessed.
    Unproven,
}

/// One-shot reader over a `/proc`-shaped directory tree.
#[derive(Debug, Clone)]
pub struct ProcFsSource {
    root: PathBuf,
    source: SourceId,
    status: String,
    record_limit: usize,
}

impl ProcFsSource {
    /// Reads the host's real process filesystem. Only this constructor may
    /// describe its data as live.
    pub fn live() -> Self {
        Self::rooted(
            PathBuf::from(DEFAULT_PROC_ROOT),
            LIVE_STATUS_TEXT.to_string(),
        )
    }

    /// Reads a `/proc`-shaped tree. The root is part of the source identity, so
    /// records from different roots can never alias.
    ///
    /// The status describes what was read and from where, and claims neither
    /// liveness nor synthesis. A path cannot tell the two apart: `/host/proc` is
    /// a bind mount of a real process filesystem, and calling its rows a fixture
    /// would label live host processes as synthetic — the same defect as the
    /// reverse, in the other direction.
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        // A relative root is anchored here, not at scan time. The scan happens
        // later and the process may have changed directory in between, which
        // would silently point the same source — same `SourceId`, same status
        // line — at a different tree, and make records from two trees compare
        // as one. Anchoring is textual on purpose: it does not resolve symlinks
        // or require the root to exist, because a missing root must still scan
        // and report itself unreadable rather than fail construction.
        let root = root.into();
        let root = if root.is_absolute() {
            root
        } else {
            std::env::current_dir().map_or_else(|_| root.clone(), |working| working.join(&root))
        };
        let status = format!(
            "Read-only · Process filesystem snapshot: {}",
            root.display()
        );
        Self::rooted(root, status)
    }

    fn rooted(root: PathBuf, status: String) -> Self {
        let source = source_id_for(&root);
        Self {
            root,
            source,
            status,
            record_limit: MAX_RECORDS,
        }
    }

    /// Bounds how many records one scan publishes. Entries beyond the bound are
    /// reported as capped rather than unreadable, and the scan is not complete.
    pub fn with_record_limit(mut self, limit: usize) -> Self {
        self.record_limit = limit;
        self
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
                    record_issue(issues, || EnumerationIssue {
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
                record_issue(issues, || EnumerationIssue {
                    scope,
                    reason,
                    detail: format!("{relative}: {error}"),
                });
                Observed::Missing(reason)
            }
        }
    }

    /// The PID namespace the scanned records are numbered in.
    ///
    /// `self/ns/pid` always names the *reader's* active namespace, which is the
    /// right answer only when the mount numbers PIDs the way this process is
    /// numbered. A host `/proc` bind-mounted into a container breaks exactly
    /// that: the numeric directories are scoped to the mount's namespace while
    /// the link resolves to the container's, so every key would be stamped with
    /// a namespace its PIDs do not belong to.
    ///
    /// The mount's own namespace init answers it directly when this scan may
    /// read it: `<root>/1` is the task *that mount* numbers 1, so its `ns/pid`
    /// link names the namespace the enumerated PIDs are numbered in, whatever
    /// namespace the reader is in. Reading another task's `ns/` link needs
    /// ptrace access, so an unprivileged scan of a host `/proc` normally cannot,
    /// and falls back to the reader's own link — but only with both proofs that
    /// it describes these records:
    ///
    /// * `<root>/self` names *this* process's own PID. A nested PID namespace
    ///   that inherited an outer `/proc` (`unshare --pid --fork` with no
    ///   remount) fails here: the mount still numbers this process by its outer
    ///   PID while `self/ns/pid` names the inner namespace.
    /// * `<root>/self` and `/proc/self` are the same device and inode, so the
    ///   mount is the procfs this process runs under. A procfs superblock
    ///   belongs to exactly one PID namespace; a bind-mounted host `/proc`
    ///   fails here even when the two numberings happen to agree.
    ///
    /// Failing either, the namespace is reported unavailable rather than
    /// guessed: an unknown identity component fails explicitly instead of
    /// degrading silently. A fixture tree has no `self` symlink and is not a
    /// procfs mount, so its own identity files answer for it.
    /// What the scanned root is, as far as this scan can prove. Every kernel
    /// interface reached *through* a procfs mount — `self/ns/pid`,
    /// `sys/kernel/hostname` — answers for the reader's namespaces, not the
    /// mount's, so the answer is only this mount's identity when the mount is
    /// the procfs this process runs under.
    fn mount_kind(&self) -> MountKind {
        let scanned_self = self.root.join("self");
        // A real procfs mount always has `self` as a symlink; a tree that does
        // not is not a procfs mount and answers from its own identity files.
        // `Path::is_symlink` would fold "cannot tell" into that second case, so
        // the file type is read explicitly and an error counts as unproven.
        let Ok(kind) = std::fs::symlink_metadata(&scanned_self).map(|data| data.file_type()) else {
            return MountKind::Unproven;
        };
        if !kind.is_symlink() {
            return MountKind::Tree;
        }
        let own_self = Path::new(DEFAULT_PROC_ROOT).join("self");
        let numbers_this_process = std::fs::read_link(&scanned_self)
            .ok()
            .and_then(|target| parse_pid(target.as_os_str().as_encoded_bytes()))
            == Some(std::process::id());
        let own_procfs = match (
            std::fs::metadata(&scanned_self),
            std::fs::metadata(&own_self),
        ) {
            (Ok(scanned), Ok(own)) => scanned.dev() == own.dev() && scanned.ino() == own.ino(),
            _ => false,
        };
        if readers_namespace_describes(numbers_this_process, own_procfs) {
            MountKind::Own
        } else {
            MountKind::Unproven
        }
    }

    fn pid_namespace(
        &self,
        mount: MountKind,
        issues: &mut Vec<EnumerationIssue>,
    ) -> Observed<PidNamespaceId> {
        if let Some(inode) = std::fs::read_link(self.root.join("1/ns/pid"))
            .ok()
            .and_then(|target| parse_namespace(&target.to_string_lossy()))
        {
            return Observed::Known(PidNamespaceId(inode));
        }
        if mount == MountKind::Unproven {
            record_issue(issues, || EnumerationIssue {
                scope: IssueScope::PidNamespace,
                reason: MissingReason::Unavailable,
                detail: format!(
                    "{} numbers its records in a PID namespace this scan cannot prove is its own",
                    self.root.display()
                ),
            });
            return Observed::Missing(MissingReason::Unavailable);
        }
        let path = self.root.join("self/ns/pid");
        match std::fs::read_link(&path) {
            Ok(target) => match parse_namespace(&target.to_string_lossy()) {
                Some(inode) => Observed::Known(PidNamespaceId(inode)),
                None => {
                    record_issue(issues, || EnumerationIssue {
                        scope: IssueScope::PidNamespace,
                        reason: MissingReason::Unavailable,
                        detail: "self/ns/pid is not a pid:[inode] link".into(),
                    });
                    Observed::Missing(MissingReason::Unavailable)
                }
            },
            Err(error) => {
                let reason = reason_for(&error);
                record_issue(issues, || EnumerationIssue {
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
        let mut vanished = 0usize;
        let mut capped = 0usize;
        let mount = self.mount_kind();
        // `sys/kernel/hostname` is a sysctl: the kernel answers it from the
        // *reader's* UTS namespace, whatever mount it is read through. Two
        // containers scanning one bind-mounted host `/proc` would otherwise
        // stamp the same processes with two different host identities, and one
        // of them with a hostname belonging to no process in the list.
        let host = if mount == MountKind::Unproven {
            record_issue(&mut issues, || {
                EnumerationIssue {
                scope: IssueScope::HostIdentity,
                reason: MissingReason::Unavailable,
                detail: format!(
                    "{} is a procfs mount this scan cannot prove is its own: its hostname would be the reader's",
                    self.root.display()
                ),
            }
            });
            Observed::Missing(MissingReason::Unavailable)
        } else {
            self.identity(
                "sys/kernel/hostname",
                IssueScope::HostIdentity,
                &mut issues,
                HostId,
            )
        };
        // The boot identity is not gated the same way: `random/boot_id` is one
        // value per running kernel, not per namespace, so every procfs mount on
        // this machine answers it identically.
        let boot = self.identity(
            "sys/kernel/random/boot_id",
            IssueScope::BootIdentity,
            &mut issues,
            BootId,
        );
        let pid_namespace = self.pid_namespace(mount, &mut issues);
        let mut records = Vec::new();
        let entries = match std::fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) => {
                // The whole scan failed: an empty list here is explicitly not
                // an authoritative "no processes" answer.
                record_issue(&mut issues, || EnumerationIssue {
                    scope: IssueScope::Root,
                    reason: reason_for(&error),
                    detail: format!("{}: {error}", self.root.display()),
                });
                return ProcessSnapshot {
                    source: self.source.clone(),
                    sampled_at,
                    records,
                    vanished,
                    capped,
                    completeness: Completeness::from_scan(skipped, issues),
                };
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    skipped += 1;
                    record_issue(&mut issues, || EnumerationIssue {
                        scope: IssueScope::Entry,
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
            if records.len() >= self.record_limit {
                // This entry was never read: it is omitted by the collector's own
                // bound, not because anything denied or hid it. Counting it as
                // skipped would publish a read failure that never happened.
                capped += 1;
                if capped == 1 {
                    let limit = self.record_limit;
                    record_issue(&mut issues, || EnumerationIssue {
                        scope: IssueScope::Limit,
                        reason: MissingReason::Unavailable,
                        detail: format!("record limit {limit} reached"),
                    });
                }
                continue;
            }
            match read_bounded(&self.root.join(&name).join("stat")) {
                // The process exited between listing the root and reading it.
                // Nothing was inaccessible: the record no longer exists at sample
                // time, which is ordinary churn on any busy host, so it is
                // counted apart and does not degrade the scan.
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    vanished += 1;
                }
                // One unreadable record is skipped with a reason; it never fails
                // the snapshot (PX-003).
                Err(error) => {
                    skipped += 1;
                    record_issue(&mut issues, || EnumerationIssue {
                        scope: IssueScope::Process(pid),
                        reason: reason_for(&error),
                        detail: format!("{pid}/stat: {error}"),
                    });
                }
                Ok(bytes) => match parse_stat(pid, &bytes) {
                    None => {
                        skipped += 1;
                        record_issue(&mut issues, || EnumerationIssue {
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
            vanished,
            capped,
            completeness: Completeness::from_scan(skipped, issues),
        }
    }
}

/// Whether `self/ns/pid` — always the *reader's* active namespace — describes
/// the records of the scanned mount. Both proofs are required, and each rules
/// out a different real configuration: a mount that does not number this process
/// by its own PID is numbering its records elsewhere (a nested namespace that
/// inherited an outer `/proc`), and a mount that is not the procfs this process
/// runs under belongs to another namespace even when the two numberings happen
/// to agree (a bind-mounted host `/proc`).
fn readers_namespace_describes(numbers_this_process: bool, is_own_procfs: bool) -> bool {
    numbers_this_process && is_own_procfs
}

/// The source identity of a root, lossless in that root's bytes.
///
/// `Path::display` replaces invalid UTF-8 with U+FFFD, so two roots differing
/// only in those bytes would share one `SourceId`, and their records could then
/// compare equal on every component — precisely the cross-root aliasing
/// [`ProcFsSource::with_root`] promises cannot happen. A path that is valid
/// UTF-8 keeps the readable form; anything else is hex-encoded under a distinct
/// prefix, so no byte sequence can reach the same identity by two routes.
fn source_id_for(root: &Path) -> SourceId {
    match root.as_os_str().to_str() {
        Some(text) => SourceId(format!("procfs:{text}")),
        None => {
            let bytes = root.as_os_str().as_encoded_bytes();
            let mut id = String::with_capacity("procfs-bytes:".len() + bytes.len() * 2);
            id.push_str("procfs-bytes:");
            for byte in bytes {
                let _ = write!(id, "{byte:02x}");
            }
            SourceId(id)
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
    // Reading one byte past the bound is what separates "exactly this long" from
    // "cut here": a stat line cut mid-field still parses — first `(`, last `)`,
    // twenty whitespace fields — and would mint a *wrong* creation token, a
    // stable but false process identity, from the digits that happened to fit.
    // An oversized file is therefore an error, skipped with a reason, never a
    // silently shortened record.
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("exceeds the {MAX_FILE_BYTES} byte read bound"),
        ));
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn a_relative_root_is_anchored_so_a_later_chdir_cannot_move_the_scan() {
        let source = ProcFsSource::with_root("srtop-relative-root");
        let anchored = std::env::current_dir()
            .expect("a test process has a working directory")
            .join("srtop-relative-root");
        assert_eq!(
            source.source_id(),
            &SourceId(format!("procfs:{}", anchored.display())),
            "the identity names the tree that will actually be scanned"
        );
        assert!(source
            .status_text()
            .contains(&anchored.display().to_string()));
        // An absolute root is untouched.
        assert_eq!(
            ProcFsSource::with_root(DEFAULT_PROC_ROOT).source_id(),
            &SourceId("procfs:/proc".into())
        );
    }

    #[test]
    fn roots_differing_only_in_invalid_utf8_never_share_a_source_identity() {
        let first = Path::new(OsStr::from_bytes(b"/tmp/procfs-\xff"));
        let second = Path::new(OsStr::from_bytes(b"/tmp/procfs-\xfe"));
        // Both display as the same text, which is exactly why the identity
        // cannot be built from `Path::display`.
        assert_eq!(first.display().to_string(), second.display().to_string());
        assert_ne!(source_id_for(first), source_id_for(second));
        // A readable root keeps its readable identity.
        assert_eq!(
            source_id_for(Path::new(DEFAULT_PROC_ROOT)),
            SourceId("procfs:/proc".into())
        );
        // The two encodings live in separate namespaces, so a path whose text
        // looks like an encoded one cannot collide with it.
        assert!(source_id_for(first).0.starts_with("procfs-bytes:"));
        assert_ne!(
            source_id_for(first),
            source_id_for(Path::new(&source_id_for(first).0))
        );
    }

    #[test]
    fn the_readers_namespace_describes_records_only_under_both_proofs() {
        // The mount this process runs under and is numbered by.
        assert!(readers_namespace_describes(true, true));
        // A nested PID namespace that inherited an outer `/proc`: the same
        // mount, so the same superblock, but it still numbers this process by
        // its outer PID while the link names the inner namespace.
        assert!(!readers_namespace_describes(false, true));
        // A bind-mounted host `/proc` whose numbering happens to agree: PIDs
        // coincide across namespaces, so agreement proves nothing on its own.
        assert!(!readers_namespace_describes(true, false));
        assert!(!readers_namespace_describes(false, false));
    }
}
