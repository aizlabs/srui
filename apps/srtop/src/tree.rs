//! The process tree (PX-018; design §§6.2, 8, 22.5, 22.7, 26).
//!
//! The tree is a second projection of the same rows the table publishes: the
//! same process instances under the same item IDs, arranged into a forest on
//! the server and published as one `Tree` model whose index space is the
//! forest's depth-first preorder, each item carrying the standard
//! `tree_depth` and `descendant_count` (PX-018-G01). The client never builds a
//! hierarchy and never holds a node per row: it indents and discloses what the
//! model says, locally (§22.5).
//!
//! A parent is never guessed. A shown process is placed under another only
//! when every one of these holds:
//!
//! - its own row was confirmed by the current scan, and its parent PID was
//!   read (`0` is the kernel's, and makes it a root);
//! - exactly one record of the **same generation** — the current scan's
//!   confirmed rows — holds that PID;
//! - both start times were read, and the parent started **no later** than the
//!   child. A parent PID held by a process that started after the child is a
//!   PID the kernel reused, never the child's parent;
//! - the parent is shown: the filters keep it.
//!
//! Anything else makes the process an explicit root, and its parent cell says
//! which rule failed ([`RootReason`]). A malformed lineage whose parents lead
//! back to themselves (possible only when start times tie, or from a source
//! that lies) is broken deterministically, at the cycle's member that orders
//! first by process identity ([`crate::sort::process_key_order`]), whatever
//! the active sort. A chain deeper than the §26 `max_model_tree_depth` allows
//! is cut where the next level would not fit: that child starts a new root,
//! placed right after the subtree it was cut from, and says so.
//!
//! Siblings — the roots, and each parent's children — keep the order of the
//! rows they come from, which is the active sort (PX-009) or else the
//! source's order, so sorting orders every level and never flattens one.
//!
//! A user thread (PX-019) is never resolved by PID: its row is placed under
//! the row of the process instance its key names — the same process, compared
//! whole — as one of that process's first children, in the order the rows
//! come in, and its parent cell says `thread of P`. A thread row is a leaf, and
//! no process is ever placed under a thread. The thread controls show a
//! thread row only with its process row, so a thread's process is always
//! shown; one that is not, or that sits on the deepest level the tree may
//! hold children on, makes the thread a root that says why.
//!
//! The forest is flattened, and every item stamped, by the SDK's
//! [`srui_sdk::TreeModel`] ([`stamp`]), so the published shape is the one the
//! runtime validates and refuses beyond its depth limit.
//!
//! The same parent rule finds a process's sampled descendants for tagging
//! (PX-020, [`descendants`]), over every process of the generation whatever
//! the filters hide.

use crate::catalogue::ColumnLayout;
use crate::projection::Row;
use crate::sort::process_key_order;
use crate::source::{MissingReason, Observed, ProcessSnapshot};
use srui_sdk::{ItemId, PropertyRef, TreeModel, TreeModelError, TreeNode, Value};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

/// The heading of the tree's last column, which states each row's parent or
/// why it has none.
pub const PARENT_HEADING: &str = "Parent";

/// Why a shown process is a root of the tree rather than under a parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RootReason {
    /// Its parent PID is 0: the kernel started it.
    Kernel,
    /// Its own parent PID could not be read, or its source collects none.
    ParentUnread(MissingReason),
    /// Its row was kept from an earlier scan that this one did not confirm, so
    /// this generation says nothing about its parent.
    Unconfirmed,
    /// No record of this generation holds the parent PID.
    ParentMissing(u32),
    /// The record holding the parent PID could not be read by this scan.
    ParentUnreadable(u32),
    /// The parent PID was listed but never read: the scan's record bound.
    ParentNotRead(u32),
    /// No record holds the parent PID, and this scan could not name every
    /// record it missed, so the parent may exist unseen.
    ParentUncertain(u32),
    /// The parent PID is held only by a row kept from an earlier scan.
    ParentUnconfirmed(u32),
    /// More than one record of this generation holds the parent PID.
    ParentAmbiguous(u32),
    /// This process's or its parent's start time could not be read, so the
    /// parent cannot be shown to have started first.
    StartUnread(u32),
    /// The process holding the parent PID started after this one: the PID was
    /// reused, and that process is not the parent.
    ParentNewer(u32),
    /// The parent is hidden by the filters.
    ParentHidden(u32),
    /// Following parents from here leads back here: the cycle is broken at
    /// this row, which names this parent PID.
    Cycle(u32),
    /// Its parent sits on the deepest level the tree may hold children on.
    DepthLimit(u32),
    /// A thread whose process row is not shown (PX-019).
    ThreadProcessNotShown(u32),
}

/// Where one shown process hangs: under the parent with this PID, or at the
/// top, for a reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Link {
    Under(u32),
    /// A thread under its process, whose PID this is (PX-019).
    ThreadOf(u32),
    Root(RootReason),
}

impl Link {
    /// The parent cell: the parent's PID, or why there is none. Fixed wording
    /// and numbers only, so no process can write into it.
    pub fn describe(&self) -> String {
        let reason = match self {
            Self::Under(parent) => return parent.to_string(),
            Self::ThreadOf(process) => return format!("thread of {process}"),
            Self::Root(reason) => reason,
        };
        match reason {
            RootReason::Kernel => "none: started by the kernel".to_string(),
            RootReason::ParentUnread(why) => format!("root: parent unread ({})", why.describe()),
            RootReason::Unconfirmed => "root: row kept from an earlier scan".to_string(),
            RootReason::ParentMissing(pid) => format!("root: parent {pid} not in this scan"),
            RootReason::ParentUnreadable(pid) => format!("root: parent {pid} unreadable"),
            RootReason::ParentNotRead(pid) => {
                format!("root: parent {pid} beyond the record limit")
            }
            RootReason::ParentUncertain(pid) => {
                format!("root: parent {pid} not seen (incomplete scan)")
            }
            RootReason::ParentUnconfirmed(pid) => {
                format!("root: parent {pid} kept from an earlier scan")
            }
            RootReason::ParentAmbiguous(pid) => format!("root: parent {pid} ambiguous"),
            RootReason::StartUnread(pid) => {
                format!("root: parent {pid} unverified (start time unread)")
            }
            RootReason::ParentNewer(pid) => {
                format!("root: parent {pid} started later (PID reused)")
            }
            RootReason::ParentHidden(pid) => format!("root: parent {pid} hidden by the filters"),
            RootReason::Cycle(pid) => format!("root: cycle through parent {pid} broken here"),
            RootReason::DepthLimit(pid) => format!("root: parent {pid} at the depth limit"),
            RootReason::ThreadProcessNotShown(pid) => {
                format!("root: thread of {pid}, which is not shown")
            }
        }
    }
}

/// A tree row's place: its level, its subtree size, and its link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreePlace {
    /// `tree_depth`: 0 for a root.
    pub depth: u64,
    /// `descendant_count`: the rows of its subtree, excluding itself.
    pub descendants: u64,
    pub link: Link,
}

impl TreePlace {
    /// The value of the tree's parent column for this row.
    pub fn parent_cell(&self) -> Value {
        Value::String(self.link.describe())
    }
}

/// What one scan could not see, which decides how an absent parent is
/// explained (PX-018). Bounded by the scan's own ledgers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanGaps {
    unreadable: BTreeSet<u32>,
    not_read: BTreeSet<u32>,
    incomplete: bool,
}

impl ScanGaps {
    /// What `snapshot` says it could not read or never reached.
    pub fn of(snapshot: &ProcessSnapshot) -> Self {
        let skipped = snapshot.completeness.skipped_records();
        Self {
            unreadable: skipped
                .map(|ledger| ledger.pids().clone())
                .unwrap_or_default(),
            not_read: snapshot.capped.pids().clone(),
            incomplete: !snapshot.capped.is_enumerable()
                || skipped.is_some_and(|ledger| !ledger.is_enumerable()),
        }
    }
}

/// The generation parents are resolved in: the server-side rows, of which
/// `unconfirmed` were kept from an earlier scan, and what the current scan
/// could not see.
pub(crate) struct Generation<'a> {
    pub unfiltered: &'a [Row],
    pub unconfirmed: &'a HashSet<ItemId>,
    pub gaps: &'a ScanGaps,
}

fn known<T: Copy>(observed: &Observed<T>) -> Option<T> {
    match observed {
        Observed::Known(value) => Some(*value),
        Observed::Missing(_) => None,
    }
}

/// The resolved link of every row of `visible`: the index of its parent among
/// `visible`, or why it has none. Cycles are not yet broken.
fn resolve(visible: &[Row], generation: &Generation<'_>) -> Vec<Result<usize, RootReason>> {
    // The current generation by PID: rows the latest scan confirmed. A
    // thread row holds its process's PID and is never anyone's parent
    // (PX-019).
    let mut by_pid: HashMap<u32, Vec<&Row>> = HashMap::new();
    let mut kept_earlier: HashSet<u32> = HashSet::new();
    for row in generation.unfiltered {
        if row.key.is_thread() {
            continue;
        }
        let Some(pid) = known(&row.key.pid) else {
            continue;
        };
        if generation.unconfirmed.contains(&row.item_id) {
            kept_earlier.insert(pid);
        } else {
            by_pid.entry(pid).or_default().push(row);
        }
    }
    let shown: HashMap<ItemId, usize> = visible
        .iter()
        .enumerate()
        .map(|(index, row)| (row.item_id, index))
        .collect();
    // Shown process rows by their whole key, for their threads (PX-019).
    let processes: HashMap<&crate::source::ProcessKey, usize> = visible
        .iter()
        .enumerate()
        .filter(|(_, row)| !row.key.is_thread())
        .map(|(index, row)| (&row.key, index))
        .collect();
    let gaps = generation.gaps;
    visible
        .iter()
        .map(|row| {
            // A thread hangs under its own process instance, whatever its
            // PID says (PX-019).
            if row.key.is_thread() {
                return processes.get(&row.key.process_key()).copied().ok_or(
                    RootReason::ThreadProcessNotShown(known(&row.key.pid).unwrap_or(0)),
                );
            }
            if generation.unconfirmed.contains(&row.item_id) {
                return Err(RootReason::Unconfirmed);
            }
            let parent = match &row.lineage.parent {
                Observed::Missing(why) => return Err(RootReason::ParentUnread(*why)),
                Observed::Known(0) => return Err(RootReason::Kernel),
                Observed::Known(parent) => *parent,
            };
            let candidates = by_pid.get(&parent).map_or(&[][..], Vec::as_slice);
            let found = match candidates {
                [] if kept_earlier.contains(&parent) => {
                    return Err(RootReason::ParentUnconfirmed(parent))
                }
                [] if gaps.unreadable.contains(&parent) => {
                    return Err(RootReason::ParentUnreadable(parent))
                }
                [] if gaps.not_read.contains(&parent) => {
                    return Err(RootReason::ParentNotRead(parent))
                }
                [] if gaps.incomplete => return Err(RootReason::ParentUncertain(parent)),
                [] => return Err(RootReason::ParentMissing(parent)),
                [only] => *only,
                _ => return Err(RootReason::ParentAmbiguous(parent)),
            };
            if found.item_id == row.item_id {
                return Err(RootReason::Cycle(parent));
            }
            match (known(&row.lineage.started), known(&found.lineage.started)) {
                (Some(child), Some(started)) if started > child => {
                    return Err(RootReason::ParentNewer(parent))
                }
                (Some(_), Some(_)) => {}
                _ => return Err(RootReason::StartUnread(parent)),
            }
            shown
                .get(&found.item_id)
                .copied()
                .ok_or(RootReason::ParentHidden(parent))
        })
        .collect()
}

/// Breaks every cycle of `links` at its member that orders first by process
/// identity, which becomes a root naming the parent PID it pointed at.
fn break_cycles(visible: &[Row], links: &mut [Result<usize, RootReason>]) {
    // 0: unvisited, 1: on the current path, 2: settled.
    let mut state = vec![0u8; links.len()];
    let mut path = Vec::new();
    for start in 0..links.len() {
        if state[start] != 0 {
            continue;
        }
        path.clear();
        let mut at = start;
        loop {
            match state[at] {
                0 => {
                    state[at] = 1;
                    path.push(at);
                    match links[at] {
                        Ok(parent) => at = parent,
                        Err(_) => break,
                    }
                }
                1 => {
                    // `at` is on the path: the path from it is a cycle.
                    let from = path.iter().position(|&member| member == at).unwrap_or(0);
                    let cut = path[from..]
                        .iter()
                        .copied()
                        .min_by(|&left, &right| {
                            process_key_order(&visible[left].key, &visible[right].key)
                                .then(left.cmp(&right))
                        })
                        .unwrap_or(at);
                    let pid = match visible[cut].lineage.parent {
                        Observed::Known(pid) => pid,
                        Observed::Missing(_) => 0,
                    };
                    links[cut] = Err(RootReason::Cycle(pid));
                    break;
                }
                _ => break,
            }
        }
        for &member in &path {
            state[member] = 2;
        }
    }
}

/// The sampled descendants of the process row `root` (PX-020): the process
/// rows of `generation.unfiltered` whose chain of parents reaches `root`
/// under exactly the rules the tree attaches a child by — each link a parent
/// PID read by this generation's scan, held by exactly one confirmed record
/// that started no later than the child — with cycles broken as the tree
/// breaks them, in preorder. The filters play no part: a descendant the
/// filters hide is a descendant still. Threads are never descendants, and a
/// row kept from an earlier scan has no parent in this generation, so none
/// is below it. Returns indexes into `generation.unfiltered`, `root`
/// excluded; empty when `root` is not one of its process rows.
pub(crate) fn descendants(generation: &Generation<'_>, root: ItemId) -> Vec<usize> {
    let processes: Vec<usize> = generation
        .unfiltered
        .iter()
        .enumerate()
        .filter(|(_, row)| !row.key.is_thread())
        .map(|(index, _)| index)
        .collect();
    let Some(start) = processes
        .iter()
        .position(|&index| generation.unfiltered[index].item_id == root)
    else {
        return Vec::new();
    };
    let visible: Vec<Row> = processes
        .iter()
        .map(|&index| generation.unfiltered[index].clone())
        .collect();
    let mut links = resolve(&visible, generation);
    break_cycles(&visible, &mut links);
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); visible.len()];
    for (index, link) in links.iter().enumerate() {
        if let Ok(parent) = link {
            children[*parent].push(index);
        }
    }
    // Cycles are broken above, so every row is reached at most once.
    let mut found = Vec::new();
    let mut stack: Vec<usize> = children[start].iter().rev().copied().collect();
    while let Some(index) = stack.pop() {
        found.push(processes[index]);
        stack.extend(children[index].iter().rev());
    }
    found
}

/// Arranges `visible` — the shown rows, in the active order — into the
/// published forest's preorder, each row's place stated, under
/// `max_levels`, the §26 `max_model_tree_depth` (levels, so the deepest
/// `tree_depth` is `max_levels - 1`, and a row with children sits at most at
/// `max_levels - 2`). Each row's value is laid out in `layout` with its
/// parent cell last. Descendant counts are left to [`stamp`], after the row
/// ceiling has chosen the published prefix.
pub(crate) fn arrange(
    visible: Vec<Row>,
    generation: &Generation<'_>,
    layout: &ColumnLayout,
    max_levels: usize,
) -> Vec<Row> {
    let mut links = resolve(&visible, generation);
    break_cycles(&visible, &mut links);
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); visible.len()];
    let mut roots = Vec::new();
    for (index, link) in links.iter().enumerate() {
        match link {
            Ok(parent) => children[*parent].push(index),
            Err(_) => roots.push(index),
        }
    }
    // A process's threads are its first children (PX-019), in their order.
    for siblings in &mut children {
        siblings.sort_by_key(|&child| !visible[child].key.is_thread());
    }
    let mut place: Vec<Option<(u64, Link)>> = vec![None; visible.len()];
    let mut order: Vec<usize> = Vec::with_capacity(visible.len());
    let max_depth = max_levels.max(1) as u64 - 1;
    for root in roots {
        // Subtrees cut at the depth limit are emitted right after the subtree
        // they were cut from, in the order they were cut.
        let mut cut: VecDeque<(usize, Link)> = VecDeque::new();
        let reason = match links[root] {
            Err(reason) => reason,
            Ok(_) => unreachable!("a root has no parent"),
        };
        cut.push_back((root, Link::Root(reason)));
        while let Some((top, link)) = cut.pop_front() {
            let mut stack = vec![(top, 0u64, link)];
            while let Some((index, depth, link)) = stack.pop() {
                place[index] = Some((depth, link));
                order.push(index);
                let pid = known(&visible[index].key.pid).unwrap_or(0);
                // A thread is linked as its process's thread (PX-019).
                let link_of = |child: usize| {
                    if visible[child].key.is_thread() {
                        Link::ThreadOf(pid)
                    } else {
                        Link::Under(pid)
                    }
                };
                if depth < max_depth {
                    for &child in children[index].iter().rev() {
                        stack.push((child, depth + 1, link_of(child)));
                    }
                } else {
                    for &child in &children[index] {
                        cut.push_back((child, Link::Root(RootReason::DepthLimit(pid))));
                    }
                }
            }
        }
    }
    debug_assert_eq!(order.len(), visible.len(), "every shown row is placed once");
    let mut slots: Vec<Option<Row>> = visible.into_iter().map(Some).collect();
    order
        .into_iter()
        .filter_map(|index| {
            let mut row = slots[index].take()?;
            let (depth, link) = place[index]?;
            row.tree = Some(TreePlace {
                depth,
                descendants: 0,
                link,
            });
            Some(row.laid_out(layout))
        })
        .collect()
}

/// Stamps every row of the preorder `rows` with its subtree size, through the
/// SDK's [`TreeModel`], which also refuses a forest deeper than `max_levels`.
///
/// `rows` must be a preorder — each depth at most one more than the previous,
/// the first a root — which [`arrange`] produces and which every prefix of it
/// still is, so the row ceiling can cut it anywhere.
pub(crate) fn stamp(rows: &mut [Row], max_levels: usize) -> Result<(), TreeModelError> {
    let mut roots: Vec<TreeNode> = Vec::new();
    let mut open: Vec<TreeNode> = Vec::new();
    let close = |open: &mut Vec<TreeNode>, roots: &mut Vec<TreeNode>| {
        let node = open.pop().expect("an open node");
        match open.last_mut() {
            Some(parent) => parent.children.push(node),
            None => roots.push(node),
        }
    };
    for row in rows.iter() {
        let depth = row.tree.as_ref().map_or(0, |place| place.depth) as usize;
        while open.len() > depth {
            close(&mut open, &mut roots);
        }
        debug_assert_eq!(open.len(), depth, "a preorder never skips a level");
        open.push(TreeNode::new(row.item_id, Value::Null));
    }
    while !open.is_empty() {
        close(&mut open, &mut roots);
    }
    let items = TreeModel::new(roots)
        .with_max_depth(max_levels)
        .into_items()?;
    for (row, item) in rows.iter_mut().zip(items) {
        debug_assert_eq!(row.item_id, item.item_id, "the SDK keeps the preorder");
        let read = |property| match item.get_property(property) {
            Some(Value::UnsignedInt(value)) => *value,
            _ => 0,
        };
        if let Some(place) = &mut row.tree {
            place.depth = read(PropertyRef::TREE_DEPTH);
            place.descendants = read(PropertyRef::DESCENDANT_COUNT);
        }
    }
    Ok(())
}

/// What the published forest holds, for the view status.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ForestSummary {
    pub rows: usize,
    /// Of `rows`, the thread rows (PX-019).
    pub threads: usize,
    pub roots: usize,
    /// Roots that are not the kernel's: each states why it has no parent.
    pub orphans: usize,
    /// Roots whose parent PID was reused by a newer process.
    pub reused: usize,
    /// Roots whose parent is not in this scan, or could not be read.
    pub absent: usize,
    /// Roots cut from a cycle or at the depth limit.
    pub malformed: usize,
    /// Roots whose parent the filters hide.
    pub hidden: usize,
    /// The deepest level shown.
    pub deepest: u64,
}

impl ForestSummary {
    /// The summary of the published preorder `rows`.
    pub(crate) fn of(rows: &[Row]) -> Self {
        let mut summary = Self {
            rows: rows.len(),
            threads: rows.iter().filter(|row| row.key.is_thread()).count(),
            ..Self::default()
        };
        for place in rows.iter().filter_map(|row| row.tree.as_ref()) {
            summary.deepest = summary.deepest.max(place.depth);
            let Link::Root(reason) = place.link else {
                continue;
            };
            summary.roots += 1;
            match reason {
                RootReason::Kernel => continue,
                RootReason::ParentNewer(_) => summary.reused += 1,
                RootReason::ParentMissing(_)
                | RootReason::ParentUnreadable(_)
                | RootReason::ParentNotRead(_)
                | RootReason::ParentUncertain(_)
                | RootReason::ParentUnconfirmed(_)
                | RootReason::ParentAmbiguous(_)
                | RootReason::ParentUnread(_)
                | RootReason::StartUnread(_)
                | RootReason::Unconfirmed => summary.absent += 1,
                RootReason::Cycle(_)
                | RootReason::DepthLimit(_)
                | RootReason::ThreadProcessNotShown(_) => summary.malformed += 1,
                RootReason::ParentHidden(_) => summary.hidden += 1,
            }
            summary.orphans += 1;
        }
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::SessionItemIds;
    use crate::source::{
        BootId, CappedRecords, Completeness, CpuUsage, CreationToken, DeterministicPrng,
        DisplayName, HostId, Lineage, PidNamespaceId, ProcessKey, ProcessRecord, SnapshotTime,
        SourceId, SystemSample, OWNER_NOT_COLLECTED,
    };
    use std::time::SystemTime;

    fn record(pid: u32, parent: u32, started: u64) -> ProcessRecord {
        ProcessRecord {
            kind: crate::source::KIND_NOT_COLLECTED,
            lineage: Lineage::new(parent, started),
            owner: OWNER_NOT_COLLECTED,
            key: ProcessKey {
                source: SourceId("tree-unit".into()),
                host: Observed::Known(HostId("h".into())),
                boot: Observed::Known(BootId("b".into())),
                pid_namespace: Observed::Known(PidNamespaceId(1)),
                pid: Observed::Known(pid),
                creation: CreationToken::LinuxBootTicks(started),
                thread: None,
            },
            display_name: DisplayName::from(format!("p{pid}").as_str()),
            resident: Observed::Known(0),
            cpu: CpuUsage::WarmingUp,
        }
    }

    fn rows(records: Vec<ProcessRecord>) -> Vec<Row> {
        let snapshot = ProcessSnapshot {
            source: SourceId("tree-unit".into()),
            sampled_at: SnapshotTime(SystemTime::UNIX_EPOCH),
            records,
            vanished: 0,
            threads: crate::source::ThreadScan::NotProvided,
            capped: CappedRecords::none(),
            completeness: Completeness::Complete,
            system: SystemSample::not_provided(),
        };
        SessionItemIds::default()
            .project(&snapshot, &ColumnLayout::default())
            .unwrap()
    }

    fn arranged(all: &[Row], max_levels: usize) -> Vec<Row> {
        let unconfirmed = HashSet::new();
        let gaps = ScanGaps::default();
        let generation = Generation {
            unfiltered: all,
            unconfirmed: &unconfirmed,
            gaps: &gaps,
        };
        let mut out = arrange(
            all.to_vec(),
            &generation,
            &ColumnLayout::default(),
            max_levels,
        );
        stamp(&mut out, max_levels).expect("an arranged forest is admitted");
        out
    }

    fn pid(row: &Row) -> u32 {
        match row.key.pid {
            Observed::Known(pid) => pid,
            Observed::Missing(_) => 0,
        }
    }

    fn started(row: &Row) -> u64 {
        match row.lineage.started {
            Observed::Known(started) => started,
            Observed::Missing(_) => u64::MAX,
        }
    }

    /// Every invariant the published forest must hold, checked against the
    /// rows it was arranged from: an independent oracle of the rules, not a
    /// restatement of the algorithm.
    fn check(input: &[Row], out: &[Row], max_levels: usize) {
        assert_eq!(out.len(), input.len(), "every row is placed once");
        let ids: HashSet<ItemId> = out.iter().map(|row| row.item_id).collect();
        assert_eq!(ids.len(), input.len());
        let by_pid: HashMap<u32, Vec<&Row>> = input.iter().fold(HashMap::new(), |mut map, row| {
            map.entry(pid(row)).or_insert_with(Vec::new).push(row);
            map
        });
        let mut open: Vec<usize> = Vec::new();
        for (index, row) in out.iter().enumerate() {
            let place = row.tree.as_ref().expect("a tree row");
            assert!(place.depth < max_levels as u64, "depth under the limit");
            if place.descendants > 0 {
                assert!(
                    place.depth + 1 < max_levels as u64,
                    "children fit the limit"
                );
            }
            // The preorder: each depth at most one more than the open path.
            assert!(
                place.depth as usize <= open.len(),
                "a preorder never skips a level"
            );
            open.truncate(place.depth as usize);
            // The subtree size: the following rows deeper than this one.
            let subtree = out[index + 1..]
                .iter()
                .take_while(|later| later.tree.as_ref().unwrap().depth > place.depth)
                .count();
            assert_eq!(place.descendants as usize, subtree, "descendant count");
            match place.link {
                Link::Under(parent_pid) => {
                    let parent = &out[*open.last().expect("a child has an open parent")];
                    assert_eq!(pid(parent), parent_pid, "the parent cell names the parent");
                    // The parent rule: the one record holding the PID in this
                    // generation, the child's own parent PID, no later start.
                    assert_eq!(row.lineage.parent, Observed::Known(parent_pid));
                    assert_eq!(by_pid[&parent_pid].len(), 1, "an unambiguous parent");
                    assert!(started(parent) <= started(row), "the parent started first");
                }
                Link::ThreadOf(_) => panic!("these forests hold no thread"),
                Link::Root(reason) => {
                    assert_eq!(place.depth, 0, "a root sits at the top");
                    match reason {
                        RootReason::Kernel => assert_eq!(row.lineage.parent, Observed::Known(0)),
                        RootReason::ParentMissing(p) => assert!(!by_pid.contains_key(&p)),
                        RootReason::ParentAmbiguous(p) => assert!(by_pid[&p].len() > 1),
                        RootReason::ParentNewer(p) => {
                            assert!(started(by_pid[&p][0]) > started(row))
                        }
                        RootReason::Cycle(_) | RootReason::DepthLimit(_) => {}
                        other => panic!("no such reason here: {other:?}"),
                    }
                }
            }
            open.push(index);
        }
        // Siblings keep the input order.
        let position: HashMap<ItemId, usize> = input
            .iter()
            .enumerate()
            .map(|(index, row)| (row.item_id, index))
            .collect();
        let mut last_child: HashMap<Option<u64>, usize> = HashMap::new();
        let mut path: Vec<u64> = Vec::new();
        for row in out {
            let place = row.tree.as_ref().unwrap();
            path.truncate(place.depth as usize);
            let parent = path.last().copied();
            if let Link::Under(_) = place.link {
                let at = position[&row.item_id];
                if let Some(previous) = last_child.insert(parent, at) {
                    assert!(previous < at, "siblings keep the active order");
                }
            }
            path.push(row.item_id.get());
        }
    }

    #[test]
    fn randomized_forests_hold_every_rule_under_every_depth_limit() {
        let mut rng = DeterministicPrng::new(0x7EE_018);
        let mut cases = 0;
        for _ in 0..400 {
            let count = rng.gen_range(1, 60) as u32;
            let duplicate = rng.gen_range(0, 4) == 0;
            let records: Vec<ProcessRecord> = (0..count)
                .map(|offset| {
                    let pid = 2 + offset;
                    // A parent among the PIDs (often), the kernel's, one no
                    // record holds, a duplicate PID, or itself; start times
                    // that tie often, so cycles form.
                    let parent = match rng.gen_range(0, 10) {
                        0 => 0,
                        1 => 10_000,
                        2 => pid,
                        _ => 2 + rng.gen_range(0, u64::from(count)) as u32,
                    };
                    record(pid, parent, rng.gen_range(0, 8))
                })
                .chain(duplicate.then(|| record(2, 0, 100)))
                .collect();
            let input = rows(records);
            for levels in [1, 2, 3, 5, 256] {
                let out = arranged(&input, levels);
                check(&input, &out, levels);
                cases += 1;
            }
        }
        assert_eq!(cases, 2000);
    }

    #[test]
    fn a_cycle_is_cut_at_its_first_identity_and_its_members_stay_together() {
        let input = rows(vec![record(9, 7, 1), record(7, 8, 1), record(8, 9, 1)]);
        let out = arranged(&input, 256);
        let shape: Vec<(u32, u64, Link)> = out
            .iter()
            .map(|row| {
                let place = row.tree.as_ref().unwrap();
                (pid(row), place.depth, place.link)
            })
            .collect();
        assert_eq!(
            shape,
            vec![
                (7, 0, Link::Root(RootReason::Cycle(8))),
                (9, 1, Link::Under(7)),
                (8, 2, Link::Under(9)),
            ]
        );
    }

    #[test]
    fn every_prefix_of_an_arrangement_is_a_forest_the_sdk_stamps() {
        let input = rows(
            (0..40)
                .map(|offset| {
                    record(
                        100 + offset,
                        if offset == 0 { 0 } else { 100 + offset / 3 },
                        u64::from(offset),
                    )
                })
                .collect(),
        );
        let unconfirmed = HashSet::new();
        let gaps = ScanGaps::default();
        let generation = Generation {
            unfiltered: &input,
            unconfirmed: &unconfirmed,
            gaps: &gaps,
        };
        let full = arrange(input.clone(), &generation, &ColumnLayout::default(), 256);
        for cut in 0..=full.len() {
            let mut prefix = full[..cut].to_vec();
            stamp(&mut prefix, 256).unwrap();
            for (index, row) in prefix.iter().enumerate() {
                let place = row.tree.as_ref().unwrap();
                let subtree = prefix[index + 1..]
                    .iter()
                    .take_while(|later| later.tree.as_ref().unwrap().depth > place.depth)
                    .count();
                assert_eq!(place.descendants as usize, subtree);
            }
        }
    }

    #[test]
    fn the_parent_cell_is_fixed_wording_and_numbers() {
        for (link, text) in [
            (Link::Under(42), "42"),
            (
                Link::Root(RootReason::Kernel),
                "none: started by the kernel",
            ),
            (
                Link::Root(RootReason::ParentNewer(7)),
                "root: parent 7 started later (PID reused)",
            ),
            (
                Link::Root(RootReason::DepthLimit(9)),
                "root: parent 9 at the depth limit",
            ),
        ] {
            assert_eq!(link.describe(), text);
        }
    }
}
