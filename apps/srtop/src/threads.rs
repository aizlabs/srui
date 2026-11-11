//! User threads and kernel-thread visibility (PX-019; design §§6.2, 6.3, 7.6,
//! 8, 12, 22.1, 22.5).
//!
//! Two independent checkboxes choose which tasks the collection holds: **Show
//! user threads** (off by default) adds one row per thread of each shown user
//! process, and **Show kernel threads** (on by default) keeps or hides the
//! kernel's own tasks. The filters (PX-010, PX-012) then apply within that
//! collection, to processes: a thread row is shown exactly when its process
//! row is, and is never matched, counted or hidden on its own. In the table a
//! thread row follows its process row, the process's threads in the active
//! order; in the tree (PX-018) a process's threads are its first children,
//! attached by the process instance's whole key, never by PID.
//!
//! Identity: a thread row's key is its process's key plus the thread's own TID
//! and creation token ([`crate::source::ThreadIdentity`]), so a thread that
//! ends and a newcomer under the same TID get different item IDs, and so does
//! a thread of a new process under a reused PID.
//!
//! Counters: a process row shows the whole process — all its threads, and the
//! CPU time of its threads that ended — and a thread row shows that thread
//! alone. A thread row's CPU share is its own; its resident memory is its
//! process's, so it says "See process" instead of repeating it. Nothing here
//! adds rows together: the system summary counts processes from the scan's
//! process records, which never hold a thread, and the thread counts below
//! count each thread once.
//!
//! A source that reads no per-thread records — a fixture without threads, or
//! a host without a per-thread process filesystem, such as macOS — says so
//! ([`ThreadScan::NotProvided`]), and the threads status states it as a
//! capability limitation, never as zero threads. A process whose threads
//! could not be read, or that the thread bound left unread, is counted and
//! named, and its earlier thread rows are kept.

use crate::projection::Row;
use crate::source::{Observed, ProcessKey, TaskKind, ThreadScan};
use std::collections::{HashMap, HashSet};

/// The user-thread checkbox's label.
pub const USER_THREADS_LABEL: &str = "Show user threads";
pub const USER_THREADS_DESCRIPTION: &str =
    "When on, each shown process is followed by one row per thread, read on the server from the latest scan. A thread row's CPU is that thread's own and a process row's is the whole process, so never add them together; a thread's memory is its process's. The filters choose processes, and a process's threads are shown with it.";
/// The kernel-thread checkbox's label.
pub const KERNEL_THREADS_LABEL: &str = "Show kernel threads";
pub const KERNEL_THREADS_DESCRIPTION: &str =
    "When off, the kernel's own tasks are hidden. A process the source does not classify is always shown.";
/// The threads status of a shell that collects nothing.
pub const UNCOLLECTED_STATUS: &str = "Threads disabled: process collection not started";
/// The longest threads status published, in encoded bytes.
pub const MAX_THREADS_STATUS_BYTES: usize = 768;

/// Which tasks the collection holds (PX-019). Session-wide, like the filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadSettings {
    /// Show user threads.
    pub user: bool,
    /// Show kernel threads.
    pub kernel: bool,
}

impl Default for ThreadSettings {
    /// User threads hidden, kernel threads shown: the collection as it was
    /// before threads existed.
    fn default() -> Self {
        Self {
            user: false,
            kernel: true,
        }
    }
}

/// The thread controls as the client holds them (PX-019).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThreadsView {
    pub(crate) status: String,
    pub(crate) user: bool,
    pub(crate) kernel: bool,
}

/// What the last accepted sample said about user threads (PX-019): counts
/// and fixed categories only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadReport {
    /// The source reads no per-thread records.
    NotProvided,
    /// The sample did not read threads: they were hidden when it was taken.
    NotRequested,
    /// The sample read threads.
    Read {
        /// Processes whose threads could not be read.
        unreadable: usize,
        /// Of those, refused rather than failed.
        denied: usize,
        /// Processes whose threads the thread bound left unread.
        capped: usize,
        /// Whether the scan could not name every process it missed.
        incomplete: bool,
    },
}

impl ThreadReport {
    /// What `scan` says.
    pub fn of(scan: &ThreadScan) -> Self {
        match scan {
            ThreadScan::NotProvided => Self::NotProvided,
            ThreadScan::NotRequested => Self::NotRequested,
            ThreadScan::Read(read) => Self::Read {
                unreadable: read.unreadable.count(),
                denied: read.denied,
                capped: read.capped.count(),
                incomplete: !read.unreadable.is_enumerable() || !read.capped.is_enumerable(),
            },
        }
    }
}

/// Whether `row` is a kernel thread.
pub(crate) fn is_kernel_thread(row: &Row) -> bool {
    row.kind == Observed::Known(TaskKind::KernelThread)
}

/// Whether `settings` let the collection hold `row` at all, before any
/// filter: a thread row while user threads are shown, a kernel thread while
/// kernel threads are, and every other row.
pub(crate) fn admits(settings: ThreadSettings, row: &Row) -> bool {
    if row.key.is_thread() {
        settings.user
    } else if is_kernel_thread(row) {
        settings.kernel
    } else {
        true
    }
}

/// The rows of `rows` that `keeps` keeps, where `keeps` is asked about
/// process rows only: a thread row is kept exactly when its process row is.
pub(crate) fn kept_with_processes(rows: &[Row], keeps: impl Fn(&Row) -> bool) -> Vec<Row> {
    let mut kept_processes: HashSet<&ProcessKey> = HashSet::new();
    let mut kept = Vec::new();
    for row in rows {
        let keep = if row.key.is_thread() {
            kept_processes.contains(&row.key.process_key())
        } else {
            let keep = keeps(row);
            if keep {
                kept_processes.insert(&row.key);
            }
            keep
        };
        if keep {
            kept.push(row.clone());
        }
    }
    kept
}

/// `rows` with every thread row moved right after its process row, the
/// process's threads in the order they held among themselves, and every
/// process row where it was (PX-019). A thread row whose process row is not
/// in `rows` is dropped: a thread is never shown without its process. Returns
/// the rows and how many were dropped.
pub(crate) fn group(rows: Vec<Row>) -> (Vec<Row>, usize) {
    if !rows.iter().any(|row| row.key.is_thread()) {
        return (rows, 0);
    }
    let mut threads: HashMap<ProcessKey, Vec<Row>> = HashMap::new();
    let mut processes = Vec::with_capacity(rows.len());
    let total = rows.len();
    for row in rows {
        if row.key.is_thread() {
            threads.entry(row.key.process_key()).or_default().push(row);
        } else {
            processes.push(row);
        }
    }
    let mut grouped = Vec::with_capacity(total);
    for process in processes {
        let own = threads.remove(&process.key);
        grouped.push(process);
        grouped.extend(own.into_iter().flatten());
    }
    let dropped = threads.values().map(Vec::len).sum();
    (grouped, dropped)
}

/// How `row` is named in a status line, if it is a thread row: its TID and
/// its process's PID, in fixed wording (PX-019).
pub(crate) fn thread_subject(row: &Row) -> Option<String> {
    let thread = row.key.thread.as_ref()?;
    Some(match row.key.pid {
        Observed::Known(pid) => format!("TID {} of PID {pid}", thread.tid),
        Observed::Missing(reason) => format!("TID {} of PID {}", thread.tid, reason.describe()),
    })
}

/// What the threads status counts, over the rows the server holds (PX-019).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ThreadCounts {
    /// Process rows: user processes, kernel threads and unclassified ones.
    processes: usize,
    kernel: usize,
    unclassified: usize,
    /// User processes whose thread count the scan read, and the threads
    /// those counts add up to.
    counted: usize,
    threads: u64,
    /// User processes whose thread count was not read.
    uncounted: usize,
    /// Thread rows, and the processes they belong to.
    thread_rows: usize,
    with_thread_rows: usize,
}

impl ThreadCounts {
    /// The counts of `rows`, the server-side collection.
    pub(crate) fn of(rows: &[Row]) -> Self {
        let mut counts = Self::default();
        let mut owners: HashSet<ProcessKey> = HashSet::new();
        for row in rows {
            if row.key.is_thread() {
                counts.thread_rows += 1;
                owners.insert(row.key.process_key());
                continue;
            }
            counts.processes += 1;
            match &row.kind {
                Observed::Known(TaskKind::KernelThread) => counts.kernel += 1,
                Observed::Known(TaskKind::Process {
                    threads: Observed::Known(threads),
                }) => {
                    counts.counted += 1;
                    counts.threads += u64::from(*threads);
                }
                Observed::Known(TaskKind::Process { .. }) => counts.uncounted += 1,
                // A thread kind on a process row is refused by the
                // projection; an unread kind is unclassified.
                Observed::Known(TaskKind::Thread) | Observed::Missing(_) => {
                    counts.unclassified += 1
                }
            }
        }
        counts.with_thread_rows = owners.len();
        counts
    }
}

fn plural<'a>(count: impl Into<u64>, one: &'a str, many: &'a str) -> &'a str {
    if count.into() == 1 {
        one
    } else {
        many
    }
}

/// The threads status (PX-019): what each checkbox shows, the counts, and
/// every way the threads could not be read, in fixed wording and numbers
/// only, so no process can write into it. `paused` is whether the view is
/// paused, which decides when hidden-until-sampled threads appear.
pub(crate) fn status(
    settings: ThreadSettings,
    report: ThreadReport,
    counts: ThreadCounts,
    paused: bool,
) -> String {
    let mut clauses = Vec::new();
    if !settings.user {
        clauses.push(if counts.counted == 0 && counts.uncounted == 0 {
            "User threads hidden · this source states no thread counts".to_string()
        } else {
            let mut clause = format!(
                "User threads hidden · {} {} in {} user {} by the kernel's count",
                counts.threads,
                plural(counts.threads, "thread", "threads"),
                counts.counted,
                plural(counts.counted as u64, "process", "processes"),
            );
            if counts.uncounted > 0 {
                clause.push_str(&format!(
                    " ({} {} unread)",
                    counts.uncounted,
                    plural(counts.uncounted as u64, "count", "counts")
                ));
            }
            clause
        });
    } else {
        match report {
            ThreadReport::NotProvided => clauses.push(
                "User threads unavailable: this source reads no per-thread records, so rows \
                 show whole processes only"
                    .to_string(),
            ),
            ThreadReport::NotRequested => clauses.push(if paused {
                "User threads shown from the next sample · the view is paused: Resume view reads \
                 them"
                    .to_string()
            } else {
                "User threads shown from the next sample".to_string()
            }),
            ThreadReport::Read {
                unreadable,
                denied,
                capped,
                incomplete,
            } => {
                clauses.push(format!(
                    "User threads: {} thread {} under {} {} · a thread row's CPU is that thread's \
                     own, a process row's the whole process: never add both",
                    counts.thread_rows,
                    plural(counts.thread_rows as u64, "row", "rows"),
                    counts.with_thread_rows,
                    plural(counts.with_thread_rows as u64, "process", "processes"),
                ));
                if unreadable > 0 {
                    let mut clause = format!(
                        "threads of {unreadable} {} unreadable",
                        plural(unreadable as u64, "process", "processes")
                    );
                    if denied > 0 {
                        clause.push_str(&format!(" ({denied} permission denied)"));
                    }
                    clauses.push(clause);
                }
                if capped > 0 {
                    clauses.push(format!(
                        "threads of {capped} {} not read: the scan's thread limit was reached",
                        plural(capped as u64, "process", "processes"),
                    ));
                }
                if incomplete {
                    clauses.push("thread listing incomplete".to_string());
                }
            }
        }
    }
    if counts.kernel == 0 && counts.unclassified > 0 && counts.unclassified == counts.processes {
        clauses.push(
            "Kernel threads: this source does not say which processes are kernel threads"
                .to_string(),
        );
    } else {
        let mut clause = format!(
            "Kernel threads {} ({})",
            if settings.kernel { "shown" } else { "hidden" },
            counts.kernel
        );
        if counts.unclassified > 0 {
            clause.push_str(&format!(
                " · {} {} unclassified, always shown",
                counts.unclassified,
                plural(counts.unclassified as u64, "process", "processes")
            ));
        }
        clauses.push(clause);
    }
    crate::bounded(&clauses.join(" · "), MAX_THREADS_STATUS_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue::ColumnLayout;
    use crate::projection::SessionItemIds;
    use crate::source::{ProcessSource, ThreadsSource};

    fn rows(threads: bool) -> Vec<Row> {
        let mut source = ThreadsSource::default();
        source.collect_threads(threads);
        SessionItemIds::default()
            .project(&source.snapshot(), &ColumnLayout::default())
            .unwrap()
    }

    #[test]
    fn grouping_puts_threads_after_their_process_and_drops_orphans() {
        let all = rows(true);
        // Reversed: every thread before its process.
        let reversed: Vec<Row> = all.iter().rev().cloned().collect();
        let (grouped, dropped) = group(reversed);
        assert_eq!(dropped, 0);
        let order: Vec<(bool, u32)> = grouped
            .iter()
            .map(|row| {
                let tid = row.key.thread.as_ref().map_or(0, |thread| thread.tid);
                (row.key.is_thread(), tid)
            })
            .collect();
        // Processes reversed, and the server's threads, reversed among
        // themselves, right after it.
        assert_eq!(
            order,
            [
                (false, 0),
                (false, 0),
                (true, 202),
                (true, 201),
                (true, 200),
                (false, 0),
                (false, 0),
                (false, 0),
            ]
        );
        // A thread whose process is gone is never shown alone.
        let orphans: Vec<Row> = all
            .into_iter()
            .filter(|row| row.key.pid != Observed::Known(200) || row.key.is_thread())
            .collect();
        let (grouped, dropped) = group(orphans);
        assert_eq!(dropped, 3);
        assert!(grouped.iter().all(|row| !row.key.is_thread()));
    }

    #[test]
    fn the_settings_admit_tasks_independently_and_threads_follow_their_process() {
        let all = rows(true);
        for (user, kernel, expected) in [
            (false, true, 5),
            (true, true, 8),
            (false, false, 3),
            (true, false, 6),
        ] {
            let admitted = all
                .iter()
                .filter(|row| admits(ThreadSettings { user, kernel }, row))
                .count();
            assert_eq!(admitted, expected, "user {user} kernel {kernel}");
        }
        let kept = kept_with_processes(&all, |row| row.display_name.as_str() == "server");
        assert_eq!(kept.len(), 4);
        // Asked about a thread's own name, never: threads are not matched.
        let kept = kept_with_processes(&all, |row| row.display_name.as_str() == "worker-1");
        assert!(kept.is_empty());
    }

    #[test]
    fn counts_count_each_thread_once_and_the_status_is_bounded() {
        let counts = ThreadCounts::of(&rows(true));
        assert_eq!(counts.processes, 5);
        assert_eq!(counts.kernel, 2);
        assert_eq!(counts.counted, 3);
        assert_eq!(counts.threads, 6);
        assert_eq!(counts.thread_rows, 3);
        assert_eq!(counts.with_thread_rows, 1);
        let widest = ThreadCounts {
            processes: usize::MAX,
            kernel: usize::MAX,
            unclassified: usize::MAX,
            counted: usize::MAX,
            threads: u64::MAX,
            uncounted: usize::MAX,
            thread_rows: usize::MAX,
            with_thread_rows: usize::MAX,
        };
        for settings in [
            ThreadSettings {
                user: false,
                kernel: true,
            },
            ThreadSettings {
                user: true,
                kernel: false,
            },
        ] {
            let text = status(
                settings,
                ThreadReport::Read {
                    unreadable: usize::MAX,
                    denied: usize::MAX,
                    capped: usize::MAX,
                    incomplete: true,
                },
                widest,
                true,
            );
            assert!(
                text.len() <= MAX_THREADS_STATUS_BYTES + '…'.len_utf8(),
                "{text}"
            );
        }
    }
}
