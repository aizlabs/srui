//! Tags: stable multi-selection by process instance, and descendant tagging
//! (PX-020; design §§6.2, 7.6, 8; D2 T28).
//!
//! A tag is server-side session state that marks a process instance for
//! later, independent of the selection: the native highlight stays the
//! client's (§22.5) and the table's promoted `value` stays one item
//! (PX-011-G01). Tags are kept by [`ProcessKey`], the whole process-instance
//! identity, never by PID, row index or name, so they survive sorting,
//! filtering, the view switch and the column chooser, and a newer process
//! that reuses a PID never inherits one. Only process rows are tagged: a
//! thread row (PX-019) is part of its process, and tagging one is refused.
//!
//! "Tag selected and its descendants" adds the selected process and the
//! descendants the latest sample shows under it — resolved by the process
//! tree's own parent rule ([`crate::tree::descendants`]) whatever the
//! filters hide — as individual tags, frozen at the request. It is not a
//! process group and not a promise about later children: a child started
//! after the request is never added.
//!
//! A tagged process that leaves the collection keeps a historical tag: its
//! tombstone in [`crate::lastseen::LastSeen`] (PX-021), which says whether it
//! exited or is no longer observed and when it was last observed, listed in
//! the status and never on a row. Historical tags are bounded with every
//! other tombstone, by count ([`MAX_HISTORICAL_TAGS`]), age and bytes, the
//! oldest forgotten first and counted; "Clear tags" removes every tag, live
//! and historical. A historical tag whose record says the process exited is
//! never matched again. One whose record says only "no longer observed"
//! (the row limit left the process out, or a scan could not confirm its
//! identity) is live again when a scan lists the very same instance, under
//! the same whole key, again: it was observed after all (PX-021); a newer
//! process under its PID is never tagged. At most [`MAX_TAGGED`] processes are tagged at once:
//! a request that would tag more is refused whole and says so. Nothing here
//! touches a process: there is no bulk action, and a tag means nothing
//! outside this session.

use crate::bounded;
use crate::lastseen::LastSeen;
use crate::projection::Row;
use crate::source::{Observed, ProcessKey};
use std::collections::{HashMap, HashSet};

/// The heading of the tag column, which follows the chosen columns.
pub const TAG_HEADING: &str = "Tag";
/// A tagged row's tag cell; an untagged row's is empty.
pub const TAGGED_CELL: &str = "tagged";
pub const TAG_TOGGLE_LABEL: &str = "Tag or untag selected";
pub const TAG_TOGGLE_DESCRIPTION: &str =
    "Tags the selected process, or untags it when it is tagged. Tags are kept on the server for this process instance, never for its PID, and survive sorting and filtering; they change no process.";
pub const TAG_DESCENDANTS_LABEL: &str = "Tag selected and descendants";
pub const TAG_DESCENDANTS_DESCRIPTION: &str =
    "Tags the selected process and every descendant the latest sample shows under it, filtered or not, each as its own tag. Processes started later are never added: this is not a process group.";
pub const TAGS_CLEAR_LABEL: &str = "Clear tags";
pub const TAGS_CLEAR_DESCRIPTION: &str =
    "Removes every tag, including the historical tags of processes that exited or are no longer observed. The selection is unchanged.";
/// The tags status of a shell that collects nothing.
pub const UNCOLLECTED_STATUS: &str = "Tags disabled: process collection not started";
/// The longest tags status published, in encoded bytes, before the cut mark.
pub const MAX_TAGS_STATUS_BYTES: usize = 1024;
/// The most historical tags kept; the oldest are forgotten first. They are
/// tombstones, so they share the tombstones' count bound (PX-021).
pub const MAX_HISTORICAL_TAGS: usize = crate::lastseen::MAX_TOMBSTONES;
/// The most process instances tagged at once. A request that would tag more
/// is refused whole. It bounds the set, and the bytes tagged cells add to a
/// catch-up snapshot, which the row ceiling reserves once
/// ([`crate::refresh::TAG_CELLS_RESERVE_BYTES`]).
pub const MAX_TAGGED: usize = 1024;
/// The historical tags the status names, newest first; the rest are counted.
const NAMED_HISTORY: usize = 3;
/// The most of a process name one status mention echoes.
const MAX_NAME_ECHO_BYTES: usize = 64;

/// How a process is named in the tags status: its PID or TID and its name,
/// cut, in fixed wording.
pub(crate) fn who(row: &Row) -> String {
    let subject = crate::threads::thread_subject(row).unwrap_or_else(|| match row.key.pid {
        Observed::Known(pid) => format!("PID {pid}"),
        Observed::Missing(_) => "PID unavailable".to_string(),
    });
    format!(
        "{subject} · {}",
        bounded(row.display_name.as_str(), MAX_NAME_ECHO_BYTES)
    )
}

fn plural(count: usize, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 {
        one
    } else {
        many
    }
}

/// The tag set (PX-020): the tagged process instances and what the last
/// request did. The historical tags are tombstones (PX-021).
///
/// Every change takes a new generation, which the tags status carries to the
/// client; a request or a sample whose status never committed puts the
/// previous set back, so the set never runs ahead of what the client shows.
#[derive(Debug, Clone, Default)]
pub struct Tags {
    /// Each tagged process instance, and how it was named when tagged.
    live: HashMap<ProcessKey, String>,
    generation: u64,
    /// What the last request did, until the next one.
    outcome: Option<String>,
}

/// Where the tagged processes are, for the tags status (PX-020).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TagCounts {
    /// Tagged processes hidden by the text filter, the user filter or the PID set.
    pub(crate) filtered: usize,
    /// Kernel threads hidden by "Show kernel threads".
    pub(crate) kernel_hidden: usize,
    /// Kept by every filter but beyond the publishable size limit.
    pub(crate) beyond_limit: usize,
}

/// The tags status as the client holds it, and the generation it states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TagsView {
    pub(crate) status: String,
    pub(crate) generation: u64,
}

impl Tags {
    /// Whether the process instance `key` is tagged. A thread never is.
    pub fn is_tagged(&self, key: &ProcessKey) -> bool {
        !key.is_thread() && self.live.contains_key(key)
    }

    /// How many process instances are tagged.
    pub fn tagged(&self) -> usize {
        self.live.len()
    }

    /// This set's generation: every change takes a new one.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn changed(&mut self, outcome: String) {
        self.generation += 1;
        self.outcome = Some(outcome);
    }

    /// Tags `row`, or untags it when it is tagged. A thread row is refused.
    pub(crate) fn toggle(&mut self, row: &Row) {
        if row.key.is_thread() {
            return self.refuse(format!(
                "{} is a thread: tags mark processes, so select its process row",
                who(row)
            ));
        }
        if self.live.remove(&row.key).is_some() {
            self.changed(format!("Untagged {}", who(row)));
        } else if self.live.len() >= MAX_TAGGED {
            self.refuse(format!(
                "Nothing tagged: {} not tagged, as {MAX_TAGGED} processes already are, the limit; untag or clear some first",
                who(row)
            ));
        } else {
            self.live.insert(row.key.clone(), who(row));
            self.changed(format!("Tagged {}", who(row)));
        }
    }

    /// Tags `row` and `descendants`, the processes the latest sample shows
    /// under it, each as its own tag. A thread row is refused.
    pub(crate) fn tag_with_descendants(&mut self, row: &Row, descendants: &[&Row]) {
        if row.key.is_thread() {
            return self.refuse(format!(
                "{} is a thread: tags mark processes, so select its process row",
                who(row)
            ));
        }
        debug_assert!(
            descendants
                .iter()
                .all(|descendant| !descendant.key.is_thread()),
            "a thread is never a descendant"
        );
        let mut asked: HashMap<&ProcessKey, &Row> = HashMap::with_capacity(descendants.len() + 1);
        asked.insert(&row.key, row);
        asked.extend(
            descendants
                .iter()
                .map(|descendant| (&descendant.key, *descendant)),
        );
        let already = asked
            .keys()
            .filter(|key| self.live.contains_key(**key))
            .count();
        let total = self.live.len() + asked.len() - already;
        if total > MAX_TAGGED {
            return self.refuse(format!(
                "Nothing tagged: {} and {} sampled {} would make {total} tags, above the limit of {MAX_TAGGED}",
                who(row),
                descendants.len(),
                plural(descendants.len(), "descendant", "descendants"),
            ));
        }
        for (key, tagged) in asked {
            self.live.entry(key.clone()).or_insert_with(|| who(tagged));
        }
        let mut outcome = format!(
            "Tagged {} and {} sampled {}",
            who(row),
            descendants.len(),
            plural(descendants.len(), "descendant", "descendants"),
        );
        if already > 0 {
            outcome.push_str(&format!(" ({already} already tagged)"));
        }
        outcome.push_str("; processes started later are not added");
        self.changed(outcome);
    }

    /// Removes every live tag; `historical` is how many historical tags the
    /// caller removed from the tombstones with them.
    pub(crate) fn clear(&mut self, historical: u64) {
        let live = self.live.len();
        self.live.clear();
        self.changed(format!(
            "Cleared {live} {} and {historical} historical; the selection is unchanged",
            plural(live, "tag", "tags"),
        ));
    }

    /// Tags `keys` directly, under a new generation: a test's shortcut to a
    /// set no request could build quickly.
    #[cfg(test)]
    pub(crate) fn tag_for_test(&mut self, keys: impl IntoIterator<Item = ProcessKey>) {
        self.live
            .extend(keys.into_iter().map(|key| (key, "test".to_string())));
        self.generation += 1;
    }

    /// Makes the tags of `rows` live again (PX-021): each was a historical
    /// tag whose record said only "no longer observed", and a scan lists the
    /// same instance, under the same whole key, again. The caller keeps the
    /// set within [`MAX_TAGGED`].
    pub(crate) fn retag(&mut self, rows: &[&Row]) {
        if rows.is_empty() {
            return;
        }
        for row in rows {
            self.live.insert(row.key.clone(), who(row));
        }
        let outcome = match rows {
            [row] => format!(
                "Tagged again: {} is listed again after it was no longer observed",
                who(row)
            ),
            _ => format!(
                "Tagged again: {} processes are listed again after they were no longer observed",
                rows.len()
            ),
        };
        self.changed(outcome);
    }

    /// Records a request that changed no tag, and why.
    pub(crate) fn refuse(&mut self, why: String) {
        self.changed(why);
    }

    /// The tagged process instances that are not in `collection`: they have
    /// left it. Each is named as `previous`, the collection before, last
    /// showed it, or else as it was named when tagged; in `previous`'s order,
    /// then by PID.
    pub(crate) fn departed(
        &self,
        previous: &[Row],
        collection: &[Row],
    ) -> Vec<(ProcessKey, String)> {
        if self.live.is_empty() {
            return Vec::new();
        }
        let listed: HashSet<&ProcessKey> = collection.iter().map(|row| &row.key).collect();
        let mut gone: HashMap<&ProcessKey, &String> = self
            .live
            .iter()
            .filter(|(key, _)| !listed.contains(key))
            .collect();
        if gone.is_empty() {
            return Vec::new();
        }
        let mut departed: Vec<(ProcessKey, String)> = Vec::with_capacity(gone.len());
        for row in previous {
            if gone.remove(&row.key).is_some() {
                departed.push((row.key.clone(), who(row)));
            }
        }
        let mut rest: Vec<(&ProcessKey, &String)> = gone.into_iter().collect();
        rest.sort_by_key(|(key, _)| match key.pid {
            Observed::Known(pid) => u64::from(pid),
            Observed::Missing(_) => u64::MAX,
        });
        departed.extend(
            rest.into_iter()
                .map(|(key, named)| (key.clone(), named.clone())),
        );
        departed
    }

    /// Takes the tags of `departed` off the live set (PX-020), under a new
    /// generation; their tombstones become the historical tags (PX-021).
    pub(crate) fn retire(&mut self, departed: &[(ProcessKey, String)]) {
        if departed.is_empty() {
            return;
        }
        for (key, _) in departed {
            self.live.remove(key);
        }
        self.generation += 1;
    }

    /// The tags status (PX-020): how many processes are tagged and where the
    /// ones not shown are, the historical tags kept in `last_seen` with how
    /// each left and when it was last observed (PX-021), and what the last
    /// request did, in fixed wording, numbers and cut names only.
    pub(crate) fn status(&self, counts: TagCounts, last_seen: &LastSeen) -> String {
        let mut clauses = Vec::new();
        let kept = last_seen.tagged().count();
        let forgotten = last_seen.forgotten_tags();
        if self.live.is_empty() && kept == 0 && forgotten == 0 {
            clauses.push(
                "Tags: none · a tag marks a process instance on this server and changes no process"
                    .to_string(),
            );
        } else {
            clauses.push(format!("Tags: {} tagged", self.live.len()));
            let hidden = [
                (
                    counts.filtered,
                    "hidden by the filters",
                    "hidden by the filters",
                ),
                (
                    counts.kernel_hidden,
                    "kernel thread hidden",
                    "kernel threads hidden",
                ),
                (
                    counts.beyond_limit,
                    "beyond the publishable size limit",
                    "beyond the publishable size limit",
                ),
            ];
            for (count, one, many) in hidden {
                if count > 0 {
                    clauses.push(format!("{count} {}", plural(count, one, many)));
                }
            }
            let historical = kept as u64 + forgotten;
            if historical > 0 {
                let named: Vec<String> = last_seen
                    .tagged()
                    .rev()
                    .take(NAMED_HISTORY)
                    .map(|tombstone| {
                        format!(
                            "{} {}, {}",
                            tombstone.subject(),
                            tombstone.departure().verdict(),
                            tombstone.observed_text()
                        )
                    })
                    .collect();
                // Every historical tag is counted once: the named ones, then
                // the rest, of which the forgotten ones are a part.
                let clause = if named.is_empty() {
                    // All of them are forgotten: none is kept to name.
                    if historical == 1 {
                        "1 historical, forgotten".to_string()
                    } else {
                        format!("{historical} historical, all forgotten")
                    }
                } else {
                    let mut clause = format!("{historical} historical: {}", named.join("; "));
                    let unnamed = historical - named.len() as u64;
                    if unnamed > 0 {
                        clause.push_str(&format!(" and {unnamed} more"));
                    }
                    if forgotten > 0 {
                        clause.push_str(&format!(" ({forgotten} forgotten)"));
                    }
                    clause
                };
                clauses.push(clause);
            }
        }
        if let Some(outcome) = &self.outcome {
            clauses.push(outcome.clone());
        }
        bounded(&clauses.join(" · "), MAX_TAGS_STATUS_BYTES)
    }
}
