//! Periodic incremental refresh of the published process collection
//! (design §§8, 12.1, 13, 23; PX-004) and of the system summary beside it
//! (PX-007). The semantic node tree is built once, by
//! [`crate::initialize_from_source`]; every later tick emits nothing but model
//! mutations and — only when they actually changed — the status text and the
//! summary's scalar properties ([`crate::summary`]). Since PX-009 the
//! collection can also be sorted ([`crate::sort`]): a sort request reorders the
//! published rows, and every later refresh publishes in the active order. A row
//! that changes place keeps its item identity: it is moved, never replaced —
//! see [`plan_moves`]. Since PX-011 every publication that changes the rows
//! also re-counts the active search ([`crate::search`]) against the rows the
//! client is about to hold, in the same plan, so the search status never
//! describes a different collection than the one published with it. Since
//! PX-012 the rows shown are the ones the user filter, the PID set and the text
//! filter all keep ([`crate::owner`]), and every publication that applies them
//! also publishes the user filter's status in the same plan. Since PX-013
//! every publication that changes the rows also re-plans the selected-process
//! inspector ([`crate::inspector`]) against the rows it publishes, and the
//! inspector's detail worker publishes each detail result it accepts. Since
//! PX-015 the view can be paused ([`crate::sampling`]): a paused view
//! publishes no sample at all, every control keeps re-projecting the frozen
//! rows, and the refresh loop's one ticker follows the interval the sampling
//! controls set. Since PX-017 every publication that changes the rows also
//! re-plans follow ([`crate::follow`]) last, after every row operation: the
//! table's `keep_visible` hint (PX-017-G01) names the followed process while
//! follow is on and nothing otherwise, so the client reveals it wherever a
//! commit moves it, and follow never touches the selection. Since PX-018 the
//! rows can be shown as a process tree instead ([`crate::tree`]): the same
//! rows under the same item IDs, arranged on the server into one hierarchical
//! `Tree` model whose index space is the forest's preorder. Exactly one of the
//! two collections holds rows; a view switch empties one and fills the other
//! in one transaction, and every tree edit is one index-based transaction,
//! because a hierarchical model is judged whole after its last operation and a
//! sparse replica cannot place an identity delete (PX-018-G01).
//! Disclosure stays each client's own (§22.5): the view mirrors the collapsed
//! items each client instance reported, only to tell when the selecting
//! client's selection or followed process is hidden there. Since PX-019
//! the collection can hold user threads too ([`crate::threads`]): each thread
//! row follows its process row, in the table and under it in the tree, and the
//! thread controls decide which tasks the collection holds before any filter
//! applies. The refresh loop asks the source for threads only while they are
//! shown, and every publication that applies the controls also publishes the
//! threads status in the same plan.
//!
//! Four rules shape this module:
//!
//! * A tick that observed no user-visible change opens no transaction at all.
//!   Nothing that changes on its own — an issue count, a sequence number, the
//!   source's internal state — is ever serialized, so an unchanging source
//!   produces an idle wire (§12.2). The sample time is the one exception, and
//!   only because a client sees it: the data-freshness line publishes it, to
//!   the second (PX-007). A sort request for the order already published is the
//!   same kind of non-change: it opens nothing (PX-009).
//! * Rows are deleted only on the word of a scan that is entitled to say a
//!   process is gone. A scan that could not list the process filesystem, or
//!   whose records this app refuses to identify, is an error published over the
//!   last-known rows, never a collection that emptied itself. Entitlement is
//!   scoped to the absences a scan could not account for
//!   ([`crate::source::Retention`]): a scan that named the records it skipped
//!   still deletes the rows of processes that really ended, so one permanently
//!   denied record cannot freeze the collection and let it grow without bound.
//! * Every published change is recorded as it commits, so the view's idea of
//!   what the client holds is exactly what the store holds even when a later
//!   batch of one refresh fails.
//! * A refresh is split into transactions no client can refuse: by the §26
//!   operation bound, by the §26 items-per-operation bound, *and* by the §26
//!   frame size. The three are independent — a host that grows from a handful
//!   of rows to tens of thousands of long names needs only four operations to
//!   publish, and those four operations still encode to more than one 16 MiB
//!   frame. A transaction that the store commits and the codec then refuses to
//!   write would advance the server's revision, detach the client, and leave
//!   nothing to republish on the next tick, because the next tick diffs clean.
//! * The collection this app publishes is never larger than a client can be
//!   *sent*. Splitting the live mutations is not enough on its own: a client
//!   that attaches, or resyncs after a journal gap, is brought up by one
//!   catch-up snapshot transaction (§18, §21) that carries every cached range
//!   of the model at once and is bounded only by its operation count. A model
//!   that encodes to more than one frame is therefore a collection no fresh
//!   client can attach to at all, however carefully each refresh was split. The
//!   rows that do not fit are not published, and the status says how many.
//! * The rows that fit never depend on what the status says. The status travels
//!   in the same catch-up snapshot as the rows, so it is charged against that
//!   frame first — at a fixed reserve covering the longest status this app can
//!   emit, never at this tick's length. Charging the current text would let a
//!   failed scan, whose status is the longest of all because it carries both the
//!   failure and the retained-row clause, shrink the row budget and delete rows
//!   that same scan had just said it could not account for. A refresh that
//!   confirmed nothing and may delete nothing goes further and plans no row
//!   mutation at all.
//! * The collection is bounded in rows as well as in bytes. Retention keeps a
//!   row whose absence a scan could not account for, and a scan that cannot
//!   enumerate its uncertainty accounts for nothing, so on such a host every
//!   absent row would be kept for ever — short rows fit a frame in their tens of
//!   thousands, so the frame ceiling does not stop that growth. No refresh ever
//!   publishes more than [`MAX_PUBLISHED_ROWS`] rows: the unconfirmed rows
//!   beyond that are evicted, leading window first, and the status says how
//!   many.
//!
//! ## Why the collection is bounded rather than the snapshot chunked
//!
//! Chunked snapshot delivery is runtime work: it needs a snapshot-framing
//! signal on the wire and conformance coverage on both replicas, which is
//! tracked as PX-004-G01 and is not this app's to invent. What is this app's
//! is what it publishes, so the ceiling is applied where the rows are chosen —
//! on the start path and on every refresh alike, since both produce the model a
//! later snapshot must carry.
//!
//! ## Why the size bound holds
//!
//! Splitting by size needs a per-operation number the encoder cannot exceed,
//! not one it usually stays under, so every quantity below is an upper bound on
//! what protobuf will emit. Strings are counted in encoded UTF-8 bytes rather
//! than in characters: a display name is bounded to
//! [`crate::source::MAX_DISPLAY_NAME_CHARS`] *characters*, and one character
//! encodes to as many as four bytes, so counting characters would understate a
//! full-width name fourfold. Every length prefix is charged its widest varint,
//! every field its tag, and the ceiling itself is
//! [`srui_protocol::DEFAULT_MAX_FRAME_SIZE`] less a fixed allowance for the
//! envelope the operations travel in, so payload is never mistaken for frame.

use crate::catalogue::{ColumnLayout, LayoutError, ProcessField};
use crate::filter::{FilterGeneration, FilterQuery, PendingFilter};
use crate::follow::{Follow, FollowView};
use crate::inspector::{
    DetailRead, DetailReads, DetailReadsIdle, DetailRequest, DetailSource, HeldSelection,
    Inspector, InspectorView,
};
use crate::owner::{OwnerChoice, OwnerFilter, OwnerView};
use crate::projection::{Row, SessionItemIds};
use crate::sampling::{relabel_freshness, Frozen, IntervalError, Sampling, SamplingView};
use crate::search::{Search, SearchStep, SearchView};
use crate::sort::{self, SortOrder};
use crate::source::{ProcessSnapshot, ProcessSource, Retention};
use crate::summary::{self, CollectorError, Sample};
use crate::threads::{self, ThreadCounts, ThreadReport, ThreadSettings, ThreadsView};
use crate::tree::{self, ForestSummary, Generation, ScanGaps};
use crate::KERNEL_THREADS_TOGGLE;
use crate::{
    bounded, initialize_rows, published_status_from, sort_button_of, ColumnAction, ScanReport,
    COLUMN_CHOOSER_BUTTONS, FILTER_INPUT, FILTER_STATUS, FOLLOW_STATUS, FOLLOW_TOGGLE,
    INSPECTOR_COMMAND, INSPECTOR_DETAILS, INSPECTOR_METRICS, INSPECTOR_PROCESS, INSPECTOR_STATUS,
    INTERVAL_INPUT, LIST_SHORTCUT, MAX_PUBLISHED_LABEL_BYTES, MODEL, OWNER_ALL, OWNER_INPUT,
    OWNER_MINE, OWNER_STATUS, PID_GO, PID_INPUT, PID_SET_INPUT, SAMPLING_PAUSE, SAMPLING_REFRESH,
    SAMPLING_STATUS, SEARCH_INPUT, SEARCH_NEXT, SEARCH_PREVIOUS, SEARCH_STATUS, STATUS, TABLE,
    THREADS_STATUS, TREE, TREE_MODEL, USER_THREADS_TOGGLE, VIEW_STATUS, VIEW_TOGGLE,
};
use srui_protocol::DEFAULT_MAX_FRAME_SIZE;
use srui_sdk::{
    ItemId, ModelId, NodeId, Operation, PropertyRef, Tree, Value, Visibility, COLUMNS, ENABLED,
    KEEP_VISIBLE, LABEL, TEXT, VALUE,
};
use srui_semantic_tree::{
    DEFAULT_MAX_CACHED_ITEMS_PER_MODEL, DEFAULT_MAX_ITEMS_PER_MODEL_OPERATION,
    DEFAULT_MAX_MODEL_TREE_DEPTH,
};
use srui_sessiond::{Session, SessionError};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};
use tokio_util::sync::CancellationToken;

/// Sampling interval used unless the operator configures another one.
pub const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(1);
/// Fastest configurable interval. A shorter one would spend more of the host on
/// scanning it than on running it, and this app is a read-only observer.
pub const MIN_REFRESH_INTERVAL: Duration = Duration::from_millis(50);
/// Slowest configurable interval.
pub const MAX_REFRESH_INTERVAL: Duration = Duration::from_secs(300);

/// Bytes of a transaction's frame that carry no operation payload: the
/// `SruiMessage.transaction` tag and length prefix, and the transaction's own
/// `base_revision`, `new_revision` and `priority` fields. Each of those is one
/// tag byte plus a varint of at most ten bytes, and the length prefix of a frame
/// below 16 MiB needs at most five, so 64 is above every encoding of them. The
/// allowance exists so this module never mistakes payload for frame.
const TRANSACTION_ENVELOPE_BYTES: usize = 64;

/// The most operation payload one transaction may carry and still encode into a
/// frame every conforming decoder accepts (§26).
///
/// Derived from the protocol's own frame limit rather than from a number of this
/// app's choosing, so raising or lowering that limit moves this bound with it.
const MAX_TRANSACTION_PAYLOAD_BYTES: usize = DEFAULT_MAX_FRAME_SIZE - TRANSACTION_ENVELOPE_BYTES;

/// Bytes one operation costs a transaction beyond its own body: the
/// `Transaction.operations` tag and length prefix, and the `Operation` oneof tag
/// and length prefix. Every tag here is one byte, and no length below 16 MiB
/// needs more than five varint bytes.
const OPERATION_FRAMING_BYTES: usize = 12;

/// Bytes an operation body costs before its first item: the widest fixed header
/// any operation this module plans carries. `ModelDeleteOp` is the widest at
/// three uint64 fields plus the tag and length prefix of its packed `item_ids`;
/// `SetPropertyOp` carries a node ID and a two-field `PropertyRef` submessage;
/// `ModelInsertOp` and `ModelUpdateOp` carry only a model ID and an index. Every
/// field is a tag byte plus a varint of at most ten, so 48 is above all of them.
const OPERATION_HEADER_BYTES: usize = 48;

/// Bytes one `ModelItem` costs beyond its value: its `items` tag and length
/// prefix, and its `item_id` tag and varint.
const MODEL_ITEM_FRAMING_BYTES: usize = 17;

/// Bytes one item ID costs inside `ModelDeleteOp.item_ids`: a tag byte and a
/// ten-byte varint. That field is packed on the wire, which is smaller still.
const ITEM_ID_BYTES: usize = 11;

/// Bytes one `Value` costs beyond its own body: its oneof tag, which is two
/// bytes for the highest field numbers this module reaches, and a length prefix
/// of at most five.
const VALUE_FRAMING_BYTES: usize = 7;

/// Bytes any `Value` variant that nests no other value occupies at most: the
/// widest is a rectangle's four doubles at nine bytes each, and a 32-byte
/// resource hash is smaller still.
const SCALAR_VALUE_BYTES: usize = 64;

/// Bytes one record property costs beyond its own value: its `properties` tag
/// and length prefix and its two-field `PropertyRef` submessage.
const RECORD_PROPERTY_BYTES: usize = 24;

/// Bytes a catch-up snapshot spends on everything that is not a model row: the
/// `CREATE_MODEL` operation and one `CREATE_NODE` operation per shell node,
/// each with its framing, header and properties (§13, §18).
///
/// The shell's structure — heading, status, table, the summary's nodes, the
/// sort row with its four short-labelled buttons (PX-009), the filter row
/// (PX-010) and the column chooser's row with its thirteen short-labelled
/// buttons (PX-068) — costs a few kilobytes at most; 8 KiB is above every
/// encoding of them and leaves the ceiling insensitive to a later label. The status text and the
/// system summary are what a scan can lengthen, so each is charged separately,
/// at its own fixed reserve ([`STATUS_RESERVE_BYTES`], [`SUMMARY_RESERVE_BYTES`];
/// `the_shell_status_and_summary_fit_their_snapshot_allowances` measures all
/// three on the snapshot a client is sent).
const SNAPSHOT_SHELL_BYTES: usize = 8192;

/// Bytes one `MODEL_RESET_RANGE` operation of a snapshot costs beyond its items.
/// A snapshot emits one per [`DEFAULT_MAX_ITEMS_PER_MODEL_OPERATION`] rows.
const SNAPSHOT_RANGE_BYTES: usize = OPERATION_FRAMING_BYTES + OPERATION_HEADER_BYTES;

/// Bytes reserved for everything the status builders can add to a source's own
/// label: every clause they can emit, each stating its widest count, and the
/// widest rejection reason one of them can quote.
///
/// Reserved rather than measured. A status is charged against the snapshot frame
/// before the rows are, so measuring *this tick's* text would let a scan that
/// failed — whose status is the longest this app publishes, because it carries
/// the failure and the retained-row clause — shrink the row budget and evict
/// rows a failed scan is entitled to keep. The allowance is asserted against the
/// real builder, with every count at `usize::MAX`, by
/// `the_reserved_status_allowance_covers_the_longest_status_the_builder_can_emit`.
const MAX_STATUS_CLAUSE_BYTES: usize = 1024;

/// Bytes the published status is charged against the catch-up snapshot frame,
/// whatever this tick's status turns out to say: the widest label a source can
/// publish plus the widest set of clauses a scan can add to it.
const STATUS_RESERVE_BYTES: usize = MAX_PUBLISHED_LABEL_BYTES + MAX_STATUS_CLAUSE_BYTES;

/// Bytes the system summary is charged against the catch-up snapshot frame,
/// whatever it says this tick (PX-007): one `CREATE_NODE` per summary node, each
/// with its fixed properties and every published one at its widest.
///
/// Reserved rather than measured, for the status's reason: the summary travels
/// in the same snapshot as the rows, and its lines are longest exactly when a
/// collector error is being reported, so charging this tick's text would let a
/// failing collector shrink the row budget and delete rows no scan said had
/// ended. Every line is bounded ([`summary::MAX_SUMMARY_TEXT_BYTES`],
/// [`summary::MAX_BAR_DESCRIPTION_BYTES`]), so the widest summary is a constant,
/// and `the_summary_reserve_covers_the_widest_summary_on_the_real_encoder`
/// asserts this allowance against it, on the planner's own accounting and on
/// the protocol's encoder.
const SUMMARY_RESERVE_BYTES: usize = 8 * 1024;

/// Bytes reserved for the command filter input and status in a catch-up snapshot (PX-010):
/// one `TextInput` and one status `Text` with their properties and max query/status lengths.
pub const FILTER_RESERVE_BYTES: usize = 2048;

/// Bytes reserved for the search controls in a catch-up snapshot (PX-011): the
/// row, the search and PID `TextInput`s with their labels and bounded values,
/// the three buttons and the status line at
/// [`crate::search::MAX_SEARCH_STATUS_BYTES`], and the table's promoted
/// selection (one `ItemId` `value`, PX-011-G01). Charged at this fixed reserve,
/// like the status and the filter, so a long search status can never shrink
/// the row budget. `the_shell_status_and_summary_fit_their_snapshot_allowances`
/// measures it with every search value at its widest.
pub const SEARCH_RESERVE_BYTES: usize = 4096;

/// Bytes reserved for the user filter and PID set in a catch-up snapshot
/// (PX-012): the row, the two buttons, the user and PID-set `TextInput`s with
/// their labels, descriptions and values at [`crate::owner::MAX_OWNER_INPUT_BYTES`]
/// and [`crate::owner::MAX_PID_SET_INPUT_BYTES`], and the status line at
/// [`crate::owner::MAX_OWNER_STATUS_BYTES`]. Charged at this fixed reserve, so
/// a long status can never shrink the row budget;
/// `the_shell_status_and_summary_fit_their_snapshot_allowances` measures it
/// with every value at its widest.
pub const OWNER_RESERVE_BYTES: usize = 4096;

/// Bytes reserved for the selected-process inspector in a catch-up snapshot
/// (PX-013): its Column with its description, and four status `Text`s, each
/// at [`crate::inspector::MAX_INSPECTOR_LINE_BYTES`] plus the cut mark.
/// Charged at this fixed reserve, so a long name or a long line can never
/// shrink the row budget;
/// `the_shell_status_and_summary_fit_their_snapshot_allowances` measures it
/// with every line at its widest.
pub const INSPECTOR_RESERVE_BYTES: usize = 6144;

/// Bytes reserved for the inspector's command-line view in a catch-up snapshot
/// (PX-014): one `RichText` with its label, description and read-only flag,
/// and its text at [`crate::command::MAX_COMMAND_TEXT_BYTES`]. Charged at this
/// fixed reserve, so a long command line can never shrink the row budget;
/// `the_shell_status_and_summary_fit_their_snapshot_allowances` measures it
/// with the text at its widest.
pub const COMMAND_RESERVE_BYTES: usize = 10_240;

/// Bytes reserved for the sampling controls in a catch-up snapshot (PX-015):
/// the row, the Pause view/Resume view, Refresh now and Apply interval
/// buttons, the interval `TextInput` with its label, placeholder, description
/// and a value of at most [`crate::sampling::MAX_INTERVAL_INPUT_BYTES`], and
/// the status line at [`crate::sampling::MAX_SAMPLING_STATUS_BYTES`]. Charged
/// at this fixed reserve, so a long sampling status can never shrink the row
/// budget; `the_shell_status_and_summary_fit_their_snapshot_allowances`
/// measures it with every value at its widest.
pub const SAMPLING_RESERVE_BYTES: usize = 2048;

/// Bytes reserved for the follow controls in a catch-up snapshot (PX-017):
/// the row, the "Follow selected process" `Toggle` with its label,
/// description, shortcut and value, and the status line at
/// [`crate::follow::MAX_FOLLOW_STATUS_BYTES`], and the table's `keep_visible`
/// hint (one `ItemId`, PX-017-G01). Charged at this fixed reserve, so a long
/// follow status can never shrink the row budget;
/// `the_shell_status_and_summary_fit_their_snapshot_allowances` measures it
/// with the status at its widest.
pub const FOLLOW_RESERVE_BYTES: usize = 2048;

/// Bytes reserved for the view switch and the process tree in a catch-up
/// snapshot (PX-018): the row, the "Show process tree" `Toggle` with its
/// label, description, shortcut and value, the view status at
/// [`MAX_VIEW_STATUS_BYTES`], the tree's `CREATE_MODEL`, and the `Tree` node
/// with its label, headings, selection mode, visibility, shortcut, promoted
/// selection and `keep_visible` hint. Charged at this fixed reserve in both
/// views, so switching views never changes the row budget's overheads;
/// `the_shell_status_and_summary_fit_their_snapshot_allowances` measures it.
pub const VIEW_RESERVE_BYTES: usize = 4096;

/// The longest view status, before the cut mark (PX-018).
pub const MAX_VIEW_STATUS_BYTES: usize = 512;
/// The thread controls' allowance in the catch-up snapshot (PX-019): the
/// row, the two checkboxes with their fixed labels and descriptions, and the
/// threads status at its widest. Charged at this fixed reserve, never at the
/// status's current length.
pub const THREADS_RESERVE_BYTES: usize = 4096;

/// What a model item of the process tree carries beyond a table row's: the
/// two standard item properties `tree_depth` and `descendant_count`, each a
/// property record holding one unsigned integer (PX-018).
const TREE_ITEM_PROPERTIES_BYTES: usize =
    2 * (RECORD_PROPERTY_BYTES + VALUE_FRAMING_BYTES + SCALAR_VALUE_BYTES);

/// The view status while the table is shown (PX-018).
pub const TABLE_VIEW_STATUS: &str =
    "View: process table · Show process tree arranges the same processes under their parents";
/// The view status of a shell that collects nothing (PX-018).
pub const UNCOLLECTED_VIEW_STATUS: &str = "View disabled: process collection not started";

/// Which collection shows the processes (PX-018): the table, or the process
/// tree. Exactly one is shown and holds rows; the other's model is empty, so a
/// catch-up snapshot never carries the collection twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Table,
    Tree,
}

impl ViewMode {
    /// The collection node of this view.
    pub fn node(self) -> NodeId {
        match self {
            Self::Table => TABLE,
            Self::Tree => TREE,
        }
    }

    /// The model the collection node references.
    pub fn model(self) -> ModelId {
        match self {
            Self::Table => MODEL,
            Self::Tree => TREE_MODEL,
        }
    }
}

/// Bytes one `CLEAR_PROPERTY` settlement adds when a commit removes the
/// promoted row (PX-011-G01) or the hinted row (PX-017-G01): a clear carries
/// no value.
const PROMOTION_CLEAR_BYTES: usize = OPERATION_FRAMING_BYTES + OPERATION_HEADER_BYTES;

/// The most of a rejection reason the status quotes. The reason is this app's
/// own error text, not a record's, and it is bounded all the same so that the
/// status stays inside [`MAX_STATUS_CLAUSE_BYTES`] whatever a later error says.
const MAX_REJECTION_BYTES: usize = 128;

/// The most rows this app ever publishes, however a scan degrades and however
/// long it stays degraded (§26; PX-004 review round 7).
///
/// The frame ceiling bounds a collection of *wide* rows, and the scan's own
/// ledgers bound the identities one scan can leave uncertain — but a ledger that
/// overflows reports itself unenumerable, and a scan that cannot enumerate its
/// uncertainty keeps every absent row. Short rows fit a frame in their tens of
/// thousands, so without a bound on the rows carried forward, ordinary churn on
/// a host whose ledger overflows grows the collection past the model's own §26
/// cached-item limit, after which every insert is refused and no refresh can
/// converge again. This is that bound, and it is a bound on the published
/// collection rather than on one scan's memory, so no ledger, issue list or
/// retention decision can lift it.
pub const MAX_PUBLISHED_ROWS: usize = DEFAULT_MAX_CACHED_ITEMS_PER_MODEL;
const _: () = assert!(MAX_PUBLISHED_ROWS <= DEFAULT_MAX_CACHED_ITEMS_PER_MODEL);

/// An upper bound on the bytes `value` occupies inside an encoded operation,
/// its own tag and length prefix included.
///
/// A string is charged its encoded UTF-8 length, never its character count: the
/// display name in a row is bounded in characters, and one character encodes to
/// as many as four bytes. Lists and records recurse; every other variant nests
/// nothing and fits in [`SCALAR_VALUE_BYTES`].
fn value_wire_bytes(value: &Value) -> usize {
    VALUE_FRAMING_BYTES
        + match value {
            Value::String(text) => text.len(),
            Value::List(values) => values.iter().map(value_wire_bytes).sum(),
            Value::Record(record) => {
                RECORD_PROPERTY_BYTES
                    + record
                        .properties
                        .iter()
                        .map(|property| RECORD_PROPERTY_BYTES + value_wire_bytes(&property.value))
                        .sum::<usize>()
            }
            _ => SCALAR_VALUE_BYTES,
        }
}

/// An upper bound on the bytes one row occupies inside a `MODEL_INSERT` or
/// `MODEL_UPDATE` operation.
fn row_wire_bytes(row: &Row) -> usize {
    MODEL_ITEM_FRAMING_BYTES
        + value_wire_bytes(&row.value)
        + row.tree.as_ref().map_or(0, |_| TREE_ITEM_PROPERTIES_BYTES)
}

/// What one refresh published.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Refreshed {
    pub inserted: usize,
    pub deleted: usize,
    pub updated: usize,
    /// Rows that changed place and kept their item identity (PX-009): each is
    /// deleted and reinserted under its own item ID in one transaction, and is
    /// counted here rather than in `inserted` or `deleted`.
    pub moved: usize,
    /// Rows the scan did not confirm and that were kept rather than deleted.
    pub retained: usize,
    /// Rows the scan did not confirm and that were dropped anyway, because
    /// carrying them would publish more than [`MAX_PUBLISHED_ROWS`] rows.
    pub evicted: usize,
    /// Rows this app declined to publish because the resulting collection would
    /// not fit in one catch-up snapshot frame, or would hold more rows than a
    /// model may cache (§18, §26).
    pub truncated: usize,
    pub status_changed: bool,
    /// Summary properties this refresh set or cleared (PX-007).
    pub summary: usize,
    /// Sort-button labels a sort request set (PX-009), filter controls
    /// (PX-010), and the table headings and chooser buttons a column layout
    /// change set (PX-068).
    pub controls: usize,
    /// Transactions this refresh committed. Zero means the snapshot held no
    /// user-visible change and nothing was sent.
    pub transactions: usize,
}

impl Refreshed {
    pub fn published(&self) -> bool {
        self.transactions > 0
    }
}

/// The rows, status and summary this session has published, and the identity
/// allocator that owns the rows' item IDs.
pub struct ProcessView {
    ids: SessionItemIds,
    unfiltered_rows: Vec<Row>,
    rows: Vec<Row>,
    status: String,
    filter_status: String,
    /// The summary's properties as the client holds them (PX-007).
    summary: summary::Properties,
    /// The last successful sample, which the summary shows until another
    /// replaces it. Server-side memory only: what reaches the wire is
    /// `summary`.
    sample: Option<Sample>,
    /// The most rows this view publishes: [`MAX_PUBLISHED_ROWS`], further
    /// lowered to whatever this session's store will cache.
    published_limit: usize,
    /// The order the collection is published in (PX-009), or `None` until the
    /// first sort request: the source's own order, as before sorting existed.
    /// Every refresh publishes its target in this order, so a snapshot sampled
    /// before a newer request is still published in the newer order.
    sort: Option<SortOrder>,
    /// The active committed filter query (PX-010).
    committed_filter: FilterQuery,
    /// Generation of the active committed filter query (PX-010).
    committed_generation: FilterGeneration,
    /// Counter allocating monotonic generations for staged queries (PX-010).
    filter_generation: FilterGeneration,
    /// Currently staged pending filter awaiting commit (PX-010).
    pending_filter: Option<PendingFilter>,
    /// The visible columns the table's headings and every row's cells are
    /// published in (PX-068). Every refresh projects its rows in it.
    columns: ColumnLayout,
    /// The server's search state (PX-011): query, PID field and the current
    /// match, held by item ID.
    search: Search,
    /// The search controls as the client holds them (PX-011).
    search_published: SearchView,
    /// Item IDs of the server-side rows the latest scan did not confirm (they
    /// are retained from an earlier scan). Bounded by the collection, which is
    /// bounded by [`MAX_PUBLISHED_ROWS`]; replaced, never grown, on every
    /// refresh. A PID jump never selects one of these (PX-011).
    unconfirmed: HashSet<ItemId>,
    /// The table's promoted selection (PX-011-G01) as the store holds it after
    /// the last commit: set only by an explicit search, withdrawn by a follow
    /// retarget that names another process (PX-017), cleared by settlement
    /// when its row leaves the model.
    promoted: Option<ItemId>,
    /// The table's `keep_visible` hint (PX-017-G01) as the store holds it
    /// after the last commit: the followed process while follow is on (PX-017),
    /// cleared by settlement when its row leaves the model.
    keep_visible: Option<ItemId>,
    /// Follow selected process (PX-017).
    follow: Follow,
    /// The follow controls as the client holds them (PX-017).
    follow_published: FollowView,
    /// The user filter, the PID set and the account-name cache (PX-012).
    owner: OwnerFilter,
    /// The user-filter controls as the client holds them (PX-012).
    owner_published: OwnerView,
    /// The selected-process inspector's target, generation and details, and
    /// its detail worker once installed (PX-013).
    inspector: Inspector,
    /// The inspector's lines as the client holds them (PX-013).
    inspector_published: InspectorView,
    /// The interval, the pause state and the refresh loop's requests (PX-015).
    sampling: Sampling,
    /// The sampling controls as the client holds them (PX-015).
    sampling_published: SamplingView,
    /// Which collection shows the processes (PX-018), as the store holds it
    /// after the last commit. `rows` are that collection's model, in its
    /// index order: the table's rows in the active order, or the tree's
    /// preorder.
    mode: ViewMode,
    /// What the latest scan could not see (PX-018), which explains an absent
    /// parent. Replaced, never grown, by every sample.
    gaps: ScanGaps,
    /// The tree items each client last reported collapsed (PX-018), through
    /// `EXPANSION_CHANGED`, by the client instance that reported them.
    /// Disclosure is each client's own (§22.5): this is only what the server
    /// was told, used to say when a followed or selected process is hidden
    /// under a collapsed parent on the client whose selection it is
    /// (`holder`). A client that never reported is all expanded, as a fresh
    /// `OutlineCollectionAdapter` is. Bounded by the published rows and
    /// [`MAX_DISCLOSURE_CLIENTS`], pruned to the rows on every publication;
    /// emptied with the tree.
    disclosure: Disclosure,
    /// Whose selection the inspector and follow describe (PX-018): the client
    /// that reported it, or every client for a promotion made in process.
    /// Only that client's disclosure can hide it.
    holder: Holder,
    /// The latest revealing promotion committed in the tree (PX-018, Codex
    /// review round 5): every client applies it after the reports it sent
    /// before it, and expands the collapsed parents above its match then.
    revealing: Option<Revealing>,
    /// The §26 `max_model_tree_depth` the tree is arranged under (PX-018): the
    /// store's own limit, at most the wire default every client enforces.
    tree_levels: usize,
    /// The view status as the client holds it (PX-018).
    view_status: String,
    /// What the status line is built from (PX-004, PX-018): the source's own
    /// label and the last scan's report, and the counts last stated. A
    /// republish that changes which rows are shown — a view switch, a sort, a
    /// filter — restates the status from these with its own row counts, so a
    /// row the ceiling newly leaves out is counted in that commit, not only at
    /// the next sample.
    status_source: String,
    status_scan: ScanReport,
    status_counts: RefreshCounts,
    /// Which tasks the collection holds (PX-019): user threads and kernel
    /// threads, each shown or hidden. Session-wide, like the filters.
    threads: ThreadSettings,
    /// The thread controls as the client holds them (PX-019).
    threads_published: ThreadsView,
    /// What the last accepted sample said about user threads (PX-019).
    /// Replaced, never grown, by every sample.
    thread_report: ThreadReport,
}

/// The rows every active filter keeps, and what the filters' controls say
/// about them (PX-010, PX-012).
struct Visible {
    /// The kept rows, bounded to what one snapshot can carry.
    rows: Vec<Row>,
    /// How many rows every filter keeps, before that bound.
    matched: usize,
    /// Of `matched`, the process rows (PX-019): what the user filter and the
    /// summary count, which never count a thread.
    matched_processes: usize,
    /// How many rows the thread controls let the collection hold, before any
    /// filter (PX-019): the text filter's total.
    total: usize,
    /// How many kept rows the bound left out.
    truncated: usize,
    /// Whether any filter is active.
    restricted: bool,
    /// Whether the user filter or the PID set is active.
    beyond_text: bool,
    owner: OwnerView,
    /// What the published forest holds, in the tree view (PX-018).
    forest: Option<ForestSummary>,
}

/// Why a column layout request was not published (PX-068).
#[derive(Debug)]
pub enum ColumnsError {
    /// The request is not a layout; nothing was committed.
    Layout(LayoutError),
    /// A transaction failed; the view holds what did commit.
    Session(SessionError),
}

impl std::fmt::Display for ColumnsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Layout(error) => write!(formatter, "not a column layout: {error}"),
            Self::Session(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for ColumnsError {}

impl From<SessionError> for ColumnsError {
    fn from(error: SessionError) -> Self {
        Self::Session(error)
    }
}

/// The table's headings for `layout`: each visible column's catalogued
/// heading, in the layout's order, so a heading always names the cell beneath
/// it (PX-042, PX-068).
pub fn column_headings(layout: &ColumnLayout) -> Vec<Value> {
    layout
        .fields()
        .iter()
        .map(|field| Value::String(field.heading().into()))
        .collect()
}

impl ProcessView {
    fn published(&mut self) -> Published<'_> {
        Published {
            rows: &mut self.rows,
            status: &mut self.status,
            summary: &mut self.summary,
            sort: &mut self.sort,
            filter_status: &mut self.filter_status,
            columns: &mut self.columns,
            search: &mut self.search_published,
            promoted: &mut self.promoted,
            keep_visible: &mut self.keep_visible,
            owner: &mut self.owner_published,
            inspector: &mut self.inspector_published,
            sampling: &mut self.sampling_published,
            follow: &mut self.follow_published,
            mode: &mut self.mode,
            view_status: &mut self.view_status,
            threads: &mut self.threads_published,
        }
    }

    /// This store's mutation bounds for a plan that may remove rows. While the
    /// table holds a promoted selection (PX-011-G01) or a `keep_visible` hint
    /// (PX-017-G01), the SDK's settlement adds one `CLEAR_PROPERTY` for each
    /// to the transaction that removes its row, and those operations count
    /// against the same operation and frame bounds, so one operation and its
    /// bytes are left free for each.
    fn planning_limits(&self, session: &Session) -> (usize, usize, usize) {
        let (batch, max_ops) = mutation_limits(session);
        let held = usize::from(self.promoted.is_some()) + usize::from(self.keep_visible.is_some());
        (
            batch,
            max_ops.saturating_sub(held).max(1),
            MAX_TRANSACTION_PAYLOAD_BYTES - held * PROMOTION_CLEAR_BYTES,
        )
    }

    /// Publishes the shell and the first snapshot in one transaction, and keeps
    /// the state a later [`Self::refresh`] diffs against. The sampling
    /// controls state [`DEFAULT_REFRESH_INTERVAL`].
    pub fn start(
        session: &Session,
        source: &mut impl ProcessSource,
    ) -> Result<(Self, ProcessSnapshot), Box<dyn std::error::Error>> {
        Self::start_with_interval(session, source, DEFAULT_REFRESH_INTERVAL)
    }

    /// [`Self::start`], with the sampling controls stating `interval` from
    /// the first transaction (PX-015), so a loop polling at that interval
    /// publishes nothing to adopt it.
    pub fn start_with_interval(
        session: &Session,
        source: &mut impl ProcessSource,
        interval: Duration,
    ) -> Result<(Self, ProcessSnapshot), Box<dyn std::error::Error>> {
        let sampling = Sampling::new(interval.clamp(MIN_REFRESH_INTERVAL, MAX_REFRESH_INTERVAL));
        let source_status = source.status_text().to_string();
        let snapshot = source.snapshot();
        let mut ids = SessionItemIds::default();
        // The shell starts in the default layout (PX-068).
        let columns = ColumnLayout::default();
        let unfiltered_rows = ids.project(&snapshot, &columns)?;
        // The thread controls start at their defaults (PX-019): no thread row
        // is shown, whatever the first sample read, and the kernel's are.
        let threads = ThreadSettings::default();
        let thread_report = ThreadReport::of(&snapshot.threads);
        // The user filter starts at every visible user, and its cache starts
        // naming the daemon's own account at once (PX-012).
        let owner = OwnerFilter::new(&source.accounts());
        let committed_filter = FilterQuery::default();
        // Both bounds are this store's as well as the protocol's: a session
        // minted with a lower cached-item limit publishes fewer rows, never rows
        // its own store would refuse.
        let published_limit = session
            .with_store(|store| MAX_PUBLISHED_ROWS.min(store.limits().max_cached_items_per_model));
        // The start path is exactly the attach path the ceiling exists for: the
        // first publication is one transaction, and every client that attaches
        // later is brought up by one snapshot of this same model. The status is
        // charged its fixed reserve, never its current length, so the rows that
        // fit do not depend on what this tick's status happens to say.
        let mut rows: Vec<Row> = unfiltered_rows
            .iter()
            .filter(|row| threads::admits(threads, row))
            .cloned()
            .collect();
        let truncated = bound_to_publishable_collection(&mut rows, published_limit);
        let status_counts = RefreshCounts {
            truncated,
            ..RefreshCounts::default()
        };
        let status = refresh_status(&source_status, &snapshot, &status_counts);
        debug_assert!(status.len() <= STATUS_RESERVE_BYTES);
        // The first sample is shown if it is one; a first scan that could not
        // list its processes publishes a collector error and no figure.
        let (sample, error) = match Sample::of(&snapshot) {
            Ok(sample) => (Some(sample), None),
            Err(error) => (None, Some(error)),
        };
        let summary = summary::properties(&summary::summarize(&summary::State::Collecting {
            last: sample.as_ref(),
            error,
            source: &snapshot.source,
            filter_matches: None,
        }));
        let owner_published = {
            let scope = owner.scope(&unfiltered_rows, &committed_filter);
            let shown = rows.iter().filter(|row| !row.key.is_thread()).count();
            owner.view(&scope, &unfiltered_rows, shown, shown + truncated)
        };
        let threads_published = ThreadsView {
            status: threads::status(
                threads,
                thread_report,
                ThreadCounts::of(&unfiltered_rows),
                false,
            ),
            user: threads.user,
            kernel: threads.kernel,
        };
        initialize_rows(
            session,
            rows.iter().map(Row::to_model_item).collect(),
            rows.len() + truncated,
            &status,
            &summary,
            &owner_published,
            true,
            Some(&sampling),
            &threads_published.status,
        )?;
        let sampling_published = sampling.view();
        ids.retain(&unfiltered_rows);
        let committed_generation = FilterGeneration::INITIAL;
        let filter_status = committed_filter.format_committed_status(
            committed_generation,
            rows.len(),
            rows.len() + truncated,
            rows.len() + truncated,
        );
        Ok((
            Self {
                ids,
                unfiltered_rows,
                rows,
                status,
                filter_status,
                summary,
                sample,
                published_limit,
                sort: None,
                committed_filter,
                committed_generation,
                filter_generation: FilterGeneration::INITIAL,
                pending_filter: None,
                columns,
                search: Search::default(),
                search_published: SearchView::initial(true),
                unconfirmed: HashSet::new(),
                promoted: None,
                keep_visible: None,
                follow: Follow::default(),
                follow_published: FollowView::initial(true),
                owner,
                owner_published,
                inspector: Inspector::default(),
                inspector_published: InspectorView::initial(true),
                sampling,
                sampling_published,
                mode: ViewMode::Table,
                gaps: ScanGaps::of(&snapshot),
                disclosure: Disclosure::default(),
                holder: Holder::Every,
                revealing: None,
                tree_levels: session.with_store(|store| {
                    store
                        .limits()
                        .max_model_tree_depth
                        .min(DEFAULT_MAX_MODEL_TREE_DEPTH)
                }),
                view_status: TABLE_VIEW_STATUS.to_string(),
                status_source: source_status,
                status_scan: ScanReport::of(&snapshot),
                status_counts,
                threads,
                threads_published,
                thread_report,
            },
            snapshot,
        ))
    }

    /// Lowers this view's published-row ceiling.
    ///
    /// The ceiling can only ever be lowered: whatever a caller asks for, no view
    /// publishes more than [`MAX_PUBLISHED_ROWS`] rows, or more than its own
    /// store will cache. It exists so a test can drive the eviction that ceiling
    /// causes without publishing a hundred thousand rows per tick.
    #[must_use]
    pub fn with_published_limit(mut self, limit: usize) -> Self {
        self.published_limit = self.published_limit.min(limit.max(1));
        self
    }

    /// The most rows this view will publish.
    pub fn published_limit(&self) -> usize {
        self.published_limit
    }

    /// Lowers the levels the process tree may hold (PX-018).
    ///
    /// Like the row ceiling, it can only be lowered: never above the store's
    /// own `max_model_tree_depth` or the wire default every client enforces,
    /// and never below one level. It exists so a test can drive the depth
    /// limit without a chain of 256 processes.
    #[must_use]
    pub fn with_tree_levels(mut self, levels: usize) -> Self {
        self.tree_levels = self.tree_levels.min(levels.max(1));
        self
    }

    /// The levels the process tree may hold (PX-018).
    pub fn tree_levels(&self) -> usize {
        self.tree_levels
    }

    /// Which collection shows the processes (PX-018).
    pub fn view_mode(&self) -> ViewMode {
        self.mode
    }

    /// The view status this view last published (PX-018).
    pub fn view_status(&self) -> &str {
        &self.view_status
    }

    /// The tree items `client` last reported collapsed (PX-018); empty for a
    /// client that reported none, which shows the tree all expanded.
    pub fn collapsed_by(&self, client: &[u8]) -> HashSet<ItemId> {
        self.disclosure.of(client).cloned().unwrap_or_default()
    }

    /// How many clients' disclosure this view mirrors (PX-018).
    pub fn disclosure_clients(&self) -> usize {
        self.disclosure.clients.len()
    }

    /// Samples the source once and publishes the difference.
    pub fn refresh(
        &mut self,
        session: &Session,
        source: &mut impl ProcessSource,
    ) -> Result<Refreshed, SessionError> {
        let source_status = source.status_text().to_string();
        // User threads are read only while they are shown (PX-019).
        source.collect_threads(self.threads_requested());
        let snapshot = source.snapshot();
        self.apply(session, &source_status, &snapshot)
    }

    /// Publishes the difference between the published rows and `snapshot`.
    ///
    /// Sampling is separated from publishing so a caller can sample off the
    /// async runtime: reading a host's process filesystem is blocking work.
    pub fn apply(
        &mut self,
        session: &Session,
        source_status: &str,
        snapshot: &ProcessSnapshot,
    ) -> Result<Refreshed, SessionError> {
        // A paused view publishes no sample: its rows, cells and summary stay
        // those of the revision it was paused at (PX-015). The loop takes no
        // sample while paused; this refuses one that reaches it all the same.
        if self.sampling.frozen().is_some() {
            self.sampling.discard();
            return Ok(Refreshed::default());
        }
        // This sample ends a resume: the sampling status says live again.
        self.sampling.published_sample();
        // A snapshot this app cannot identify is not evidence that any process
        // ended. It is published as an error over the last-known rows, exactly
        // like a scan that could not list the process filesystem at all.
        let (confirmed, rejected) = match self.ids.project(snapshot, &self.columns) {
            Ok(rows) => (rows, None),
            Err(error) => (Vec::new(), Some(error.to_string())),
        };
        // A scan says which absences it cannot account for, and only those rows
        // survive not being confirmed. A rejected snapshot confirmed nothing at
        // all, so no absence in it is evidence of anything.
        let retention = if rejected.is_some() {
            Retention::Unenumerable
        } else {
            snapshot.retention()
        };
        let confirmed_ids: HashSet<ItemId> = confirmed.iter().map(|row| row.item_id).collect();
        let confirmed_empty = confirmed.is_empty();
        // A thread row this scan did not confirm is kept only beside the
        // process instance it belongs to, confirmed by this scan under the
        // same whole key, and only when the scan says it could not read that
        // process's threads (PX-019): never a process row, and never a thread
        // of an earlier instance under a reused PID.
        let thread_retention = if rejected.is_some() {
            crate::source::ThreadRetention::None
        } else {
            snapshot.thread_retention()
        };
        let live_processes: HashSet<crate::source::ProcessKey> = confirmed
            .iter()
            .filter(|row| !row.key.is_thread())
            .map(|row| row.key.clone())
            .collect();
        let mut unfiltered_target =
            retain_unconfirmed_by(&self.unfiltered_rows, confirmed, |row| {
                retention.keeps(&row.key)
                    || (row.key.is_thread()
                        && thread_retention.covers(&row.key.pid)
                        && live_processes.contains(&row.key.process_key()))
            });
        // A scan that read no threads (PX-019) says nothing about any thread:
        // they were hidden when it was taken, or its source reads none. Its
        // earlier thread rows leave the collection rather than linger as rows
        // kept from an earlier scan; a scan that did read threads keeps only
        // those of the processes whose threads it could not read.
        if rejected.is_none() && snapshot.threads.read().is_none() {
            unfiltered_target.retain(|row| !row.key.is_thread());
        }
        // Once a sort is active the target is published in it, confirmed and
        // retained rows alike: a retained row sorts under the values it was
        // published with, not last as though its values were missing. The order
        // is the view's at publication time, never the one in force when the
        // snapshot was sampled (PX-009).
        if let Some(order) = self.sort {
            unfiltered_target.sort_by(|left, right| sort::compare(order, left, right));
        }
        // Each process's threads follow it, in that order among themselves
        // (PX-019); a thread whose process row is gone goes with it.
        let (grouped, orphaned) = threads::group(unfiltered_target);
        let mut unfiltered_target = grouped;
        // Retention is bounded by construction, whatever a scan can and cannot
        // enumerate: the rows carried forward are evicted down to what the
        // published-row ceiling leaves once this scan's own rows are counted.
        let evicted_ids =
            evict_unconfirmed_beyond(&mut unfiltered_target, &confirmed_ids, self.published_limit);
        // A guard, not a case that occurs: every thread row follows its
        // process, a thread of an unconfirmed process is itself unconfirmed,
        // and eviction works from the end, so it takes a process's threads
        // before the process and this regrouping drops nothing. Were it ever
        // to drop a thread, that thread would be evicted, never ended
        // (PX-019).
        let mut evicted_ids = evicted_ids;
        let (unfiltered_target, evicted_threads) = threads::group_dropping(unfiltered_target);
        evicted_ids.extend(evicted_threads);
        let evicted = evicted_ids.len() + orphaned;
        // The inspector learns what this scan established about its target
        // before anything is planned (PX-013 review round): its latest row, or
        // that retention dropped it unconfirmed, which is not an exit, or that
        // a scan entitled to say so found it gone. A thread's absence is
        // left unaccounted for, beyond the record retention, only while its
        // own process instance is confirmed by this scan and the scan read no
        // threads or could not read that process's threads (PX-019): the same
        // whole-key rule the row retention above keeps thread rows by.
        let threads_read = snapshot.threads.read().is_some();
        self.inspector
            .observe_scan(&unfiltered_target, &evicted_ids, |key| {
                retention.keeps(key)
                    || (key.is_thread()
                        && live_processes.contains(&key.process_key())
                        && (!threads_read || thread_retention.covers(&key.pid)))
            });
        // The rows this scan did not confirm, which a PID jump never selects
        // (PX-011) and the tree never takes a parent from (PX-018): bounded by
        // the collection, and replaced on every refresh.
        let unconfirmed: HashSet<ItemId> = unfiltered_target
            .iter()
            .map(|row| row.item_id)
            .filter(|id| !confirmed_ids.contains(id))
            .collect();
        // What this scan could not see, which explains an absent parent in the
        // tree (PX-018). A rejected snapshot saw nothing it can be trusted on.
        let gaps = if rejected.is_some() {
            ScanGaps::default()
        } else {
            ScanGaps::of(snapshot)
        };
        // Every filter is applied to the collection together (PX-012): the
        // user filter, the PID set and the text filter. The tree is arranged
        // from this generation (PX-018).
        if rejected.is_none() {
            // What this sample said about threads (PX-019).
            self.thread_report = ThreadReport::of(&snapshot.threads);
        }
        let visible = self.visible_in(self.mode, &unfiltered_target, &unconfirmed, &gaps);
        let forest = visible.forest;
        let total = visible.total;
        let (target, matched, filter_truncated) =
            (visible.rows, visible.matched, visible.truncated);
        let filter_matches = visible.restricted.then_some(visible.matched_processes);
        let owner_view = visible.owner;
        let beyond_text = visible.beyond_text;
        // The summary shows the last successful sample (PX-007): this scan, if
        // its process list was read and accepted, and otherwise the one before
        // it, marked with this scan's collector error.
        let summary = self.summarize(snapshot, rejected.is_some(), filter_matches);
        // A scan that confirmed nothing and is entitled to delete nothing has no
        // row work to do at all: the target *is* the published rows. It plans
        // only its status and summary, so a failure whose text is the longest
        // this app emits cannot cost a row — there is no row budget, no diff and
        // no eviction on this path to cost one.
        if confirmed_empty && retention.is_global() {
            return self.publish_status_only(session, source_status, snapshot, rejected, summary);
        }
        // A client receives the status and the rows in the same catch-up
        // snapshot, so the status is charged against the frame before the rows
        // are — at its fixed reserve, never at this tick's length, so no clause a
        // degraded scan adds can shrink the row budget and delete a row the scan
        // never said had ended.
        let truncated = filter_truncated;
        let retained = target
            .iter()
            .filter(|row| !confirmed_ids.contains(&row.item_id))
            .count();
        let counts = RefreshCounts {
            retained,
            rejected,
            evicted,
            truncated,
        };
        let status = refresh_status(source_status, snapshot, &counts);
        debug_assert!(status.len() <= STATUS_RESERVE_BYTES);
        self.remember_status(source_status, snapshot, counts);
        let filter_status = self.filter_status_for(target.len(), matched, total, beyond_text);

        let (batch, max_ops, max_bytes) = self.planning_limits(session);

        let mut plan = Vec::new();
        if status != self.status {
            // First, so a scan that failed explains itself even if a later batch
            // of the same refresh does not commit.
            plan.push(Planned::new(Effect::Status(status)));
        }
        plan.extend(self.sampling_effects());
        if filter_status != self.filter_status {
            plan.push(Planned::new(Effect::FilterStatus(filter_status)));
        }
        plan.extend(self.owner_effects(owner_view));
        plan.extend(self.threads_effects(&unfiltered_target));
        // Then the summary, whose lines are scalar properties: only the ones a
        // client would see change are planned (PX-007).
        plan.extend(summary_effects(&self.summary, &summary));
        // Then the rows that changed place, each moved under its own item ID
        // (PX-009), and only then what really appeared, ended or changed; in
        // the tree, one index-range unit (PX-018).
        plan.extend(self.plan_rows(&target, batch, max_ops, max_bytes));
        // The rows reach the client once every entry before this one has.
        let rows_end = plan.len();
        plan.extend(self.view_effects(self.mode, forest.as_ref()));
        // Last, the search re-counted against the rows this plan publishes, so
        // it commits with the final rows rather than ahead of them (PX-011),
        // and the inspector, whose metrics and state follow them (PX-013).
        plan.extend(self.search_effects(&target, &unfiltered_target, &unconfirmed));
        // A row this sample moved under a parent the client keeps collapsed is
        // no longer selected there (PX-018).
        let held = self.reconcile_disclosure(&target, false);
        plan.extend(self.inspector_effects(&target, &unfiltered_target, &unconfirmed));
        // Follow last of all (PX-017): it stops when this plan hides or ends
        // the followed process, and a hint it sets names the row's final
        // place, after every row operation of this plan.
        self.follow
            .track(&target, &unfiltered_target, self.threads.user);
        let follow = self.follow_after(self.mode, &target, false, &plan);
        plan.extend(follow);

        let mut result = Refreshed {
            retained,
            evicted,
            truncated,
            ..Refreshed::default()
        };
        count(&plan, &mut result);
        if plan.is_empty() {
            // Nothing a client can see changed: no transaction is opened, so an
            // unchanging host produces no traffic at all. The rows stand, so the
            // identities worth keeping are still exactly theirs. Their order
            // values may still have moved without a cell changing — bytes
            // inside one displayed unit — and are brought up to this sample.
            refresh_sort_values(&mut self.rows, &target);
            refresh_sort_values(&mut self.unfiltered_rows, &unfiltered_target);
            self.unfiltered_rows = unfiltered_target;
            self.ids.retain(&self.unfiltered_rows);
            self.unconfirmed = unconfirmed;
            self.gaps = gaps;
            self.search.observe(&self.rows);
            self.inspector.reread(&self.rows, &confirmed_ids);
            return Ok(result);
        }

        let (transactions, reached, outcome) =
            commit_reaching(session, &plan, max_ops, max_bytes, &mut self.published());
        result.transactions = transactions;
        self.settle_disclosure(held, reached >= rows_end);
        // Order values never reach the wire, so every published row takes this
        // sample's whatever committed: a later click sorts by current values,
        // never by the ones a row was first published with.
        refresh_sort_values(&mut self.rows, &target);
        refresh_sort_values(&mut self.unfiltered_rows, &unfiltered_target);
        self.unfiltered_rows = unfiltered_target;
        self.ids.retain(&self.unfiltered_rows);
        self.unconfirmed = unconfirmed;
        self.gaps = gaps;
        self.prune_collapsed();
        self.search.observe(&self.rows);
        // The selected process's state is read again for this sample, if the
        // scan confirmed its row (PX-013).
        self.inspector.reread(&self.rows, &confirmed_ids);
        outcome.map(|()| result)
    }

    /// The order this view publishes in (PX-009), or `None` while the
    /// collection is still in its source's order.
    pub fn sort(&self) -> Option<SortOrder> {
        self.sort
    }

    /// Applies one click on `field`'s sort button (§7.6, PX-009): the column's
    /// catalogued default direction when it is not the active column, the
    /// reverse of the active direction when it is.
    ///
    /// A click is resolved against the order this view holds when the click is
    /// applied, and clicks are applied one at a time in the order they arrive,
    /// so the last click always decides the order the client ends up with.
    pub fn click_sort(
        &mut self,
        session: &Session,
        field: ProcessField,
    ) -> Result<Refreshed, SessionError> {
        self.set_sort(session, SortOrder::resolve(self.sort, field))
    }

    /// Makes `order` the collection's active sort and republishes the rows in
    /// it (PX-009).
    ///
    /// Every row keeps its item identity: a row that changes place is moved —
    /// deleted and reinserted under its own item ID inside one transaction
    /// ([`plan_moves`]) — never replaced by a new row, so a client's native
    /// selection, which is anchored to item IDs, stays on the same process
    /// (§6.2, §8). Nothing is sampled: the rows are the server's collection,
    /// filtered again. Normally the published set does not change, only its
    /// order; when the filters' result has changed since the last publication
    /// (an account name resolved, or the published-row ceiling keeps a
    /// different leading window in the new order), the rows that joined or
    /// left are inserted or deleted in the same plan, with the filter and
    /// search statuses that describe them (PX-012 review round, F1). A request
    /// for the order already published commits nothing at all.
    ///
    /// The buttons' labels are planned ahead of the rows, like the status on a
    /// refresh, and the view's record of the active order advances when the
    /// label naming it commits. Should a later transaction of the same request
    /// fail, the view still holds what the store holds — the new order, with
    /// the rows partly moved — and the next refresh completes the move, since it
    /// publishes in that order.
    pub fn set_sort(
        &mut self,
        session: &Session,
        order: SortOrder,
    ) -> Result<Refreshed, SessionError> {
        if self.sort == Some(order) {
            // A request for the order already published changes nothing a
            // client can see, so it opens no transaction, exactly as an
            // unchanged snapshot does.
            self.ids.retain(&self.unfiltered_rows);
            return Ok(Refreshed::default());
        }
        self.unfiltered_rows
            .sort_by(|left, right| sort::compare(order, left, right));
        // Each process's threads follow it, sorted among themselves (PX-019).
        // Every thread row's process row is here, so none is dropped.
        let (grouped, _) = threads::group(std::mem::take(&mut self.unfiltered_rows));
        self.unfiltered_rows = grouped;
        let mut plan = Vec::new();
        if let Some(previous) = self.sort.filter(|previous| previous.field != order.field) {
            plan.push(Planned::new(Effect::SortLabel {
                node: sort_button_of(previous.field),
                label: sort::sort_button_label(previous.field, None),
                activates: None,
            }));
        }
        plan.push(Planned::new(Effect::SortLabel {
            node: sort_button_of(order.field),
            label: sort::sort_button_label(order.field, Some(order.direction)),
            activates: Some(order),
        }));
        // A sort re-applies every filter and publishes through the full path
        // (PX-012 review round, F1): moves for the rows that stay, insertions
        // and deletions for any whose membership changed since the last
        // publication — an account name resolved in between, or a different
        // leading window under the published-row ceiling — and the filters'
        // statuses, the search and the inspector (PX-013) re-planned against
        // exactly those rows, in the same plan. Every row keeps its item ID,
        // and the inspector follows its own wherever it moved, or reports it
        // as not shown when the new order's row ceiling drops it.
        self.republish(session, plan)
    }

    /// The visible columns, in published order (PX-068).
    pub fn columns(&self) -> Vec<ProcessField> {
        self.columns.fields().to_vec()
    }

    /// The published column layout (PX-068).
    pub fn column_layout(&self) -> &ColumnLayout {
        &self.columns
    }

    /// Applies one click on a column chooser button (§7.6, PX-068), resolved
    /// against the layout this view holds when the click is applied.
    pub fn click_column(
        &mut self,
        session: &Session,
        action: ColumnAction,
    ) -> Result<Refreshed, SessionError> {
        let layout = action.apply(&self.columns);
        self.publish_layout(session, layout)
    }

    /// Shows exactly `fields`, in that order (PX-068). A request that is not a
    /// layout — no column, or one column twice — is refused and commits
    /// nothing.
    pub fn set_columns(
        &mut self,
        session: &Session,
        fields: &[ProcessField],
    ) -> Result<Refreshed, ColumnsError> {
        let layout = ColumnLayout::new(fields).map_err(ColumnsError::Layout)?;
        self.publish_layout(session, layout)
            .map_err(ColumnsError::Session)
    }

    /// Shows the catalogued default columns again (PX-068).
    pub fn restore_default_columns(
        &mut self,
        session: &Session,
    ) -> Result<Refreshed, SessionError> {
        self.publish_layout(session, ColumnLayout::default())
    }

    /// Makes `layout` the published column layout (PX-068).
    ///
    /// A layout is presentation, not identity: the published rows, their item
    /// IDs, their order and the active sort are all kept. The table's headings
    /// are set, the chooser buttons whose label or enablement changes are
    /// updated, and every published row whose cells change is updated in place
    /// — `MODEL_UPDATE`s only, never a deletion, so no client revision lacks a
    /// row it had selected, and the table and its model are never recreated.
    /// Rows keep every catalogued cell and typed order value, so a hidden
    /// sorted column still orders the rows exactly, and showing it again needs
    /// no new sample. When the whole change fits one transaction it is
    /// committed as one, so headings and cells never disagree on the wire.
    fn publish_layout(
        &mut self,
        session: &Session,
        layout: ColumnLayout,
    ) -> Result<Refreshed, SessionError> {
        if layout == self.columns {
            self.ids.retain(&self.unfiltered_rows);
            return Ok(Refreshed::default());
        }
        let target: Vec<Row> = self.rows.iter().map(|row| row.laid_out(&layout)).collect();
        let (batch, max_ops, max_bytes) = self.planning_limits(session);
        // Both collections' headings follow the one layout, whichever is
        // shown, so switching views never re-lays out a column (PX-018).
        let mut plan = vec![
            Planned::new(Effect::Columns(layout.clone())),
            Planned::new(Effect::TreeColumns(layout.clone())),
        ];
        for (node, action) in COLUMN_CHOOSER_BUTTONS {
            let label = action.label(&layout);
            if label != action.label(&self.columns) {
                plan.push(Planned::new(Effect::ChooserLabel { node, label }));
            }
            let enabled = action.enabled(&layout);
            if enabled != action.enabled(&self.columns) {
                plan.push(Planned::new(Effect::ChooserEnabled { node, enabled }));
            }
        }
        plan.extend(self.plan_rows(&target, batch, max_ops, max_bytes));
        let bytes: usize = plan.iter().map(|planned| planned.bytes).sum();
        if plan.len() <= max_ops && bytes <= max_bytes {
            let last = plan.len() - 1;
            for planned in &mut plan[..last] {
                planned.joins_next = true;
            }
        }
        let mut result = Refreshed::default();
        count(&plan, &mut result);
        // In the table; the tree may replace its preorder whole when its
        // in-place updates exceed a bound (PX-018).
        debug_assert!(
            self.mode == ViewMode::Tree
                || (result.inserted, result.deleted, result.moved) == (0, 0, 0),
            "a layout change only updates rows in place"
        );
        let (transactions, outcome) =
            commit(session, &plan, max_ops, max_bytes, &mut self.published());
        result.transactions = transactions;
        // The server-side rows follow whatever layout committed, so a later
        // sort, filter or refresh publishes from cells in that layout.
        let committed = self.columns.clone();
        for row in &mut self.unfiltered_rows {
            *row = row.laid_out(&committed);
        }
        self.ids.retain(&self.unfiltered_rows);
        outcome.map(|()| result)
    }

    /// The item identities of the published rows, in published order.
    pub fn item_ids(&self) -> Vec<ItemId> {
        self.rows.iter().map(|row| row.item_id).collect()
    }

    /// Re-evaluates the summary using the last sample and updated filter match count.
    fn summarize_with_matches(&self, filter_matches: Option<usize>) -> summary::Properties {
        match &self.sample {
            Some(sample) => summary::properties(&summary::summarize(&summary::State::Collecting {
                last: Some(sample),
                error: None,
                source: &sample.source,
                filter_matches,
            })),
            None => summary::properties(&summary::summarize(&summary::State::NotStarted)),
        }
    }

    /// The summary this refresh publishes (PX-007): this scan's sample if it is
    /// one, and otherwise the last successful sample marked with this scan's
    /// collector error. A scan this app rejected is not a sample, whatever it
    /// read.
    fn summarize(
        &mut self,
        snapshot: &ProcessSnapshot,
        rejected: bool,
        filter_matches: Option<usize>,
    ) -> summary::Properties {
        let error = if rejected {
            Some(CollectorError::Rejected)
        } else {
            match Sample::of(snapshot) {
                Ok(sample) => {
                    self.sample = Some(sample);
                    None
                }
                Err(error) => Some(error),
            }
        };
        summary::properties(&summary::summarize(&summary::State::Collecting {
            last: self.sample.as_ref(),
            error,
            source: &snapshot.source,
            filter_matches,
        }))
    }

    /// Publishes an error over the last-known rows and mutates no row.
    ///
    /// This is the whole of a refresh whose scan confirmed nothing and accounted
    /// for no absence — a rejected snapshot, or a process list that could not be
    /// listed. Such a tick is evidence about the scan, never about a process, so
    /// there is nothing to diff: the published rows are already inside every
    /// bound they were published under, and re-choosing them here is exactly how
    /// a longer status could evict one.
    fn publish_status_only(
        &mut self,
        session: &Session,
        source_status: &str,
        snapshot: &ProcessSnapshot,
        rejected: Option<String>,
        summary: summary::Properties,
    ) -> Result<Refreshed, SessionError> {
        let retained = self.rows.len();
        let counts = RefreshCounts {
            retained,
            rejected,
            ..RefreshCounts::default()
        };
        let status = refresh_status(source_status, snapshot, &counts);
        debug_assert!(status.len() <= STATUS_RESERVE_BYTES);
        self.remember_status(source_status, snapshot, counts);
        let mut result = Refreshed {
            retained,
            ..Refreshed::default()
        };
        let mut plan = Vec::new();
        if status != self.status {
            result.status_changed = true;
            plan.push(Planned::new(Effect::Status(status)));
        }
        plan.extend(summary_effects(&self.summary, &summary));
        result.summary = plan.len() - usize::from(result.status_changed);
        let sampling = self.sampling_effects();
        result.controls += sampling.len();
        plan.extend(sampling);
        // This scan confirmed no row, so none is the currently observed
        // instance a PID jump may select (PX-011). The rows themselves stand.
        self.unconfirmed = self.unfiltered_rows.iter().map(|row| row.item_id).collect();
        let search = self.search_effects(&self.rows, &self.unfiltered_rows, &self.unconfirmed);
        result.controls += search.len();
        plan.extend(search);
        let inspector =
            self.inspector_effects(&self.rows, &self.unfiltered_rows, &self.unconfirmed);
        result.controls += inspector.len();
        plan.extend(inspector);
        if plan.is_empty() {
            // An identical failure republishes nothing at all.
            self.ids.retain(&self.unfiltered_rows);
            return Ok(result);
        }
        let max_ops = session.with_store(|store| store.limits().max_transaction_operations.max(1));
        let (transactions, outcome) = commit(
            session,
            &plan,
            max_ops,
            MAX_TRANSACTION_PAYLOAD_BYTES,
            &mut self.published(),
        );
        result.transactions = transactions;
        self.ids.retain(&self.unfiltered_rows);
        outcome.map(|()| result)
    }

    /// The status text this view last published.
    pub fn status(&self) -> &str {
        &self.status
    }

    /// Number of published rows.
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Stages a filter query change with a new monotonic generation (PX-010).
    ///
    /// Updates the filter status line to show that the new generation is pending,
    /// while the table continues to show the currently committed results.
    pub fn stage_filter(
        &mut self,
        session: &Session,
        query: &str,
    ) -> Result<FilterGeneration, SessionError> {
        let next_gen = self.filter_generation.next();
        self.filter_generation = next_gen;
        let filter_query = FilterQuery::parse(query);
        let pending = PendingFilter {
            generation: next_gen,
            query: filter_query,
        };
        let pending_status = pending.query.format_pending_status(
            next_gen,
            self.committed_generation,
            self.rows.len(),
            self.admitted_rows(),
        );
        self.pending_filter = Some(pending);

        if pending_status != self.filter_status {
            let plan = vec![Planned::new(Effect::FilterStatus(pending_status))];
            let max_ops =
                session.with_store(|store| store.limits().max_transaction_operations.max(1));
            let (_, outcome) = commit(
                session,
                &plan,
                max_ops,
                MAX_TRANSACTION_PAYLOAD_BYTES,
                &mut self.published(),
            );
            outcome?;
        }
        Ok(next_gen)
    }

    /// Commits a staged query generation (PX-010).
    ///
    /// Rejects stale or out-of-order generations so older query results cannot
    /// replace the newest result set. When applied, filters visible rows from
    /// `unfiltered_rows` preserving item identity and selection, updates the
    /// system summary with matching counts, and publishes the committed filter status.
    pub fn commit_filter(
        &mut self,
        session: &Session,
        generation: FilterGeneration,
    ) -> Result<Refreshed, SessionError> {
        if generation < self.committed_generation {
            return Ok(Refreshed::default());
        }

        let query = match &self.pending_filter {
            Some(pending) if pending.generation == generation => {
                let q = pending.query.clone();
                self.pending_filter = None;
                q
            }
            _ => {
                if generation == self.committed_generation {
                    return Ok(Refreshed::default());
                }
                return Ok(Refreshed::default());
            }
        };

        self.committed_generation = generation;
        self.committed_filter = query;

        let mut plan = Vec::new();
        if input_value(session, FILTER_INPUT) != self.committed_filter.raw() {
            plan.push(Planned::new(Effect::FilterValue(
                self.committed_filter.raw().to_string(),
            )));
        }
        self.republish(session, plan)
    }

    /// Keeps what a sample's status line was built from (PX-018), so a
    /// republish can restate it with its own row counts.
    fn remember_status(
        &mut self,
        source_status: &str,
        snapshot: &ProcessSnapshot,
        counts: RefreshCounts,
    ) {
        self.status_source = source_status.to_string();
        self.status_scan = ScanReport::of(snapshot);
        self.status_counts = counts;
    }

    /// The status line for a republish that shows `target`, of which the
    /// ceiling left `truncated` matching rows out (PX-018): the last sample's
    /// scan and counts, with the rows kept from an earlier scan and the rows
    /// beyond the publishable size limit counted again for these rows. A
    /// republish samples nothing, so its scan clauses are the last sample's.
    fn republished_status(&self, target: &[Row], truncated: usize) -> (String, RefreshCounts) {
        let counts = RefreshCounts {
            retained: target
                .iter()
                .filter(|row| self.unconfirmed.contains(&row.item_id))
                .count(),
            truncated,
            ..self.status_counts.clone()
        };
        let status = refresh_status_from(&self.status_source, &self.status_scan, &counts);
        debug_assert!(status.len() <= STATUS_RESERVE_BYTES);
        (status, counts)
    }

    /// The rows every active filter keeps out of `unfiltered`, and the
    /// user-filter controls for them (PX-010, PX-012), as `mode` shows
    /// them (PX-018): the table's leading rows in the active order, or the
    /// tree's leading preorder.
    ///
    /// The row ceiling applies to a forest exactly as to a table: the rows
    /// kept are the leading ones in the order the collection is published in
    /// — here the forest's preorder, each level in the active order — up to
    /// the same frame budget and row limit, each tree row charged its parent
    /// cell and item properties too. A prefix of a preorder is itself a
    /// forest, and every kept row's ancestors precede it, so the ceiling
    /// never orphans a shown process: a subtree it cuts is shown with the
    /// descendants that fit. `truncated` counts the rest, which the status
    /// line states in the same commit on every path that publishes these rows
    /// — a sample, a view switch either way, a sort, a filter — and the
    /// filter status says how many of the matching rows are shown.
    ///
    /// The thread controls (PX-019) first choose the rows the collection
    /// holds at all — thread rows only while user threads are shown, kernel
    /// threads only while they are — and the filters then choose processes
    /// among those: a thread row is kept exactly when its process row is.
    fn visible_in(
        &self,
        mode: ViewMode,
        unfiltered: &[Row],
        unconfirmed: &HashSet<ItemId>,
        gaps: &ScanGaps,
    ) -> Visible {
        let settings = self.threads;
        let admitted: std::borrow::Cow<'_, [Row]> =
            if unfiltered.iter().all(|row| threads::admits(settings, row)) {
                std::borrow::Cow::Borrowed(unfiltered)
            } else {
                std::borrow::Cow::Owned(
                    unfiltered
                        .iter()
                        .filter(|row| threads::admits(settings, row))
                        .cloned()
                        .collect(),
                )
            };
        let scope = self.owner.scope(&admitted, &self.committed_filter);
        let kept = threads::kept_with_processes(&admitted, |row| scope.keeps(row));
        let matched = kept.len();
        let matched_processes = kept.iter().filter(|row| !row.key.is_thread()).count();
        let (rows, truncated, forest) = match mode {
            ViewMode::Table => {
                let mut rows = kept;
                let truncated = bound_to_publishable_collection(&mut rows, self.published_limit);
                (rows, truncated, None)
            }
            ViewMode::Tree => {
                let generation = Generation {
                    unfiltered,
                    unconfirmed,
                    gaps,
                };
                let mut rows = tree::arrange(kept, &generation, &self.columns, self.tree_levels);
                let truncated = bound_to_publishable_collection(&mut rows, self.published_limit);
                // The arrangement never exceeds the depth limit, so the SDK
                // refuses nothing here; a refusal would be this app's defect,
                // and it publishes the rows flat rather than a refused shape.
                if let Err(error) = tree::stamp(&mut rows, self.tree_levels) {
                    debug_assert!(false, "the arranged forest was refused: {error}");
                    eprintln!("srtop: process tree refused: {error}");
                    for row in &mut rows {
                        *row = row.flat(&self.columns);
                    }
                }
                let forest = ForestSummary::of(&rows);
                (rows, truncated, Some(forest))
            }
        };
        let shown_processes = rows.iter().filter(|row| !row.key.is_thread()).count();
        let owner = self
            .owner
            .view(&scope, &admitted, shown_processes, matched_processes);
        Visible {
            restricted: scope.is_restricted(),
            beyond_text: scope.restricts_beyond_text(),
            total: admitted.len(),
            rows,
            matched,
            matched_processes,
            truncated,
            owner,
            forest,
        }
    }

    /// The text filter's status when `shown` of the `matched` rows every filter
    /// keeps are published out of `total`. While the user filter or the PID
    /// set is active, the text filter's own counts say they are counted after
    /// them (PX-012).
    fn filter_status_for(
        &self,
        shown: usize,
        matched: usize,
        total: usize,
        beyond_text: bool,
    ) -> String {
        if !beyond_text {
            return self.committed_filter.format_committed_status(
                self.committed_generation,
                shown,
                matched,
                total,
            );
        }
        if self.committed_filter.is_empty() {
            let head = if self.committed_generation.get() == 0 {
                "Filter: none".to_string()
            } else {
                format!("Committed filter (gen {}): none", self.committed_generation)
            };
            format!("{head} · {shown} of {total} rows shown by the user filter and PID set")
        } else {
            format!(
                "{} · counted after the user filter and PID set",
                self.committed_filter.format_committed_status(
                    self.committed_generation,
                    shown,
                    matched,
                    total,
                )
            )
        }
    }

    /// The user-filter controls' changes when they say `target` (PX-012).
    fn owner_effects(&self, target: OwnerView) -> Vec<Planned> {
        let published = &self.owner_published;
        let mut plan = Vec::new();
        if target.status != published.status {
            plan.push(Planned::new(Effect::OwnerStatus(target.status)));
        }
        for (node, was, now) in [
            (OWNER_MINE, published.mine, target.mine),
            (OWNER_ALL, published.all, target.all),
        ] {
            if was != now {
                plan.push(Planned::new(Effect::OwnerEnabled { node, enabled: now }));
            }
        }
        plan
    }

    /// Re-applies every filter to the server-side collection and publishes the
    /// result after `plan` (PX-010, PX-012): the filters' status lines, the
    /// summary's match count, the rows that leave, return or move — each under
    /// its own item ID, never a new one — and the search re-counted against
    /// them, all in one plan. A row that leaves takes the table's promoted
    /// selection with it, which the SDK's settlement clears in the same commit
    /// (PX-011-G01).
    fn republish(
        &mut self,
        session: &Session,
        plan: Vec<Planned>,
    ) -> Result<Refreshed, SessionError> {
        self.republish_in(session, plan, None)
    }

    /// [`Self::republish`], into the view `switch` names when it names one
    /// (PX-018): the shown collection's model is emptied, the other's filled
    /// with the same rows under the same item IDs, and the two swap
    /// visibility, all in one transaction, so no client revision shows both
    /// collections or neither.
    fn republish_in(
        &mut self,
        session: &Session,
        mut plan: Vec<Planned>,
        switch: Option<ViewMode>,
    ) -> Result<Refreshed, SessionError> {
        let mode = switch.unwrap_or(self.mode);
        let visible = self.visible_in(mode, &self.unfiltered_rows, &self.unconfirmed, &self.gaps);
        let forest = visible.forest;
        let target = visible.rows;
        let filter_matches = visible.restricted.then_some(visible.matched_processes);
        // A paused view re-projects its frozen rows, and its freshness line
        // keeps saying it is paused (PX-015).
        let summary = self.labelled(self.summarize_with_matches(filter_matches));
        let filter_status = self.filter_status_for(
            target.len(),
            visible.matched,
            visible.total,
            visible.beyond_text,
        );
        let (batch, max_ops, max_bytes) = self.planning_limits(session);
        // The rows this publishes may not be the ones the last sample's
        // status counted: a switch to the tree, whose rows cost more bytes,
        // or a sort or filter that brings other rows under the ceiling
        // (PX-018). The status counts these rows, in this commit.
        let (status, counts) = self.republished_status(&target, visible.truncated);
        if status != self.status {
            plan.push(Planned::new(Effect::Status(status)));
        }
        self.status_counts = counts;
        if filter_status != self.filter_status {
            plan.push(Planned::new(Effect::FilterStatus(filter_status)));
        }
        plan.extend(self.owner_effects(visible.owner));
        plan.extend(self.threads_effects(&self.unfiltered_rows));
        plan.extend(summary_effects(&self.summary, &summary));

        let switched = match switch {
            Some(to) => {
                plan.extend(self.plan_switch(to, &target, batch, max_bytes));
                true
            }
            None => {
                plan.extend(self.plan_rows(&target, batch, max_ops, max_bytes));
                false
            }
        };
        // The rows reach the client once every entry before this one has.
        let rows_end = plan.len();
        plan.extend(self.view_effects(mode, forest.as_ref()));
        // The search counts the newly filtered rows; a target a filter hid is
        // reported as no longer shown, never replaced (PX-011). So is the
        // inspector's (PX-013), and a row these rows put under a parent the
        // client keeps collapsed is no longer selected there (PX-018).
        plan.extend(self.search_effects(&target, &self.unfiltered_rows, &self.unconfirmed));
        let held = self.reconcile_disclosure(&target, switched);
        plan.extend(self.inspector_effects(&target, &self.unfiltered_rows, &self.unconfirmed));
        if switched {
            // The selection survives the switch by item ID (PX-018): the
            // other view never held it, so the server promotes the process
            // the inspector describes — the user's own last selection — in the
            // same commit that shows the view. While follow is on, only the
            // followed process may be promoted (PX-017).
            let selected = self
                .inspector
                .selected_row()
                .and_then(|selected| target.iter().find(|row| row.item_id == selected.item_id))
                .filter(|row| {
                    self.follow
                        .following()
                        .is_none_or(|followed| followed == row.item_id)
                })
                .cloned();
            if let Some(row) = selected {
                // A promotion reveals: in a freshly filled tree nothing is
                // collapsed, so the mirror has nothing to expand (PX-018).
                self.reveal_in_mirror(&target, row.item_id);
                plan.push(Planned::new(Effect::Promote(mode.node(), Box::new(row))));
            }
        }
        // Follow last (PX-017): a filter that hides the followed process
        // stops follow; a sort or a filter that moves it needs nothing, as the
        // client reveals the hinted item wherever it moves. A switch carries
        // it into the shown view: the new collection holds no hint yet
        // (PX-018).
        self.follow
            .track(&target, &self.unfiltered_rows, self.threads.user);
        let follow = self.follow_after(mode, &target, switched, &plan);
        plan.extend(follow);
        if switched {
            // The whole switch is one transaction when it fits one, so the
            // shown view, its selection and its hint arrive together.
            let ops: usize = plan.iter().map(Planned::operations).sum();
            let bytes: usize = plan.iter().map(|planned| planned.bytes).sum();
            if ops <= max_ops && bytes <= max_bytes {
                let last = plan.len() - 1;
                for planned in &mut plan[..last] {
                    planned.joins_next = true;
                }
            }
        }

        let mut result = Refreshed::default();
        count(&plan, &mut result);

        if plan.is_empty() {
            refresh_sort_values(&mut self.rows, &target);
            self.ids.retain(&self.unfiltered_rows);
            self.search.observe(&self.rows);
            return Ok(result);
        }

        let (transactions, reached, outcome) =
            commit_reaching(session, &plan, max_ops, max_bytes, &mut self.published());
        result.transactions = transactions;
        self.settle_disclosure(held, reached >= rows_end);
        refresh_sort_values(&mut self.rows, &target);
        self.ids.retain(&self.unfiltered_rows);
        self.prune_collapsed();
        self.search.observe(&self.rows);
        outcome.map(|()| result)
    }

    /// Makes `choice` the user filter (PX-012), as a button does: the user
    /// field is emptied, and every filter is applied again. The rows keep
    /// their item IDs whichever are shown.
    pub fn choose_owner(
        &mut self,
        session: &Session,
        choice: OwnerChoice,
    ) -> Result<Refreshed, SessionError> {
        self.owner.choose(choice);
        let mut plan = Vec::new();
        if !input_value(session, OWNER_INPUT).is_empty() {
            plan.push(Planned::new(Effect::OwnerValue {
                node: OWNER_INPUT,
                value: String::new(),
            }));
        }
        self.republish(session, plan)
    }

    /// The user field's new text (PX-012): empty is every visible user, digits
    /// a UID, anything else an account name.
    pub fn set_owner_input(
        &mut self,
        session: &Session,
        text: &str,
    ) -> Result<Refreshed, SessionError> {
        let kept = self.owner.set_input(text).to_string();
        let mut plan = Vec::new();
        if input_value(session, OWNER_INPUT) != kept {
            plan.push(Planned::new(Effect::OwnerValue {
                node: OWNER_INPUT,
                value: kept,
            }));
        }
        self.republish(session, plan)
    }

    /// The PID-set field's new text (PX-012).
    pub fn set_pid_set(
        &mut self,
        session: &Session,
        text: &str,
    ) -> Result<Refreshed, SessionError> {
        let kept = self.owner.set_pid_set(text).to_string();
        let mut plan = Vec::new();
        if input_value(session, PID_SET_INPUT) != kept {
            plan.push(Planned::new(Effect::OwnerValue {
                node: PID_SET_INPUT,
                value: kept,
            }));
        }
        self.republish(session, plan)
    }

    /// The user filter, PID set and account-name cache (PX-012).
    pub fn owner_filter(&self) -> &OwnerFilter {
        &self.owner
    }

    /// The user-filter status this view last published (PX-012).
    pub fn owner_status(&self) -> &str {
        &self.owner_published.status
    }

    /// Stages and commits a filter query (PX-010).
    pub fn set_filter(
        &mut self,
        session: &Session,
        query: &str,
    ) -> Result<Refreshed, SessionError> {
        let generation = self.stage_filter(session, query)?;
        self.commit_filter(session, generation)
    }

    /// Handles a client `TEXT_EDIT` event on the filter input (PX-010).
    pub fn handle_client_text_edit(
        &mut self,
        session: &Session,
        text: &str,
    ) -> Result<Refreshed, SessionError> {
        self.set_filter(session, text)
    }

    /// The active committed filter query.
    pub fn committed_filter(&self) -> &FilterQuery {
        &self.committed_filter
    }

    /// Monotonic generation of the active committed filter query.
    pub fn committed_generation(&self) -> FilterGeneration {
        self.committed_generation
    }

    /// Currently allocated filter generation.
    pub fn filter_generation(&self) -> FilterGeneration {
        self.filter_generation
    }

    /// Pending staged filter, if any.
    pub fn pending_filter(&self) -> Option<&PendingFilter> {
        self.pending_filter.as_ref()
    }

    /// Current filter status text.
    pub fn filter_status(&self) -> &str {
        &self.filter_status
    }

    /// Count of unfiltered rows retained server-side.
    pub fn unfiltered_row_count(&self) -> usize {
        self.unfiltered_rows.len()
    }

    /// Unfiltered rows retained server-side.
    #[allow(dead_code)]
    pub(crate) fn unfiltered_rows(&self) -> &[Row] {
        &self.unfiltered_rows
    }

    /// The server's search state (PX-011).
    pub fn search(&self) -> &Search {
        &self.search
    }

    /// The search status text this view last published (PX-011).
    pub fn search_status(&self) -> &str {
        &self.search_published.status
    }

    /// Replaces the search query (PX-011): parsed by the filter's rules under a
    /// new monotonic generation, and targeting the first match in published
    /// order. No row is hidden, moved or changed: only the search controls are
    /// published.
    pub fn set_search(
        &mut self,
        session: &Session,
        query: &str,
    ) -> Result<Refreshed, SessionError> {
        self.search_as(session, Holder::Every, query)
    }

    /// [`Self::set_search`], typed by `client` (PX-018): the match it
    /// promotes is that client's selection.
    pub fn set_search_from(
        &mut self,
        session: &Session,
        client: &[u8],
        query: &str,
    ) -> Result<Refreshed, SessionError> {
        self.search_as(session, Holder::Client(client.to_vec()), query)
    }

    fn search_as(
        &mut self,
        session: &Session,
        holder: Holder,
        query: &str,
    ) -> Result<Refreshed, SessionError> {
        let selected = self.search.set_query(query, &self.rows);
        let raw = self.search.query().raw().to_string();
        let mut plan = Vec::new();
        if input_value(session, SEARCH_INPUT) != raw {
            plan.push(Planned::new(Effect::SearchValue {
                node: SEARCH_INPUT,
                value: raw,
            }));
        }
        plan.extend(self.promotion(selected));
        self.publish_search(session, plan, holder)
    }

    /// Applies one Next or Previous click (PX-011).
    pub fn step_search(
        &mut self,
        session: &Session,
        step: SearchStep,
    ) -> Result<Refreshed, SessionError> {
        self.step_search_as(session, Holder::Every, step)
    }

    /// [`Self::step_search`], clicked by `client` (PX-018).
    pub fn step_search_from(
        &mut self,
        session: &Session,
        client: &[u8],
        step: SearchStep,
    ) -> Result<Refreshed, SessionError> {
        self.step_search_as(session, Holder::Client(client.to_vec()), step)
    }

    fn step_search_as(
        &mut self,
        session: &Session,
        holder: Holder,
        step: SearchStep,
    ) -> Result<Refreshed, SessionError> {
        let selected = self.search.step(step, &self.rows);
        let plan = self.promotion(selected).into_iter().collect();
        self.publish_search(session, plan, holder)
    }

    /// Keeps the PID field's text (PX-011). Typing never moves the target; it
    /// only enables Go to PID when the text is a PID.
    pub fn set_pid_input(
        &mut self,
        session: &Session,
        text: &str,
    ) -> Result<Refreshed, SessionError> {
        let kept = self.search.set_pid_input(text).to_string();
        let mut plan = Vec::new();
        if input_value(session, PID_INPUT) != kept {
            plan.push(Planned::new(Effect::SearchValue {
                node: PID_INPUT,
                value: kept,
            }));
        }
        self.publish_search(session, plan, Holder::Every)
    }

    /// Applies one Go to PID click (PX-011): an explicit new search that
    /// resolves the PID field to the published row the latest scan confirmed
    /// with that PID.
    pub fn go_to_pid(&mut self, session: &Session) -> Result<Refreshed, SessionError> {
        self.go_to_pid_as(session, Holder::Every)
    }

    /// [`Self::go_to_pid`], clicked by `client` (PX-018).
    pub fn go_to_pid_from(
        &mut self,
        session: &Session,
        client: &[u8],
    ) -> Result<Refreshed, SessionError> {
        self.go_to_pid_as(session, Holder::Client(client.to_vec()))
    }

    fn go_to_pid_as(
        &mut self,
        session: &Session,
        holder: Holder,
    ) -> Result<Refreshed, SessionError> {
        let selected = self.search.go_to_pid(&self.rows, &self.unconfirmed);
        let plan = self.promotion(selected).into_iter().collect();
        self.publish_search(session, plan, holder)
    }

    /// The promotion of the row an explicit search just `selected` (PX-011,
    /// PX-011-G01). Promoting the item already promoted is a new promotion and
    /// reveals it again. A search that selected nothing plans nothing, so the
    /// promotion — and the client's native selection — stay as they were.
    fn promotion(&self, selected: Option<ItemId>) -> Option<Planned> {
        let item = selected?;
        let row = self.rows.iter().find(|row| row.item_id == item)?;
        // On the shown collection (PX-018): in the tree, the client expands
        // exactly the collapsed parents above the match to reveal it, and
        // reports each with `EXPANSION_CHANGED` (PX-018-G01).
        Some(Planned::new(Effect::Promote(
            self.mode.node(),
            Box::new(row.clone()),
        )))
    }

    /// The item the table's promoted selection names, as the store holds it
    /// after the last commit (PX-011).
    pub fn promoted_selection(&self) -> Option<ItemId> {
        self.promoted
    }

    /// Types `text` into the PID field and clicks Go to PID (PX-011).
    pub fn jump_to_pid(
        &mut self,
        session: &Session,
        text: &str,
    ) -> Result<Refreshed, SessionError> {
        self.set_pid_input(session, text)?;
        self.go_to_pid(session)
    }

    /// The search controls' changes when the client holds `rows` (PX-011).
    fn search_effects(
        &self,
        rows: &[Row],
        unfiltered: &[Row],
        unconfirmed: &HashSet<ItemId>,
    ) -> Vec<Planned> {
        let target = self.search.view(rows, unfiltered, unconfirmed);
        let published = &self.search_published;
        let mut plan = Vec::new();
        if target.status != published.status {
            plan.push(Planned::new(Effect::SearchStatus(target.status)));
        }
        for (node, was, now) in [
            (SEARCH_PREVIOUS, published.previous, target.previous),
            (SEARCH_NEXT, published.next, target.next),
            (PID_GO, published.go, target.go),
        ] {
            if was != now {
                plan.push(Planned::new(Effect::SearchEnabled { node, enabled: now }));
            }
        }
        plan
    }

    /// Publishes `plan` and the search controls' changes over the rows already
    /// published (PX-011). The rows are not touched. A promotion in `plan` is
    /// `holder`'s selection (PX-018).
    fn publish_search(
        &mut self,
        session: &Session,
        mut plan: Vec<Planned>,
        holder: Holder,
    ) -> Result<Refreshed, SessionError> {
        // A promotion is a selection too: the inspector follows it, under a
        // new generation, exactly as it follows a click (PX-013).
        let promoted = plan.iter().find_map(|planned| match &planned.effect {
            Effect::Promote(_, row) => Some(row.as_ref().clone()),
            _ => None,
        });
        // The promotion reveals its match: in the tree every client expands
        // the collapsed parents above it before it reports them, so every
        // client's mirror expands them now (PX-018).
        let mut unrevealed = None;
        let unselected = (self.inspector.hold(), self.holder.clone());
        if let Some(row) = &promoted {
            self.inspector.select(row);
            self.holder = holder;
            // While follow is on, the match is the process followed next
            // (PX-017): the hint moves to it.
            self.follow.selected(Some(row));
            let held = self.disclosure.clone();
            let rows = std::mem::take(&mut self.rows);
            if self.reveal_in_mirror(&rows, row.item_id) {
                unrevealed = Some(held);
            }
            self.rows = rows;
        }
        plan.extend(self.search_effects(&self.rows, &self.unfiltered_rows, &self.unconfirmed));
        plan.extend(self.inspector_effects(&self.rows, &self.unfiltered_rows, &self.unconfirmed));
        let follow = self.follow_table_effects(&plan);
        plan.extend(follow);
        plan.extend(self.follow_view_effects());
        let promotion_at = plan
            .iter()
            .position(|planned| matches!(planned.effect, Effect::Promote(..)));
        // What every client held before the promotion: a report observed at
        // this revision or earlier was sent before the client applied it.
        let before = session.current_revision();
        let (published, reached) = self.publish_controls_reaching(session, plan);
        // A promotion that never committed revealed nothing on a client, even
        // when the same item was already promoted before: the mirror keeps
        // the parents the clients keep collapsed, and the inspector keeps the
        // selection the clients still show (verification V5-I1). Follow's
        // retarget is not put back.
        if promotion_at.is_some_and(|at| reached <= at) {
            if let Some(held) = unrevealed {
                self.disclosure = held;
            }
            let (inspector, holder) = unselected;
            self.inspector.restore(inspector);
            self.holder = holder;
        } else if let (Some(row), Some(_), ViewMode::Tree) = (&promoted, promotion_at, self.mode) {
            self.revealing = Some(Revealing {
                item: row.item_id,
                before,
            });
        }
        published
    }

    /// Commits `plan`, which changes controls only and no row, in bounded
    /// transactions.
    fn publish_controls(
        &mut self,
        session: &Session,
        plan: Vec<Planned>,
    ) -> Result<Refreshed, SessionError> {
        self.publish_controls_reaching(session, plan).0
    }

    /// [`Self::publish_controls`], and how many of `plan`'s entries
    /// committed ([`commit_reaching`]).
    fn publish_controls_reaching(
        &mut self,
        session: &Session,
        plan: Vec<Planned>,
    ) -> (Result<Refreshed, SessionError>, usize) {
        let mut result = Refreshed::default();
        if plan.is_empty() {
            return (Ok(result), 0);
        }
        count(&plan, &mut result);
        let max_ops = session.with_store(|store| store.limits().max_transaction_operations.max(1));
        let (transactions, reached, outcome) = commit_reaching(
            session,
            &plan,
            max_ops,
            MAX_TRANSACTION_PAYLOAD_BYTES,
            &mut self.published(),
        );
        result.transactions = transactions;
        (outcome.map(|()| result), reached)
    }

    /// The inspector's changes when the client holds `rows` out of the
    /// server-side collection `unfiltered` (PX-013).
    fn inspector_effects(
        &self,
        rows: &[Row],
        unfiltered: &[Row],
        unconfirmed: &HashSet<ItemId>,
    ) -> Vec<Planned> {
        let target = self
            .inspector
            .view(rows, unfiltered, unconfirmed, self.owner.names());
        let published = &self.inspector_published;
        [
            (INSPECTOR_STATUS, target.status, &published.status),
            (INSPECTOR_PROCESS, target.process, &published.process),
            (INSPECTOR_DETAILS, target.details, &published.details),
            (INSPECTOR_METRICS, target.metrics, &published.metrics),
            (INSPECTOR_COMMAND, target.command, &published.command),
        ]
        .into_iter()
        .filter(|(_, now, was)| now != *was)
        .map(|(node, text, _)| Planned::new(Effect::InspectorText { node, text }))
        .collect()
    }

    /// Applies one table selection (PX-013). `item` is the only thing the
    /// server reads from a `SELECTION_CHANGED`, and it is untrusted: it is
    /// resolved against the rows this view publishes, and only a published
    /// row becomes the inspector's target, under a new generation, with its
    /// details asked of the worker. Any other item — unknown, no longer
    /// published, or no item at all — is refused under a new generation too,
    /// so no earlier read can land on it, and nothing is read for it. The
    /// native highlight is the client's; nothing here selects a row.
    ///
    /// While follow is on (PX-017), a selection of another shown row makes
    /// that row the followed process: the table's `keep_visible` hint moves to
    /// it, and a promoted `value` naming another process is withdrawn in the
    /// same commit, which keeps the user's later local selection
    /// (PX-011-G01). A selection that names no shown row stops follow and
    /// clears the hint. Nothing is ever promoted on a selection, so no reply
    /// can replace a newer local selection, whatever the round trip.
    pub fn select_item(
        &mut self,
        session: &Session,
        client: &[u8],
        item: Option<ItemId>,
    ) -> Result<Refreshed, SessionError> {
        self.select_item_in(session, client, self.mode.node(), item)
    }

    /// [`Self::select_item`], reported by `client` on the collection `node`
    /// (PX-018). Only the shown collection's selection is the user's: a
    /// report from the hidden one — sent before a switch reached that client
    /// — names rows it no longer holds, and is ignored rather than taken for
    /// an empty selection, so it neither clears the inspector nor stops
    /// follow. A report naming a tree row now hidden under a parent the
    /// reporting client keeps collapsed is a selection that client has since
    /// dropped: the inspector is cleared and follow is left as it is. Another
    /// client's disclosure never hides it: each client discloses on its own,
    /// and one that attaches later starts all expanded.
    pub fn select_item_in(
        &mut self,
        session: &Session,
        client: &[u8],
        node: NodeId,
        item: Option<ItemId>,
    ) -> Result<Refreshed, SessionError> {
        if node != self.mode.node() {
            return Ok(Refreshed::default());
        }
        let row = item.and_then(|item| self.rows.iter().find(|row| row.item_id == item).cloned());
        // The selection is now this client's: its disclosure, and no other
        // client's, decides whether the inspector's and follow's process is
        // hidden (PX-018).
        self.holder = Holder::Client(client.to_vec());
        // A row hidden under a parent the client keeps collapsed cannot be
        // selected there: a report naming one was sent before a commit moved
        // the row under that parent, and the client has since dropped the
        // selection (`OutlineCollectionAdapter.update` restores it only to a
        // shown row, with selection events suppressed, and a later expand
        // restores nothing). The client now has no selection, so neither has
        // the inspector; follow is not a selection and stays on its process
        // (PX-018).
        let hidden = row
            .as_ref()
            .is_some_and(|row| self.hidden_under(&self.rows, row.item_id).is_some());
        if hidden {
            self.inspector.clear_hidden();
        } else {
            match &row {
                Some(row) => self.inspector.select(row),
                None => self.inspector.refuse(item),
            }
            self.follow.selected(row.as_ref());
        }
        self.publish_selection(session)
    }

    /// Applies a user-cleared selection (PX-017-G02): a `SELECTION_CHANGED`
    /// with no `value`, sent when the user left the shown collection with
    /// nothing selected. The inspector says the selection was cleared under a
    /// new generation and reads nothing; follow, if on, stops with that reason
    /// and clears its hint, and turning it on again is refused until a row is
    /// selected.
    pub fn clear_selection(&mut self, session: &Session) -> Result<Refreshed, SessionError> {
        self.clear_selection_in(session, &[], self.mode.node())
    }

    /// [`Self::clear_selection`], reported by `client` on the collection
    /// `node` (PX-018): a clear from the hidden collection, sent before a
    /// switch reached that client, is ignored like a selection from it. A
    /// clear is that client's selection, as a selection is. A collapse is
    /// not a clear: it sends only `EXPANSION_CHANGED`
    /// ([`Self::expansion_changed`]).
    pub fn clear_selection_in(
        &mut self,
        session: &Session,
        client: &[u8],
        node: NodeId,
    ) -> Result<Refreshed, SessionError> {
        if node != self.mode.node() {
            return Ok(Refreshed::default());
        }
        self.holder = Holder::Client(client.to_vec());
        self.inspector.clear();
        self.follow.cleared();
        self.publish_selection(session)
    }

    /// Publishes the inspector and follow after a selection or a clear.
    fn publish_selection(&mut self, session: &Session) -> Result<Refreshed, SessionError> {
        let mut plan = self.inspector_effects(&self.rows, &self.unfiltered_rows, &self.unconfirmed);
        let follow = self.follow_table_effects(&plan);
        plan.extend(follow);
        plan.extend(self.follow_view_effects());
        self.publish_controls(session, plan)
    }

    /// Follow selected process (PX-017).
    pub fn follow(&self) -> &Follow {
        &self.follow
    }

    /// The follow controls as this view last published them (PX-017).
    pub fn follow_view(&self) -> &FollowView {
        &self.follow_published
    }

    /// Applies one follow toggle request (PX-017): `Some(true)` turns follow
    /// on for the process the inspector describes, the user's own selection,
    /// and sets the table's `keep_visible` hint to it, which reveals it at
    /// once without touching the selection; `Some(false)` turns follow off and
    /// clears the hint; `None` (a request without a boolean) changes nothing.
    /// A selection that is not a shown row is refused with its reason. Nothing
    /// is promoted: a promoted `value` naming another process is withdrawn
    /// instead (PX-011-G01). The toggle's value is always published: the
    /// client's checkbox already shows what was asked, so a refusal puts it
    /// back (§6.3).
    pub fn set_follow(
        &mut self,
        session: &Session,
        on: Option<bool>,
    ) -> Result<Refreshed, SessionError> {
        match on {
            Some(true) => {
                let selected = self.inspector.selected_row().cloned();
                self.follow
                    .turn_on(selected.as_ref(), &self.rows, &self.unfiltered_rows);
            }
            Some(false) => self.follow.turn_off(),
            None => {}
        }
        let mut plan = self.follow_table_effects(&[]);
        let target = self.follow_view_in(self.mode, &self.rows);
        plan.push(Planned::new(Effect::FollowValue(target.checked)));
        if target.status != self.follow_published.status {
            plan.push(Planned::new(Effect::FollowStatus(target.status)));
        }
        self.publish_controls(session, plan)
    }

    /// The follow controls' changes over the rows already published (PX-017).
    fn follow_view_effects(&self) -> Vec<Planned> {
        follow_view_effects(
            &self.follow_view_in(self.mode, &self.rows),
            &self.follow_published,
        )
    }

    /// Follow's hold on the shown collection once `plan`, which changes no
    /// row, commits (PX-017): see [`follow_table_effects`]. A search
    /// promotion in `plan` is the promoted `value` after it.
    fn follow_table_effects(&self, plan: &[Planned]) -> Vec<Planned> {
        let promoted = promotion_in(plan).or(self.promoted);
        follow_table_effects(
            self.mode.node(),
            self.follow.followed(),
            self.keep_visible,
            promoted,
        )
    }

    /// Follow's plan once `plan` commits and the client holds `rows` in
    /// `mode` (PX-017, PX-018): its hold on that view's collection and the
    /// follow controls' changes. `fresh` is a collection this plan just filled
    /// by a view switch, which holds no hint and no promotion of its own yet:
    /// follow carries over by hinting the same item ID there.
    fn follow_after(
        &self,
        mode: ViewMode,
        rows: &[Row],
        fresh: bool,
        plan: &[Planned],
    ) -> Vec<Planned> {
        let (hinted, held) = if fresh {
            (None, None)
        } else {
            (self.keep_visible, self.promoted)
        };
        let promoted = promotion_in(plan).or(held);
        let mut follow =
            follow_table_effects(mode.node(), self.follow.followed(), hinted, promoted);
        follow.extend(follow_view_effects(
            &self.follow_view_in(mode, rows),
            &self.follow_published,
        ));
        follow
    }

    /// The follow controls for a client that holds `rows` in `mode`: in the
    /// tree, the status also says when the followed process is hidden under a
    /// parent the selecting client (`holder`) reported collapsed, because
    /// `keep_visible` never expands one and a local expand never scrolls
    /// (PX-017-G01, PX-018).
    fn follow_view_in(&self, mode: ViewMode, rows: &[Row]) -> FollowView {
        match mode {
            ViewMode::Table => self.follow.view(rows),
            ViewMode::Tree => {
                let hidden = self
                    .follow
                    .following()
                    .and_then(|item| self.hidden_under(rows, item));
                self.follow.view_tree(rows, hidden)
            }
        }
    }

    /// The outermost tree row the selecting client (`holder`) reported
    /// collapsed whose subtree in `rows` holds `item`, if any (PX-018). Every
    /// client holds a promotion made in process, so any client's collapsed
    /// parent hides that one.
    fn hidden_under<'a>(&self, rows: &'a [Row], item: ItemId) -> Option<&'a Row> {
        match &self.holder {
            Holder::Client(client) => {
                let collapsed = self.disclosure.of(client)?;
                outermost_collapsed(rows, item, |row| collapsed.contains(row))
            }
            Holder::Every => {
                if self.disclosure.clients.is_empty() {
                    return None;
                }
                outermost_collapsed(rows, item, |row| self.disclosure.any_collapsed(row))
            }
        }
    }

    /// Forgets every collapsed item the published tree no longer holds, with
    /// exactly the client's rule (`OutlineCollectionAdapter.pruneCollapsedItems`
    /// keeps every collapsed item its model still holds), for every client:
    /// an item that has no children for a while stays collapsed, and hides the
    /// children it gains again (PX-018-G01, PX-018). Outside the tree nothing
    /// is collapsed.
    fn prune_collapsed(&mut self) {
        if self.mode != ViewMode::Tree {
            self.disclosure.clear();
            self.revealing = None;
            return;
        }
        self.disclosure.prune_to(&self.rows);
    }

    /// Brings the server's mirror of the clients' disclosure and of the
    /// selection up to `target`, the rows a plan is about to publish (PX-018).
    ///
    /// A client keeps a collapsed item while its model holds it, and every
    /// item starts expanded in a tree a view switch (`fresh`) just filled. A
    /// shown row that `target` places under an item the selecting client
    /// keeps collapsed has no row on that client, so its native selection is
    /// gone: the outline restores a selection only to a shown row, with
    /// selection events suppressed while it applies the commit, so no
    /// `SELECTION_CHANGED` reports it. The inspector is cleared to match,
    /// exactly as for a collapse ([`Self::expansion_changed`]). Another
    /// client's collapsed items never clear it. Follow is not a selection: it
    /// stays on its process, and its status says it is hidden under the
    /// collapsed parent.
    ///
    /// The plan has not committed yet, so what this changes is held: if the
    /// rows never reach the client ([`Self::settle_disclosure`]), the mirror
    /// and the inspector are put back exactly as they were. Only a sample or
    /// a filter (the text filter, the user filter, the PID set) can move a
    /// row under a collapsed parent: a sort only reorders siblings, and a
    /// switch fills a tree that starts expanded.
    fn reconcile_disclosure(&mut self, target: &[Row], fresh: bool) -> HeldDisclosure {
        let held = HeldDisclosure {
            disclosure: self.disclosure.clone(),
            inspector: self.inspector.hold(),
        };
        if fresh {
            self.disclosure.clear();
        }
        self.disclosure.prune_to(target);
        let hidden = self
            .inspector
            .selected()
            .and_then(|selected| self.hidden_under(target, selected))
            .is_some();
        if hidden {
            self.inspector.clear_hidden();
        }
        held
    }

    /// Keeps what [`Self::reconcile_disclosure`] changed when the plan's rows
    /// `landed` — every row entry of the plan committed, as the commit itself
    /// reports ([`commit_reaching`]) — and puts it back when the commit
    /// stopped before them (PX-018): the client then still shows the old
    /// rows, its collapsed set and its selection. Comparing the rows instead
    /// would miss a reparent that keeps the preorder's item IDs in place.
    fn settle_disclosure(&mut self, held: HeldDisclosure, landed: bool) {
        if !landed {
            self.disclosure = held.disclosure;
            self.inspector.restore(held.inspector);
        }
    }

    /// Mirrors a revealing promotion of `item` on the tree (PX-018): every
    /// client applies the promotion and expands exactly its collapsed items
    /// whose subtree holds it
    /// (`OutlineCollectionAdapter.expandCollapsedAncestors`), and only then
    /// reports each with `EXPANSION_CHANGED(true)`. Every client's mirror
    /// expands them when the promotion is planned, so a sample landing before
    /// those reports sees the match shown, and the reports change nothing.
    /// Returns whether it expanded any.
    fn reveal_in_mirror(&mut self, rows: &[Row], item: ItemId) -> bool {
        if self.mode != ViewMode::Tree {
            return false;
        }
        self.disclosure.reveal(rows, item)
    }

    /// The row operations that turn the published rows into `target` in the
    /// shown view: moves and a diff in the table (PX-009), one index-range
    /// unit in the tree (PX-018).
    fn plan_rows(
        &self,
        target: &[Row],
        batch: usize,
        max_ops: usize,
        max_bytes: usize,
    ) -> Vec<Planned> {
        match self.mode {
            ViewMode::Table => {
                let moved = plan_moves(&self.rows, target, batch, max_ops, max_bytes);
                let mut plan = moved.plan;
                plan.extend(diff(&moved.rows, target, batch, max_bytes));
                plan
            }
            ViewMode::Tree => plan_tree(&self.rows, target, batch, max_ops, max_bytes),
        }
    }

    /// The joined unit that switches the shown view to `to` holding `target`
    /// (PX-018): the shown collection's model emptied by one index range, the
    /// other's filled with `target` under the same item IDs, the two
    /// collections' visibility swapped, the list-focus shortcut moved with
    /// them, and the switch's own value. The emptied model takes its
    /// collection's promoted selection and hint with it, which the SDK's
    /// settlement clears in the same commit.
    fn plan_switch(
        &self,
        to: ViewMode,
        target: &[Row],
        batch: usize,
        max_bytes: usize,
    ) -> Vec<Planned> {
        let from = self.mode;
        let mut plan = Vec::new();
        if !self.rows.is_empty() {
            plan.push(Planned::new(Effect::DeleteRange {
                model: from.model(),
                index: 0,
                count: self.rows.len() as u64,
            }));
        }
        push_inserts(&mut plan, to, 0, target, batch, max_bytes);
        plan.push(Planned::new(Effect::Shortcut {
            node: from.node(),
            set: false,
        }));
        plan.push(Planned::new(Effect::Visibility {
            node: from.node(),
            shown: false,
        }));
        plan.push(Planned::new(Effect::Visibility {
            node: to.node(),
            shown: true,
        }));
        plan.push(Planned::new(Effect::Shortcut {
            node: to.node(),
            set: true,
        }));
        plan.push(Planned::new(Effect::ViewMode(to)));
        let last = plan.len() - 1;
        for planned in &mut plan[..last] {
            planned.joins_next = true;
        }
        plan
    }

    /// The view status's change for `mode` showing `forest` (PX-018).
    fn view_effects(&self, mode: ViewMode, forest: Option<&ForestSummary>) -> Vec<Planned> {
        let status = view_status(mode, forest);
        if status == self.view_status {
            Vec::new()
        } else {
            vec![Planned::new(Effect::ViewStatus(status))]
        }
    }

    /// Applies one view switch request (PX-018): `Some(true)` shows the
    /// process tree, `Some(false)` the table, `None` (a request without a
    /// boolean) changes nothing. The same rows are published under the same
    /// item IDs, the selection is carried over by item ID, and follow is
    /// carried into the shown view by hinting the same item there. The
    /// switch's value is always published: the client's checkbox already
    /// shows what was asked, so a request that changes nothing puts it back
    /// (§6.3). A paused view switches too, over its frozen rows.
    pub fn set_view(
        &mut self,
        session: &Session,
        on: Option<bool>,
    ) -> Result<Refreshed, SessionError> {
        let to = match on {
            Some(true) => ViewMode::Tree,
            Some(false) => ViewMode::Table,
            None => self.mode,
        };
        if to == self.mode {
            let plan = vec![Planned::new(Effect::ViewMode(to))];
            return self.publish_controls(session, plan);
        }
        self.republish_in(session, Vec::new(), Some(to))
    }

    /// Which tasks the collection holds (PX-019).
    pub fn thread_settings(&self) -> ThreadSettings {
        self.threads
    }

    /// The threads status this view last published (PX-019).
    pub fn threads_status(&self) -> &str {
        &self.threads_published.status
    }

    /// Whether the next sample should read user threads (PX-019): while they
    /// are shown. The refresh loop asks the source before every sample.
    pub fn threads_requested(&self) -> bool {
        self.threads.user
    }

    /// How many rows the thread controls let the collection hold (PX-019).
    fn admitted_rows(&self) -> usize {
        self.unfiltered_rows
            .iter()
            .filter(|row| threads::admits(self.threads, row))
            .count()
    }

    /// The thread controls' changes for the server-side collection
    /// `unfiltered` (PX-019).
    fn threads_effects(&self, unfiltered: &[Row]) -> Vec<Planned> {
        let status = threads::status(
            self.threads,
            self.thread_report,
            ThreadCounts::of(unfiltered),
            self.sampling.frozen().is_some(),
        );
        let published = &self.threads_published;
        let mut plan = Vec::new();
        if status != published.status {
            plan.push(Planned::new(Effect::ThreadsStatus(status)));
        }
        for (node, was, now) in [
            (USER_THREADS_TOGGLE, published.user, self.threads.user),
            (KERNEL_THREADS_TOGGLE, published.kernel, self.threads.kernel),
        ] {
            if was != now {
                plan.push(Planned::new(Effect::ThreadToggle { node, on: now }));
            }
        }
        plan
    }

    /// Applies one "Show user threads" request (PX-019): `Some(true)` shows
    /// user threads, `Some(false)` hides them, `None` (a request without a
    /// boolean) changes nothing. The checkbox's value is always published, so
    /// a request that changes nothing puts the client's checkbox back (§6.3).
    ///
    /// Showing them asks the refresh loop for one sample at once, which reads
    /// them; until it is published the threads status says they appear with
    /// the next sample, and a paused view reads none until it is resumed.
    /// Hiding them republishes at once without their rows, which the next
    /// sample, which no longer reads them, then drops; an inspector that
    /// described a thread is cleared, as the client's selection of that row is
    /// gone with it. That is not a user deselect (PX-017-G02): the inspector
    /// says nothing is selected, not that the selection was cleared, and
    /// follow states its own reason.
    pub fn set_user_threads(
        &mut self,
        session: &Session,
        on: Option<bool>,
    ) -> Result<Refreshed, SessionError> {
        let Some(on) = on.filter(|on| *on != self.threads.user) else {
            let plan = vec![Planned::new(Effect::ThreadToggle {
                node: USER_THREADS_TOGGLE,
                on: self.threads.user,
            })];
            return self.publish_controls(session, plan);
        };
        let before = self.threads;
        // A sample already being taken was taken under the other setting: it
        // is discarded when it returns, so a scan that read no threads never
        // lands after they are shown again, nor one that read them after they
        // are hidden (PX-019, merge verification F3).
        self.sampling.invalidate();
        let held = self.inspector.hold();
        let selected_thread = self
            .inspector
            .selected_row()
            .filter(|row| row.key.is_thread())
            .map(|row| row.item_id);
        self.threads.user = on;
        if on {
            self.sampling.refresh_now();
        } else if selected_thread.is_some() {
            // The row left the collection; the user did not deselect it
            // (PX-017-G02's `clear`), so this is the hidden-row clear.
            self.inspector.clear_hidden();
        }
        let published = self.republish(session, Vec::new());
        self.settle_thread_request(before, held, selected_thread);
        published
    }

    /// Puts back what a thread checkbox request changed ahead of its plan
    /// when the plan did not reach the client (PX-019, on PX-018's rule): the
    /// setting, when the checkbox's own value never committed, and the
    /// inspector on a thread, when that thread's row is still published —
    /// the client then still shows it, selected.
    fn settle_thread_request(
        &mut self,
        before: ThreadSettings,
        held: crate::inspector::HeldSelection,
        selected_thread: Option<ItemId>,
    ) {
        if self.threads_published.user != self.threads.user
            || self.threads_published.kernel != self.threads.kernel
        {
            self.threads = ThreadSettings {
                user: if self.threads_published.user == self.threads.user {
                    self.threads.user
                } else {
                    before.user
                },
                kernel: if self.threads_published.kernel == self.threads.kernel {
                    self.threads.kernel
                } else {
                    before.kernel
                },
            };
        }
        if selected_thread.is_some_and(|item| self.rows.iter().any(|row| row.item_id == item))
            && self.inspector.selected() != selected_thread
        {
            self.inspector.restore(held);
        }
    }

    /// Applies one "Show kernel threads" request (PX-019), like
    /// [`Self::set_user_threads`]: the kernel's tasks are already in every
    /// sample, so hiding or showing them republishes at once and samples
    /// nothing. A hidden kernel thread is hidden as a filter hides a row.
    pub fn set_kernel_threads(
        &mut self,
        session: &Session,
        on: Option<bool>,
    ) -> Result<Refreshed, SessionError> {
        let Some(on) = on.filter(|on| *on != self.threads.kernel) else {
            let plan = vec![Planned::new(Effect::ThreadToggle {
                node: KERNEL_THREADS_TOGGLE,
                on: self.threads.kernel,
            })];
            return self.publish_controls(session, plan);
        };
        let before = self.threads;
        let held = self.inspector.hold();
        self.threads.kernel = on;
        let published = self.republish(session, Vec::new());
        self.settle_thread_request(before, held, None);
        published
    }

    /// Applies one `EXPANSION_CHANGED` report from the tree (PX-018): the
    /// reporting `client` has already expanded or collapsed `item` locally,
    /// and the server only records what that client was told. Nothing about
    /// the rows is published: disclosure is each client's own (§22.5), and a
    /// report changes only its own client's mirror.
    ///
    /// A collapse that hides the inspector's process on the client whose
    /// selection it is (`holder`) has also deselected its row there, without
    /// a `SELECTION_CHANGED` (an `NSOutlineView` cannot select a row it does
    /// not show), so the inspector is cleared to match instead of describing
    /// a selection that client no longer shows; another client's collapse
    /// leaves it. Follow is not a selection and stays on its process; its
    /// status says it is hidden under the collapsed parent until that is
    /// expanded again.
    /// A report from the hidden collection, about an item the tree does not
    /// hold, or one that changes nothing, publishes nothing. A report about
    /// an item that has become a leaf since the client acted is applied: the
    /// client keeps that item's state, and its children hide or show by it
    /// when they return.
    pub fn expansion_changed(
        &mut self,
        session: &Session,
        client: &[u8],
        node: NodeId,
        item: ItemId,
        expanded: bool,
    ) -> Result<Refreshed, SessionError> {
        let observed = session.current_revision();
        self.expansion_changed_at(session, client, node, item, expanded, observed)
    }

    /// [`Self::expansion_changed`], stamped by the client with revision
    /// `observed` (Codex review round 5). A collapse stamped at or before the
    /// revision every client held before the latest revealing promotion, of
    /// an item whose subtree holds that promotion's match, was made before
    /// the client applied the promotion, and the client undoes it then: it
    /// expands the collapsed items above the match, selects the match with
    /// selection events suppressed
    /// (`OutlineCollectionAdapter.resolvePendingPromotion`,
    /// `expandCollapsedAncestors`) and reports the expand. Such a collapse
    /// therefore leaves the mirror and the inspector as they are, so the
    /// newer selection the promotion made is kept.
    ///
    /// The converse does not hold (verification V6-F1, a known limitation):
    /// a report stamped at the promotion's revision or later was not
    /// necessarily made after the outline applied the promotion. The client
    /// stamps it with the applier's published snapshot
    /// (`SessionController.enqueueNativeSemanticAction`,
    /// `client-macos/Session/SessionController.swift:1201-1206`, through
    /// `semanticInspectionSourceSnapshot`), which advances before the
    /// main-actor render that calls `applyPromotion`. A collapse made in that
    /// gap carries the promotion's own revision, is taken as fresh and clears
    /// the inspector, while the render then expands the parent and selects
    /// the match. No server-side boundary separates it from a collapse made
    /// after the render, which does deselect the row; closing the gap needs
    /// the client to stamp disclosure and selection events with the revision
    /// its renderer has applied.
    pub fn expansion_changed_at(
        &mut self,
        session: &Session,
        client: &[u8],
        node: NodeId,
        item: ItemId,
        expanded: bool,
        observed: u64,
    ) -> Result<Refreshed, SessionError> {
        if node != TREE || self.mode != ViewMode::Tree {
            return Ok(Refreshed::default());
        }
        let undone = !expanded
            && self.revealing.as_ref().is_some_and(|revealing| {
                observed <= revealing.before && subtree_holds(&self.rows, item, revealing.item)
            });
        if undone {
            return Ok(Refreshed::default());
        }
        // The client reports only an item that had children in the revision
        // it acted on (`OutlineCollectionAdapter.setExpanded`), but a sample
        // may have taken its last child since: the report is applied whatever
        // the item's children are now, exactly as the client applied it, and
        // ignored only for an item the tree no longer holds, which the client
        // has dropped from its own collapsed set (`pruneCollapsedItems`).
        if !self.rows.iter().any(|row| row.item_id == item) {
            return Ok(Refreshed::default());
        }
        if !self.disclosure.record(client, item, expanded) {
            return Ok(Refreshed::default());
        }
        // Only a collapse by the client that holds the selection can hide it.
        if !expanded && self.holder.is(client) {
            let hidden = self
                .inspector
                .selected()
                .and_then(|selected| self.hidden_under(&self.rows, selected))
                .is_some();
            if hidden {
                self.inspector.clear_hidden();
            }
        }
        let mut plan = self.inspector_effects(&self.rows, &self.unfiltered_rows, &self.unconfirmed);
        plan.extend(self.follow_view_effects());
        self.publish_controls(session, plan)
    }

    /// Publishes one detail result (PX-013), if it answers the inspector's
    /// current target and generation; any other result is dropped.
    pub fn details_read(
        &mut self,
        session: &Session,
        request: &DetailRequest,
        read: DetailRead,
    ) -> Result<Refreshed, SessionError> {
        if !self.inspector.completed(request, read) {
            return Ok(Refreshed::default());
        }
        let plan = self.inspector_effects(&self.rows, &self.unfiltered_rows, &self.unconfirmed);
        self.publish_controls(session, plan)
    }

    /// The sampling interval, the pause state and the loop's requests
    /// (PX-015).
    pub fn sampling(&self) -> &Sampling {
        &self.sampling
    }

    /// Where this view is frozen, if it is paused (PX-015). The rows it then
    /// shows are historical data, not the host's current processes: a future
    /// process-changing action must refuse every target while this is `Some`
    /// and be disabled on the client. None exists yet.
    pub fn frozen(&self) -> Option<Frozen> {
        self.sampling.frozen()
    }

    /// The sampling controls as this view last published them (PX-015).
    pub fn sampling_view(&self) -> &SamplingView {
        &self.sampling_published
    }

    /// The sampling controls' changes (PX-015).
    fn sampling_effects(&self) -> Vec<Planned> {
        let target = self.sampling.view();
        let published = &self.sampling_published;
        let mut plan = Vec::new();
        if target.status != published.status {
            plan.push(Planned::new(Effect::SamplingStatus(target.status)));
        }
        if target.pause_label != published.pause_label {
            plan.push(Planned::new(Effect::SamplingLabel(target.pause_label)));
        }
        if target.refresh_enabled != published.refresh_enabled {
            plan.push(Planned::new(Effect::SamplingEnabled(
                target.refresh_enabled,
            )));
        }
        plan
    }

    /// `summary` with its data-freshness line labelled for the sampling
    /// state (PX-007, PX-015): paused, resuming, or unlabelled while live.
    /// Only that line changes; every figure stays the one it was.
    fn labelled(&self, mut summary: summary::Properties) -> summary::Properties {
        let key = (summary::FRESHNESS_TEXT, TEXT);
        if let Some(Value::String(text)) = summary.get(&key) {
            let text = crate::bounded(
                &relabel_freshness(text, self.sampling.freshness_prefix()),
                summary::MAX_SUMMARY_TEXT_BYTES,
            );
            summary.insert(key, Value::String(text));
        }
        summary
    }

    /// Applies one Pause view/Resume view click (PX-015), resolved against
    /// the state this view holds when the click is applied.
    pub fn toggle_pause(&mut self, session: &Session) -> Result<Refreshed, SessionError> {
        if self.sampling.frozen().is_some() {
            self.resume_view(session)
        } else {
            self.pause_view(session)
        }
    }

    /// Freezes the published view at the session's current revision
    /// (PX-015). The rows, cells and summary figures stay exactly what that
    /// revision holds: the refresh loop takes no sample until Resume view,
    /// and a sample already being taken is discarded when it returns. This
    /// commits only the sampling controls, the freshness line's label and
    /// the inspector, so nothing a client already shows is replaced, and
    /// every other control keeps working on the frozen rows. Pausing a paused
    /// view changes nothing.
    pub fn pause_view(&mut self, session: &Session) -> Result<Refreshed, SessionError> {
        if self.sampling.frozen().is_some() {
            return Ok(Refreshed::default());
        }
        self.sampling.pause(Frozen {
            revision: session.current_revision(),
            paused_at: SystemTime::now(),
        });
        self.inspector.pause();
        self.publish_sampling(session)
    }

    /// Resumes a paused view (PX-015): the sampling status says the view is
    /// resuming, and the refresh loop takes one sample at once and publishes
    /// it as one current view — rows keep their item IDs, ended processes
    /// are deleted and new ones inserted, by the same diff every refresh
    /// uses. No sample missed while paused was ever taken, so none is
    /// replayed. Resuming a live view changes nothing.
    pub fn resume_view(&mut self, session: &Session) -> Result<Refreshed, SessionError> {
        if self.sampling.frozen().is_none() {
            return Ok(Refreshed::default());
        }
        self.sampling.resume();
        self.inspector.resume();
        self.publish_sampling(session)
    }

    /// Commits the sampling controls, the freshness label and the inspector
    /// for the current sampling state.
    fn publish_sampling(&mut self, session: &Session) -> Result<Refreshed, SessionError> {
        let mut plan = self.sampling_effects();
        // The threads status says whether a hidden-until-sampled thread waits
        // for Resume view (PX-019), so it changes with the pause too.
        plan.extend(self.threads_effects(&self.unfiltered_rows));
        let summary = self.labelled(self.summary.clone());
        plan.extend(summary_effects(&self.summary, &summary));
        plan.extend(self.inspector_effects(&self.rows, &self.unfiltered_rows, &self.unconfirmed));
        self.publish_controls(session, plan)
    }

    /// Applies one Refresh now click (PX-015): asks the refresh loop for one
    /// bounded sample through the ordinary refresh path, coalesced with any
    /// press it has not taken yet. Refused while paused, where the button is
    /// disabled: a paused view is refreshed only by Resume view. Commits
    /// nothing itself; says whether a sample was asked for.
    pub fn refresh_now(&mut self) -> bool {
        self.sampling.refresh_now()
    }

    /// Keeps the interval field's text (PX-015), at most
    /// [`crate::sampling::MAX_INTERVAL_INPUT_BYTES`] of it, and clears a
    /// refusal of the text it replaced. Nothing is applied until Apply
    /// interval.
    pub fn set_interval_input(
        &mut self,
        session: &Session,
        text: &str,
    ) -> Result<Refreshed, SessionError> {
        let kept = self.sampling.set_input(text).to_string();
        let mut plan = self.sampling_effects();
        if input_value(session, INTERVAL_INPUT) != kept {
            plan.push(Planned::new(Effect::SamplingValue(kept)));
        }
        self.publish_controls(session, plan)
    }

    /// Applies one Apply interval click (PX-015): the interval field's text
    /// becomes the sampling interval, or is refused with its reason on the
    /// sampling status while the previous interval is kept. An accepted
    /// interval restarts the loop's one ticker, so the next periodic sample
    /// is one new interval later.
    pub fn apply_interval(
        &mut self,
        session: &Session,
    ) -> Result<Result<Duration, IntervalError>, SessionError> {
        let outcome = self.sampling.apply_input();
        let plan = self.sampling_effects();
        self.publish_controls(session, plan)?;
        Ok(outcome)
    }

    /// Makes `interval` the interval the controls state, as the refresh loop
    /// starts with it. Publishes nothing when it already is.
    pub fn adopt_interval(
        &mut self,
        session: &Session,
        interval: Duration,
    ) -> Result<Refreshed, SessionError> {
        if self.sampling.interval() == interval {
            return Ok(Refreshed::default());
        }
        self.sampling.set_interval(interval);
        let kept = self
            .sampling
            .set_input(&self.sampling.interval().as_millis().to_string())
            .to_string();
        let mut plan = self.sampling_effects();
        if input_value(session, INTERVAL_INPUT) != kept {
            plan.push(Planned::new(Effect::SamplingValue(kept)));
        }
        self.publish_controls(session, plan)
    }

    /// Publishes `snapshot`, sampled under `epoch`, if that is still this
    /// view's epoch and the view is live (PX-015). A sample taken before a
    /// pause or a resume is discarded, so it can neither change frozen rows
    /// nor stand in for the sample Resume view asked for.
    pub fn apply_sampled(
        &mut self,
        session: &Session,
        epoch: u64,
        source_status: &str,
        snapshot: &ProcessSnapshot,
    ) -> Result<Refreshed, SessionError> {
        if !self.sampling.accepts(epoch) {
            return Ok(Refreshed::default());
        }
        self.apply(session, source_status, snapshot)
    }

    /// The inspector's server-side state (PX-013).
    pub fn inspector(&self) -> &Inspector {
        &self.inspector
    }

    /// The inspector's lines as this view last published them (PX-013).
    pub fn inspector_view(&self) -> &InspectorView {
        &self.inspector_published
    }
}

/// Dispatches one `SELECTION_CHANGED` on the table to the shared view
/// (PX-013). Only the event's `ItemId` argument is read. An event with no
/// `value` is the user clearing the selection (PX-017-G02); any other value
/// names no row.
pub fn selection_changed(
    view: &std::sync::Mutex<ProcessView>,
    session: &Session,
    event: &srui_protocol::Event,
) {
    let client = event.client_instance_id.as_slice();
    // The table's and the tree's selections both reach here (PX-018); only
    // the shown collection's is the user's, and it is the reporting client's.
    let node = NodeId::new(event.node_id);
    // A report whose arguments do not decode names no row and is refused,
    // like one whose `value` is not an `ItemId`; it is never a clear
    // (PX-013, PX-017-G02).
    let change = srui_semantic_tree::Event::try_from(event.clone())
        .ok()
        .and_then(|event| event.selection_change());
    let mut view = lock_view(view);
    let published = match change {
        Some(srui_sdk::SelectionChange::Cleared) => view.clear_selection_in(session, client, node),
        Some(srui_sdk::SelectionChange::Item(item)) => {
            view.select_item_in(session, client, node, Some(item))
        }
        None => view.select_item_in(session, client, node, None),
    };
    if let Err(error) = published {
        eprintln!("srtop: inspector not published: {error}");
    }
}

/// Dispatches one `VALUE_CHANGED` on the view switch to the shared view
/// (PX-018). Only the event's boolean argument is read.
pub fn view_toggled(
    view: &std::sync::Mutex<ProcessView>,
    session: &Session,
    event: &srui_protocol::Event,
) {
    let on = srui_semantic_tree::Event::try_from(event.clone())
        .ok()
        .and_then(|event| event.bool_arg());
    if let Err(error) = lock_view(view).set_view(session, on) {
        eprintln!("srtop: view not published: {error}");
    }
}

/// Dispatches one `VALUE_CHANGED` on the user-thread checkbox to the shared
/// view (PX-019). Only the event's boolean argument is read.
pub fn user_threads_toggled(
    view: &std::sync::Mutex<ProcessView>,
    session: &Session,
    event: &srui_protocol::Event,
) {
    let on = srui_semantic_tree::Event::try_from(event.clone())
        .ok()
        .and_then(|event| event.bool_arg());
    if let Err(error) = lock_view(view).set_user_threads(session, on) {
        eprintln!("srtop: user threads not published: {error}");
    }
}

/// Dispatches one `VALUE_CHANGED` on the kernel-thread checkbox to the
/// shared view (PX-019). Only the event's boolean argument is read.
pub fn kernel_threads_toggled(
    view: &std::sync::Mutex<ProcessView>,
    session: &Session,
    event: &srui_protocol::Event,
) {
    let on = srui_semantic_tree::Event::try_from(event.clone())
        .ok()
        .and_then(|event| event.bool_arg());
    if let Err(error) = lock_view(view).set_kernel_threads(session, on) {
        eprintln!("srtop: kernel threads not published: {error}");
    }
}

/// Dispatches one `EXPANSION_CHANGED` on the tree to the shared view
/// (PX-018). Only a well-formed report is read: its one `ItemId` and its
/// boolean; anything else names nothing. It is mirrored for the client
/// instance that sent it, which the session checked against that
/// connection's handshake.
pub fn expansion_reported(
    view: &std::sync::Mutex<ProcessView>,
    session: &Session,
    event: &srui_protocol::Event,
) {
    let Some(change) = srui_semantic_tree::Event::try_from(event.clone())
        .ok()
        .as_ref()
        .and_then(srui_sdk::ExpansionChange::from_event)
    else {
        return;
    };
    let published = lock_view(view).expansion_changed_at(
        session,
        &event.client_instance_id,
        change.node,
        change.item,
        change.expanded,
        event.observed_revision,
    );
    if let Err(error) = published {
        eprintln!("srtop: disclosure not published: {error}");
    }
}

/// Dispatches one `VALUE_CHANGED` on the follow toggle to the shared view
/// (PX-017). Only the event's boolean argument is read.
pub fn follow_toggled(
    view: &std::sync::Mutex<ProcessView>,
    session: &Session,
    event: &srui_protocol::Event,
) {
    let on = srui_semantic_tree::Event::try_from(event.clone())
        .ok()
        .and_then(|event| event.bool_arg());
    if let Err(error) = lock_view(view).set_follow(session, on) {
        eprintln!("srtop: follow not published: {error}");
    }
}

/// What [`ProcessView::reconcile_disclosure`] changed, held until its plan
/// commits (PX-018).
struct HeldDisclosure {
    disclosure: Disclosure,
    inspector: HeldSelection,
}

/// Most clients whose tree disclosure one view mirrors at once (PX-018).
///
/// Nothing srtop receives announces a detach, so the mirror of a client
/// that collapsed a parent and went away stays until its items leave the
/// tree, the view leaves the tree, or it is the least recently reporting of
/// more clients than this. An evicted client is mirrored all expanded until
/// it reports again.
pub const MAX_DISCLOSURE_CLIENTS: usize = 64;

/// The tree items each client reported collapsed (PX-018), by the
/// `client_instance_id` of the reports. A client with nothing collapsed has
/// no entry: it shows the tree all expanded, as every client does before it
/// first collapses an item, a fresh attachment included.
#[derive(Debug, Clone, Default)]
struct Disclosure {
    clients: HashMap<Vec<u8>, ClientDisclosure>,
    /// Orders the clients' last reports, for eviction.
    next_report: u64,
}

/// One client's collapsed items and when it last reported (PX-018).
#[derive(Debug, Clone)]
struct ClientDisclosure {
    collapsed: HashSet<ItemId>,
    reported: u64,
}

impl Disclosure {
    /// What `client` keeps collapsed; `None` when it keeps nothing collapsed.
    fn of(&self, client: &[u8]) -> Option<&HashSet<ItemId>> {
        self.clients.get(client).map(|entry| &entry.collapsed)
    }

    /// Whether any client keeps `item` collapsed.
    fn any_collapsed(&self, item: &ItemId) -> bool {
        self.clients
            .values()
            .any(|entry| entry.collapsed.contains(item))
    }

    /// Records one report from `client`; whether it changed its mirror.
    fn record(&mut self, client: &[u8], item: ItemId, expanded: bool) -> bool {
        let reported = self.next_report;
        self.next_report = self.next_report.wrapping_add(1);
        if expanded {
            let Some(entry) = self.clients.get_mut(client) else {
                return false;
            };
            entry.reported = reported;
            if !entry.collapsed.remove(&item) {
                return false;
            }
            if entry.collapsed.is_empty() {
                self.clients.remove(client);
            }
            return true;
        }
        if let Some(entry) = self.clients.get_mut(client) {
            entry.reported = reported;
            return entry.collapsed.insert(item);
        }
        if self.clients.len() >= MAX_DISCLOSURE_CLIENTS {
            // lc-debt: least-recently-reporting eviction stands in for a detach
            // notice srtop does not get (`srui_sessiond::handle_connection`
            // exposes no attachment identity); drop a client's entry on its
            // detach once the SDK reports one.
            let oldest = self
                .clients
                .iter()
                .min_by_key(|(_, entry)| entry.reported)
                .map(|(id, _)| id.clone());
            if let Some(oldest) = oldest {
                self.clients.remove(&oldest);
            }
        }
        self.clients.insert(
            client.to_vec(),
            ClientDisclosure {
                collapsed: HashSet::from([item]),
                reported,
            },
        );
        true
    }

    /// Forgets every client's collapsed items: a tree that is emptied or
    /// filled afresh starts expanded on every client.
    fn clear(&mut self) {
        self.clients.clear();
    }

    /// Keeps only the collapsed items `rows` still hold, for every client.
    fn prune_to(&mut self, rows: &[Row]) {
        if self.clients.is_empty() {
            return;
        }
        let held: HashSet<ItemId> = rows.iter().map(|row| row.item_id).collect();
        self.clients.retain(|_, entry| {
            entry.collapsed.retain(|item| held.contains(item));
            !entry.collapsed.is_empty()
        });
    }

    /// Expands, for every client, the items whose subtree in `rows` holds
    /// `item`; whether any was collapsed.
    fn reveal(&mut self, rows: &[Row], item: ItemId) -> bool {
        if self.clients.is_empty() {
            return false;
        }
        let Some(at) = rows.iter().position(|row| row.item_id == item) else {
            return false;
        };
        let ancestors: Vec<ItemId> = rows[..at]
            .iter()
            .enumerate()
            .filter(|(index, row)| holds(row, *index, at))
            .map(|(_, row)| row.item_id)
            .collect();
        let mut opened = false;
        self.clients.retain(|_, entry| {
            for ancestor in &ancestors {
                opened |= entry.collapsed.remove(ancestor);
            }
            !entry.collapsed.is_empty()
        });
        opened
    }
}

/// A revealing promotion committed in the tree, and the revision every
/// client held before it (PX-018, Codex review round 5).
#[derive(Debug, Clone, Copy)]
struct Revealing {
    item: ItemId,
    before: u64,
}

/// Whether `ancestor`'s subtree in the tree `rows` holds `item`.
fn subtree_holds(rows: &[Row], ancestor: ItemId, item: ItemId) -> bool {
    let Some(at) = rows.iter().position(|row| row.item_id == item) else {
        return false;
    };
    rows[..at]
        .iter()
        .enumerate()
        .any(|(index, row)| row.item_id == ancestor && holds(row, index, at))
}

/// Whose selection the inspector and follow describe (PX-018).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Holder {
    /// A promotion made in process, which every client applies.
    Every,
    /// The selection one client reported, or a promotion it asked for.
    Client(Vec<u8>),
}

impl Holder {
    /// Whether `client`'s disclosure can hide this holder's selection.
    fn is(&self, client: &[u8]) -> bool {
        match self {
            Holder::Every => true,
            Holder::Client(holder) => holder.as_slice() == client,
        }
    }
}

/// Whether the tree row at `index` holds the row at `at` in its subtree.
fn holds(row: &Row, index: usize, at: usize) -> bool {
    row.tree
        .as_ref()
        .is_some_and(|place| at as u64 <= index as u64 + place.descendants)
}

/// The outermost row of `rows` that `collapsed` names and whose subtree
/// holds `item`, if any (PX-018).
fn outermost_collapsed(
    rows: &[Row],
    item: ItemId,
    collapsed: impl Fn(&ItemId) -> bool,
) -> Option<&Row> {
    let at = rows.iter().position(|row| row.item_id == item)?;
    rows[..at]
        .iter()
        .enumerate()
        .find(|(index, row)| collapsed(&row.item_id) && holds(row, *index, at))
        .map(|(_, row)| row)
}

/// The item the last promotion in `plan` names, if it promotes one.
fn promotion_in(plan: &[Planned]) -> Option<ItemId> {
    plan.iter().rev().find_map(|planned| match &planned.effect {
        Effect::Promote(_, row) => Some(row.item_id),
        _ => None,
    })
}

/// The view status for `mode` showing `forest` (PX-018): counts and fixed
/// wording only, so no process can write into it.
fn view_status(mode: ViewMode, forest: Option<&ForestSummary>) -> String {
    let (ViewMode::Tree, Some(forest)) = (mode, forest) else {
        return TABLE_VIEW_STATUS.to_string();
    };
    let processes = forest.rows - forest.threads;
    // Threads are counted apart from processes, never as processes (PX-019).
    let threads = match forest.threads {
        0 => String::new(),
        1 => " and 1 thread".to_string(),
        count => format!(" and {count} threads"),
    };
    let mut clauses = vec![format!(
        "View: process tree · {} {}{threads} under {} {}",
        processes,
        if processes == 1 {
            "process"
        } else {
            "processes"
        },
        forest.roots,
        if forest.roots == 1 { "root" } else { "roots" },
    )];
    if forest.orphans > 0 {
        let mut why = Vec::new();
        for (count, text) in [
            (forest.absent, "parent not in this scan or unread"),
            (forest.reused, "parent PID reused"),
            (forest.hidden, "parent hidden by the filters"),
            (forest.malformed, "cycle or depth limit"),
        ] {
            if count > 0 {
                why.push(format!("{count} {text}"));
            }
        }
        clauses.push(format!(
            "{} without a shown parent ({}); the Parent column says why",
            forest.orphans,
            why.join(", ")
        ));
    }
    clauses.push("expand and collapse are local; sorting orders each parent's children".into());
    bounded(&clauses.join(" · "), MAX_VIEW_STATUS_BYTES)
}

/// Follow's hold on the collection `node` (PX-017), from what it holds now:
/// `hinted`, its `keep_visible` hint, and `promoted`, its promoted `value`.
///
/// - While follow is on, the hint names `followed`: set when it names
///   anything else (a turn-on, a retarget, or the repair of a set that did
///   not commit), never again for the same item, so a sample never re-sets
///   it. A promoted `value` naming another process is withdrawn in the same
///   commit, so a client that attaches later restores no selection of a
///   process follow is not on; a live client keeps its own later selection
///   (PX-011-G01).
/// - While follow is off, no hint is held.
///
/// Nothing here ever promotes.
fn follow_table_effects(
    node: NodeId,
    followed: Option<&Row>,
    hinted: Option<ItemId>,
    promoted: Option<ItemId>,
) -> Vec<Planned> {
    let mut plan = Vec::new();
    match followed {
        Some(row) => {
            if hinted != Some(row.item_id) {
                plan.push(Planned::new(Effect::KeepVisible(
                    node,
                    Box::new(row.clone()),
                )));
            }
            if promoted.is_some_and(|item| item != row.item_id) {
                plan.push(Planned::new(Effect::Withdraw(node)));
            }
        }
        None => {
            if hinted.is_some() {
                plan.push(Planned::new(Effect::ClearKeepVisible(node)));
            }
        }
    }
    // The hint and the withdrawal commit together, so no client ever holds
    // the new hint beside a promotion of another process.
    if let [first, _] = plan.as_mut_slice() {
        first.joins_next = true;
    }
    plan
}

/// The follow controls' changes from `published` to `target` (PX-017).
fn follow_view_effects(target: &FollowView, published: &FollowView) -> Vec<Planned> {
    let mut plan = Vec::new();
    if target.checked != published.checked {
        plan.push(Planned::new(Effect::FollowValue(target.checked)));
    }
    if target.status != published.status {
        plan.push(Planned::new(Effect::FollowStatus(target.status.clone())));
    }
    plan
}

/// Starts the inspector's detail worker over `details` and installs it in
/// `view` (PX-013). The worker holds the view and the session only weakly: it
/// publishes while both exist, and ends once the view is dropped and its
/// current read returns. `None` if no thread could be started; the inspector
/// then shows selected processes without details.
pub fn start_inspector(
    session: &Arc<Session>,
    view: &SharedView,
    details: Arc<dyn DetailSource>,
) -> Option<DetailReadsIdle> {
    let weak_view = Arc::downgrade(view);
    let weak_session = Arc::downgrade(session);
    let reads = DetailReads::start(details, move |request, read| {
        let (Some(view), Some(session)) = (weak_view.upgrade(), weak_session.upgrade()) else {
            return;
        };
        let published = lock_view(&view).details_read(&session, &request, read);
        if let Err(error) = published {
            eprintln!("srtop: inspector details not published: {error}");
        }
    })?;
    let idle = reads.idle();
    lock_view(view).inspector.install(reads);
    Some(idle)
}

/// The string `node` holds as its `VALUE`, or the empty string.
fn input_value(session: &Session, node: NodeId) -> String {
    session.with_store(|store| {
        store
            .get_node(node)
            .and_then(|n| n.get_property(VALUE))
            .and_then(Value::as_string)
            .unwrap_or("")
            .to_string()
    })
}

/// The text a `TEXT_EDIT` event on `node` carries, or else the value the store
/// holds for it.
fn edited_text(session: &Session, event: &srui_protocol::Event, node: NodeId) -> String {
    srui_semantic_tree::Event::try_from(event.clone())
        .ok()
        .and_then(|e| e.text_arg().map(|s| s.to_string()))
        .unwrap_or_else(|| input_value(session, node))
}

/// Dispatches one Pause view/Resume view click to the shared view (PX-015).
pub fn pause_toggled(view: &std::sync::Mutex<ProcessView>, session: &Session) {
    if let Err(error) = lock_view(view).toggle_pause(session) {
        eprintln!("srtop: pause state not published: {error}");
    }
}

/// Dispatches one Refresh now click to the shared view (PX-015).
pub fn refresh_requested(view: &std::sync::Mutex<ProcessView>, _session: &Session) {
    lock_view(view).refresh_now();
}

/// Dispatches a `TEXT_EDIT` event on the interval field to the shared view
/// (PX-015).
pub fn interval_edited(
    view: &std::sync::Mutex<ProcessView>,
    session: &Session,
    event: &srui_protocol::Event,
) {
    let text = edited_text(session, event, INTERVAL_INPUT);
    if let Err(error) = lock_view(view).set_interval_input(session, &text) {
        eprintln!("srtop: interval field not published: {error}");
    }
}

/// Dispatches one Apply interval click to the shared view (PX-015). A refused
/// interval is explained on the sampling status, not here.
pub fn interval_requested(view: &std::sync::Mutex<ProcessView>, session: &Session) {
    if let Err(error) = lock_view(view).apply_interval(session) {
        eprintln!("srtop: interval not published: {error}");
    }
}

/// Dispatches a `TEXT_EDIT` event on the filter input to the shared view (PX-010).
pub fn filter_edited(
    view: &std::sync::Mutex<ProcessView>,
    session: &Session,
    event: &srui_protocol::Event,
) {
    let text = edited_text(session, event, FILTER_INPUT);
    if let Err(error) = lock_view(view).handle_client_text_edit(session, &text) {
        eprintln!("srtop: filter not published: {error}");
    }
}

/// Dispatches a `TEXT_EDIT` event on the search input to the shared view (PX-011).
pub fn search_edited(
    view: &std::sync::Mutex<ProcessView>,
    session: &Session,
    event: &srui_protocol::Event,
) {
    let text = edited_text(session, event, SEARCH_INPUT);
    // The match it promotes is the typing client's selection (PX-018).
    if let Err(error) = lock_view(view).set_search_from(session, &event.client_instance_id, &text) {
        eprintln!("srtop: search not published: {error}");
    }
}

/// Dispatches a `TEXT_EDIT` event on the PID field to the shared view (PX-011).
pub fn pid_edited(
    view: &std::sync::Mutex<ProcessView>,
    session: &Session,
    event: &srui_protocol::Event,
) {
    let text = edited_text(session, event, PID_INPUT);
    if let Err(error) = lock_view(view).set_pid_input(session, &text) {
        eprintln!("srtop: PID field not published: {error}");
    }
}

/// Dispatches one user-filter button click to the shared view (PX-012).
pub fn owner_chosen(view: &std::sync::Mutex<ProcessView>, session: &Session, choice: OwnerChoice) {
    if let Err(error) = lock_view(view).choose_owner(session, choice) {
        eprintln!("srtop: user filter not published: {error}");
    }
}

/// Dispatches a `TEXT_EDIT` event on the user field to the shared view (PX-012).
pub fn owner_edited(
    view: &std::sync::Mutex<ProcessView>,
    session: &Session,
    event: &srui_protocol::Event,
) {
    let text = edited_text(session, event, OWNER_INPUT);
    if let Err(error) = lock_view(view).set_owner_input(session, &text) {
        eprintln!("srtop: user filter not published: {error}");
    }
}

/// Dispatches a `TEXT_EDIT` event on the PID-set field to the shared view (PX-012).
pub fn pid_set_edited(
    view: &std::sync::Mutex<ProcessView>,
    session: &Session,
    event: &srui_protocol::Event,
) {
    let text = edited_text(session, event, PID_SET_INPUT);
    if let Err(error) = lock_view(view).set_pid_set(session, &text) {
        eprintln!("srtop: PID set not published: {error}");
    }
}

/// Dispatches one Next or Previous click to the shared view (PX-011). Only
/// the clicking client's identity is read from the event: the match it
/// promotes is that client's selection (PX-018).
pub fn search_stepped(
    view: &std::sync::Mutex<ProcessView>,
    session: &Session,
    event: &srui_protocol::Event,
    step: SearchStep,
) {
    let stepped = lock_view(view).step_search_from(session, &event.client_instance_id, step);
    if let Err(error) = stepped {
        eprintln!("srtop: search step not published: {error}");
    }
}

/// Dispatches one Go to PID click to the shared view (PX-011). Only the
/// clicking client's identity is read from the event (PX-018).
pub fn pid_requested(
    view: &std::sync::Mutex<ProcessView>,
    session: &Session,
    event: &srui_protocol::Event,
) {
    if let Err(error) = lock_view(view).go_to_pid_from(session, &event.client_instance_id) {
        eprintln!("srtop: PID search not published: {error}");
    }
}

/// This store's bound on the items of one model operation, clamped to the wire
/// default every conforming decoder enforces, and its bound on the operations of
/// one transaction (§26).
fn mutation_limits(session: &Session) -> (usize, usize) {
    session.with_store(|store| {
        let limits = store.limits();
        (
            // Both bounds apply: this store's own limit, and the wire default
            // every conforming decoder enforces (§26).
            limits
                .max_items_per_model_operation
                .clamp(1, DEFAULT_MAX_ITEMS_PER_MODEL_OPERATION),
            limits.max_transaction_operations.max(1),
        )
    })
}

/// Adds what `plan` publishes to `result`'s counts. The operations of a move
/// count once, as moved rows, never as a deletion and an insertion.
fn count(plan: &[Planned], result: &mut Refreshed) {
    for planned in plan {
        match &planned.effect {
            Effect::Status(_) => result.status_changed = true,
            Effect::Summary(_) => result.summary += 1,
            Effect::SortLabel { .. } => result.controls += 1,
            Effect::FilterStatus(_) | Effect::FilterValue(_) => result.controls += 1,
            Effect::SearchStatus(_)
            | Effect::SearchEnabled { .. }
            | Effect::SearchValue { .. }
            | Effect::Promote(..)
            | Effect::InspectorText { .. } => result.controls += 1,
            Effect::KeepVisible(..) | Effect::ClearKeepVisible(_) | Effect::Withdraw(_) => {
                result.controls += 1
            }
            Effect::TreeColumns(_)
            | Effect::Visibility { .. }
            | Effect::Shortcut { .. }
            | Effect::ViewMode(_)
            | Effect::ViewStatus(_) => result.controls += 1,
            Effect::DeleteRange { count, .. } => result.deleted += *count as usize,
            Effect::TreeInsert(_, rows) => result.inserted += rows.len(),
            Effect::TreeUpdate(_, rows) => result.updated += rows.len(),
            Effect::OwnerStatus(_) | Effect::OwnerEnabled { .. } | Effect::OwnerValue { .. } => {
                result.controls += 1
            }
            Effect::Columns(_) | Effect::ChooserLabel { .. } | Effect::ChooserEnabled { .. } => {
                result.controls += 1
            }
            Effect::SamplingStatus(_)
            | Effect::SamplingLabel(_)
            | Effect::SamplingEnabled(_)
            | Effect::SamplingValue(_) => result.controls += 1,
            Effect::FollowValue(_) | Effect::FollowStatus(_) => result.controls += 1,
            Effect::ThreadsStatus(_) | Effect::ThreadToggle { .. } => result.controls += 1,
            Effect::Delete(_) if planned.moving => {}
            Effect::Insert(_, rows) if planned.moving => result.moved += rows.len(),
            Effect::Delete(ids) => result.deleted += ids.len(),
            Effect::Insert(_, rows) => result.inserted += rows.len(),
            Effect::Update(rows) => result.updated += rows.len(),
        }
    }
}

/// Commits a plan as consecutive transactions of at most `max_ops` operations
/// and at most `max_bytes` of operation payload.
///
/// §26 bounds both the operations one transaction may carry and the size of the
/// frame it travels in, and a refresh of a large collection can exceed either
/// one without exceeding the other: tens of thousands of long names need only a
/// handful of operations and still will not fit in one frame. Exceeding the
/// frame bound is the worse failure, because the store commits the transaction
/// and the codec then refuses to write it, so the client is detached from a
/// server whose revision already moved on. Each transaction is still atomic, and
/// each is recorded into `published` as it commits, so a plan that stops
/// partway leaves this app's idea of what the client holds equal to what the
/// store holds. Returns how many transactions committed and the first failure.
///
/// A transaction never ends inside a unit of operations the planner joined
/// ([`Planned::joins_next`]): a move's deletion and its reinsertion always
/// travel together (PX-009), so no client ever holds a revision in which a
/// moved row is missing. The planner sizes each unit to fit one transaction on
/// its own.
fn commit(
    session: &Session,
    plan: &[Planned],
    max_ops: usize,
    max_bytes: usize,
    published: &mut Published<'_>,
) -> (usize, Result<(), SessionError>) {
    let (transactions, _, outcome) = commit_reaching(session, plan, max_ops, max_bytes, published);
    (transactions, outcome)
}

/// [`commit`], also returning how many of `plan`'s entries committed: every
/// entry before that index reached the store and every later one did not
/// (PX-018). A caller that changed server-side state ahead of a plan entry
/// keeps the change exactly when that entry committed.
fn commit_reaching(
    session: &Session,
    plan: &[Planned],
    max_ops: usize,
    max_bytes: usize,
    published: &mut Published<'_>,
) -> (usize, usize, Result<(), SessionError>) {
    let mut committed = 0;
    let mut start = 0;
    while start < plan.len() {
        let mut end = start;
        let mut bytes = 0;
        let mut ops = 0;
        // At least one operation always travels, so a plan always makes
        // progress; the planner keeps any single operation inside `max_bytes`.
        // A promotion records two operations (PX-011-G01), and so does a
        // `keep_visible` hint (PX-017-G01).
        while end < plan.len() {
            let cost = plan[end].bytes;
            let weight = plan[end].operations();
            if end > start && (ops + weight > max_ops || bytes + cost > max_bytes) {
                break;
            }
            bytes += cost;
            ops += weight;
            end += 1;
        }
        // Never end a transaction inside a joined unit: end before it instead,
        // unless it is the first thing in this transaction — then it travels
        // whole, which the planner's sizing makes a transaction it fits.
        if end < plan.len() && plan[end - 1].joins_next {
            let mut cut = end;
            while cut > start && plan[cut - 1].joins_next {
                cut -= 1;
            }
            debug_assert!(cut > start, "a joined unit larger than one transaction");
            if cut > start {
                end = cut;
            } else {
                while end < plan.len() && plan[end - 1].joins_next {
                    end += 1;
                }
            }
        }
        let chunk = &plan[start..end];
        match session.transaction(|ui| {
            // Test-only: a failed session commit, injected here so tests can
            // exercise what a plan that stops partway leaves behind. A
            // property of a node that cannot exist is refused and the whole
            // transaction rolls back, exactly as any refused operation does.
            #[cfg(test)]
            if commit_fault::fires() {
                ui.apply_op(&Operation::clear_property(NodeId::new(u64::MAX), TEXT))?;
            }
            for planned in chunk {
                match &planned.effect {
                    // Through the SDK, which sends the item with the promotion
                    // and refuses an item the model does not hold (PX-011-G01).
                    Effect::Promote(node, row) => ui.promote_selection(*node, row.item_id)?,
                    // Likewise for the hint (PX-017-G01).
                    Effect::KeepVisible(node, row) => ui.set_keep_visible(*node, row.item_id)?,
                    // A clear that settlement already made earlier in this
                    // plan records nothing.
                    Effect::ClearKeepVisible(node) => {
                        ui.clear_keep_visible(*node)?;
                    }
                    Effect::Withdraw(node) => {
                        ui.clear_promoted_selection(*node)?;
                    }
                    _ => ui.apply_op(&planned.op)?,
                }
            }
            Ok(())
        }) {
            Ok(()) => {
                record_committed(chunk, published);
                // Settlement may have cleared the promotion or the hint in this
                // commit, when its row left the model: the store is the record
                // (PX-011, PX-017). They are the shown collection's (PX-018).
                let shown = published.mode.node();
                let (promoted, keep_visible) = session.with_store(|store| {
                    store.get_node(shown).map_or((None, None), |node| {
                        (node.promoted_selection(), node.keep_visible())
                    })
                });
                *published.promoted = promoted;
                *published.keep_visible = keep_visible;
                committed += 1;
            }
            Err(error) => return (committed, start, Err(error)),
        }
        start = end;
    }
    (committed, start, Ok(()))
}

/// Test-only fault injection into [`commit_reaching`] (PX-018), on the
/// pattern of `procfs`'s: an armed counter fails one later transaction of
/// this thread, through a refused operation, until the guard is dropped.
/// Every caller that commits a plan does so on its own thread, so a
/// thread-local reaches exactly the plan one test publishes. Compiled only
/// under `cfg(test)`: no release or integration-test build contains it.
#[cfg(test)]
mod commit_fault {
    use std::cell::Cell;

    thread_local! {
        static FAIL_IN: Cell<Option<usize>> = const { Cell::new(None) };
    }

    /// Disarms the fault when dropped, even when the test panics.
    #[must_use = "the fault is disarmed as soon as this guard is dropped"]
    pub(super) struct Armed;

    impl Drop for Armed {
        fn drop(&mut self) {
            FAIL_IN.with(|slot| slot.set(None));
        }
    }

    /// Fails the transaction attempted `after` attempts from now: 0 fails the
    /// next one.
    pub(super) fn arm(after: usize) -> Armed {
        FAIL_IN.with(|slot| slot.set(Some(after)));
        Armed
    }

    /// Whether this transaction attempt must fail.
    pub(super) fn fires() -> bool {
        FAIL_IN.with(|slot| match slot.get() {
            Some(0) => {
                slot.set(None);
                true
            }
            Some(left) => {
                slot.set(Some(left - 1));
                false
            }
            None => false,
        })
    }
}

/// The published status: what the source says it read, what the scan could not
/// see, how many rows on screen the scan did not confirm, and how many rows
/// this app declined to publish because they would not fit one snapshot frame.
///
/// Every clause is derived from counts and from fixed wording. No record's own
/// text ever reaches the status line, so a process cannot write into it.
///
/// The truncation clause names its own cause and is deliberately distinct from
/// every other shortfall: rows beyond the publishable size were read perfectly
/// well, so calling them unreadable would report a failure that never happened,
/// and they are not the entries the collector's own record bound left unread
/// (`beyond the record limit`) either — this bound belongs to the wire, not to
/// the scan.
///
/// The eviction clause is distinct again, and from the truncation clause too: an
/// evicted row is one this app *had* published and stopped carrying because no
/// scan has confirmed it since and the collection may not grow without bound. It
/// is not a process that ended — those leave no clause, because their rows are
/// simply deleted — not a record that could not be read, not an entry beyond the
/// collector's record bound, and not a row of this snapshot that did not fit.
pub fn refresh_status(
    source_status: &str,
    snapshot: &ProcessSnapshot,
    counts: &RefreshCounts,
) -> String {
    refresh_status_from(source_status, &ScanReport::of(snapshot), counts)
}

/// What one refresh adds to the status its scan already reported.
///
/// Grouped so the reserve [`refresh`](ProcessView::refresh) charges the frame
/// can be measured against this builder with every count at its widest, rather
/// than against the counts a test can arrange.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefreshCounts {
    /// Rows the scan did not confirm and that were kept.
    pub retained: usize,
    /// Why this snapshot was refused whole, when it was.
    pub rejected: Option<String>,
    /// Unconfirmed rows dropped to stay inside the published-row ceiling.
    pub evicted: usize,
    /// Rows left unpublished because the collection would not fit.
    pub truncated: usize,
}

/// The same status, from a scan's report rather than from the scan itself.
pub(crate) fn refresh_status_from(
    source_status: &str,
    scan: &ScanReport,
    counts: &RefreshCounts,
) -> String {
    let mut status = published_status_from(source_status, scan);
    if let Some(reason) = &counts.rejected {
        let _ = write!(
            status,
            " · snapshot rejected: {}",
            bounded(reason, MAX_REJECTION_BYTES)
        );
    }
    if counts.retained > 0 {
        let _ = write!(
            status,
            " · {} {} retained from an earlier scan",
            counts.retained,
            rows_word(counts.retained)
        );
    }
    if counts.evicted > 0 {
        let _ = write!(
            status,
            " · {} unconfirmed {} dropped to stay inside the published row limit",
            counts.evicted,
            rows_word(counts.evicted)
        );
    }
    if counts.truncated > 0 {
        let _ = write!(
            status,
            " · {} {} beyond the publishable size limit",
            counts.truncated,
            rows_word(counts.truncated)
        );
    }
    status
}

fn rows_word(count: usize) -> &'static str {
    if count == 1 {
        "row"
    } else {
        "rows"
    }
}

/// Drops the trailing rows this app cannot publish — those a catch-up snapshot
/// of the resulting model could not carry, and those beyond `limit` — and
/// returns how many were dropped (§18, §26).
///
/// `sessiond` exports a snapshot as one transaction holding every cached range
/// of every model, and checks only its operation count, so a model larger than
/// one frame is one no client can be sent — not on attach and not on resync.
/// Every quantity charged here is the same upper bound the refresh planner
/// charges, computed on the values actually being published, plus the
/// `MODEL_RESET_RANGE` operation the snapshot opens per
/// [`DEFAULT_MAX_ITEMS_PER_MODEL_OPERATION`] rows and a fixed allowance for the
/// shell that travels with them.
///
/// The frame bounds bytes and `limit` bounds rows, and neither implies the
/// other: tens of thousands of long names overflow a frame, while short rows fit
/// one frame in numbers a model may not cache. Both are applied here, where the
/// rows are chosen.
///
/// The status is charged [`STATUS_RESERVE_BYTES`] rather than its own length,
/// the system summary [`SUMMARY_RESERVE_BYTES`] rather than its own (PX-007), and
/// the command filter controls [`FILTER_RESERVE_BYTES`] rather than their own (PX-010), and
/// the search controls [`SEARCH_RESERVE_BYTES`] rather than their own (PX-011),
/// and the user filter [`OWNER_RESERVE_BYTES`] rather than its own (PX-012),
/// and the inspector [`INSPECTOR_RESERVE_BYTES`] rather than its own (PX-013),
/// and its command-line view [`COMMAND_RESERVE_BYTES`] rather than its own (PX-014),
/// and the sampling controls [`SAMPLING_RESERVE_BYTES`] rather than their own (PX-015),
/// and the follow controls [`FOLLOW_RESERVE_BYTES`] rather than their own (PX-017), so
/// the rows that fit are a function of the rows alone. A status or summary that
/// grew because a scan or a collector failed therefore cannot shrink the row
/// budget and delete a row no scan said had ended.
///
/// The rows kept are the *leading* ones, in the order the collection is
/// published in — initially ascending PID, then whatever the active sort says
/// — so the published window is a deterministic function of the snapshot and
/// the active order alone. An unchanged host keeps exactly the rows it kept
/// last tick: no row churns in and out, and an identical tick still publishes
/// nothing.
fn bound_to_publishable_collection(rows: &mut Vec<Row>, limit: usize) -> usize {
    let budget = MAX_TRANSACTION_PAYLOAD_BYTES.saturating_sub(
        SNAPSHOT_SHELL_BYTES
            + STATUS_RESERVE_BYTES
            + SUMMARY_RESERVE_BYTES
            + FILTER_RESERVE_BYTES
            + SEARCH_RESERVE_BYTES
            + OWNER_RESERVE_BYTES
            + INSPECTOR_RESERVE_BYTES
            + COMMAND_RESERVE_BYTES
            + SAMPLING_RESERVE_BYTES
            + FOLLOW_RESERVE_BYTES
            + VIEW_RESERVE_BYTES
            + THREADS_RESERVE_BYTES,
    );
    let mut spent = 0usize;
    let mut kept = 0usize;
    for row in rows.iter() {
        if kept == limit {
            break;
        }
        // Reserve every catalogued cell, including hidden columns. Otherwise
        // hiding a wide field admits extra rows that cannot fit when restored,
        // and the following refresh evicts them despite an unchanged source.
        // A tree row also carries its parent cell and its two item
        // properties (PX-018).
        let mut cost = MODEL_ITEM_FRAMING_BYTES
            + VALUE_FRAMING_BYTES
            + row.cells.iter().map(value_wire_bytes).sum::<usize>()
            + row.tree.as_ref().map_or(0, |place| {
                value_wire_bytes(&place.parent_cell()) + TREE_ITEM_PROPERTIES_BYTES
            });
        if kept.is_multiple_of(DEFAULT_MAX_ITEMS_PER_MODEL_OPERATION) {
            // This row opens another operation of the exported snapshot.
            cost += SNAPSHOT_RANGE_BYTES;
        }
        if spent + cost > budget {
            break;
        }
        spent += cost;
        kept += 1;
    }
    let dropped = rows.len() - kept;
    rows.truncate(kept);
    dropped
}

/// Drops the unconfirmed rows a collection bounded to `limit` has no room for,
/// and returns how many were dropped (PX-004 review round 7).
///
/// Retention keeps a row whose absence a scan could not account for, and a scan
/// that cannot enumerate its uncertainty at all — a ledger that overflowed, an
/// entry it could not examine — accounts for nothing, so on such a host every
/// absent row would be kept for ever and ordinary churn would grow the
/// collection until the model's own §26 item limit refused the next insert. This
/// is the bound that cannot happen under: it is applied to every refresh, under
/// every kind of retention, and depends on no property of the scan.
///
/// Which rows go is a function of the published order alone: the rows kept are
/// the leading unconfirmed ones, the same deterministic window the snapshot
/// ceiling keeps, so two runs of the same ticks publish the same rows. That
/// window is also the useful one, because [`retain_unconfirmed`] anchors a row
/// the current scan did not confirm behind the rows it did: a row drifts towards
/// the end for as long as it goes unconfirmed, so the trailing rows evicted here
/// are the stalest ones. A row a scan did confirm is never evicted, an evicted
/// row is never re-admitted from history — it has left the published collection
/// — and nothing here depends on iteration order or on a clock.
fn evict_unconfirmed_beyond(
    rows: &mut Vec<Row>,
    confirmed: &HashSet<ItemId>,
    limit: usize,
) -> HashSet<ItemId> {
    if rows.len() <= limit {
        return HashSet::new();
    }
    let mut excess = rows.len() - limit;
    let mut evicted: HashSet<ItemId> = HashSet::new();
    for row in rows.iter().rev() {
        if excess == 0 {
            break;
        }
        if !confirmed.contains(&row.item_id) {
            evicted.insert(row.item_id);
            excess -= 1;
        }
    }
    if !evicted.is_empty() {
        rows.retain(|row| !evicted.contains(&row.item_id));
    }
    evicted
}

/// [`retain_unconfirmed_by`] under a record [`Retention`] alone.
#[cfg(test)]
fn retain_unconfirmed(published: &[Row], confirmed: Vec<Row>, retention: &Retention) -> Vec<Row> {
    retain_unconfirmed_by(published, confirmed, |row| retention.keeps(&row.key))
}

/// Keeps rows the scan did not confirm and could not account for, in the
/// position they already hold relative to the rows it did confirm.
///
/// `keeps` decides which unconfirmed rows those are: a scan that named the
/// records it skipped keeps only those, so an unrelated process that really
/// ended is still deleted while one denied record is unreadable. Only a scan
/// whose uncertainty cannot be enumerated at all keeps every absent row. A
/// thread row is also kept beside the process instance this scan confirmed
/// when the scan could not read that process's threads (PX-019).
///
/// A retained row is anchored to the confirmed row that followed it, so a
/// scan that misses a record for one tick does not move that row to the end of
/// the table and back again on the next.
fn retain_unconfirmed_by(
    published: &[Row],
    confirmed: Vec<Row>,
    keeps: impl Fn(&Row) -> bool,
) -> Vec<Row> {
    let position: HashMap<ItemId, usize> = confirmed
        .iter()
        .enumerate()
        .map(|(index, row)| (row.item_id, index))
        .collect();
    let uncertain = |row: &Row| !position.contains_key(&row.item_id) && keeps(row);
    if !published.iter().any(uncertain) {
        return confirmed;
    }
    let mut before: HashMap<usize, Vec<Row>> = HashMap::new();
    let mut tail: Vec<Row> = Vec::new();
    let mut anchor: Option<usize> = None;
    for row in published.iter().rev() {
        match position.get(&row.item_id) {
            Some(&index) => anchor = Some(index),
            // An absent row this scan accounted for really ended: it is left out
            // of the target, which deletes it.
            None if !keeps(row) => {}
            None => match anchor {
                Some(index) => before.entry(index).or_default().push(row.clone()),
                None => tail.push(row.clone()),
            },
        }
    }
    let mut merged = Vec::with_capacity(confirmed.len() + before.len() + tail.len());
    for (index, row) in confirmed.into_iter().enumerate() {
        if let Some(mut unconfirmed) = before.remove(&index) {
            unconfirmed.reverse();
            merged.append(&mut unconfirmed);
        }
        merged.push(row);
    }
    tail.reverse();
    merged.append(&mut tail);
    merged
}
/// One planned mutation: the wire operation, the effect it has on the published
/// rows — derived from one description so the two cannot disagree — and an upper
/// bound on the bytes the operation adds to an encoded transaction.
struct Planned {
    op: Operation,
    effect: Effect,
    bytes: usize,
    /// The next operation belongs to the same unit, which [`commit`] never
    /// splits across transactions: a move's deletion and reinsertion (PX-009).
    joins_next: bool,
    /// This operation is part of a move ([`plan_moves`]), counted as moved
    /// rows rather than as a deletion or an insertion.
    moving: bool,
}

enum Effect {
    Status(String),
    /// One summary property set, or cleared (PX-007).
    Summary(summary::Change),
    /// One sort button's label set, and — when the label names the column that
    /// just became the active order — the order it activates (PX-009). The
    /// order is recorded as this effect's transaction commits, so a request
    /// that stops partway leaves the view holding exactly the order its labels
    /// announced.
    SortLabel {
        node: NodeId,
        label: String,
        activates: Option<SortOrder>,
    },
    /// The command filter status text set (PX-010).
    FilterStatus(String),
    /// The command filter text input value set (PX-010).
    FilterValue(String),
    /// The table's headings set to a layout's, which becomes the published
    /// layout as this effect commits (PX-068).
    Columns(ColumnLayout),
    /// One column chooser button's label set (PX-068).
    ChooserLabel {
        node: NodeId,
        label: String,
    },
    /// One column chooser button enabled or disabled (PX-068).
    ChooserEnabled {
        node: NodeId,
        enabled: bool,
    },
    /// The search status text set (PX-011).
    SearchStatus(String),
    /// One search button enabled or disabled (PX-011).
    SearchEnabled {
        node: NodeId,
        enabled: bool,
    },
    /// A search text field's value set to what the server keeps (PX-011).
    SearchValue {
        node: NodeId,
        value: String,
    },
    /// The collection's semantic selection promoted to this row's item ID
    /// (PX-011, PX-011-G01): committed through
    /// `UiTransaction::promote_selection`, which records a one-item
    /// `MODEL_RESET_RANGE` carrying the row and `SET_PROPERTY(value = ItemId)`.
    /// The node is the table or the tree, whichever is shown (PX-018).
    Promote(NodeId, Box<Row>),
    /// The collection's `keep_visible` hint set to this row's item ID (PX-017,
    /// PX-017-G01): committed through `UiTransaction::set_keep_visible`, which
    /// records a one-item `MODEL_RESET_RANGE` carrying the row and
    /// `SET_PROPERTY(keep_visible = ItemId)`. It never touches the selection.
    KeepVisible(NodeId, Box<Row>),
    /// The collection's `keep_visible` hint cleared (PX-017).
    ClearKeepVisible(NodeId),
    /// The collection's promoted selection withdrawn (PX-017, PX-011-G01): a
    /// `CLEAR_PROPERTY(value)`, which keeps a client's later local selection.
    Withdraw(NodeId),
    /// The tree's headings set to a layout's, with the parent column last
    /// (PX-018). The layout itself is recorded by [`Effect::Columns`].
    TreeColumns(ColumnLayout),
    /// A collection shown or collapsed out of the layout (PX-018).
    Visibility {
        node: NodeId,
        shown: bool,
    },
    /// The list-focus shortcut set on, or cleared from, a collection (PX-018):
    /// it moves with the shown one, so it is never bound twice.
    Shortcut {
        node: NodeId,
        set: bool,
    },
    /// The view switch's value set, and the view it shows recorded (PX-018).
    ViewMode(ViewMode),
    /// The view status text set (PX-018).
    ViewStatus(String),
    /// One index range of a model deleted (PX-018): the tree's edits, which
    /// a sparse replica can apply without caching the deleted items, and the
    /// emptying of the collection a view switch hides.
    DeleteRange {
        model: ModelId,
        index: u64,
        count: u64,
    },
    /// Tree rows inserted at a preorder index (PX-018).
    TreeInsert(u64, Vec<Row>),
    /// Tree rows replaced in place from a preorder index (PX-018).
    TreeUpdate(u64, Vec<Row>),
    /// The user-filter status text set (PX-012).
    OwnerStatus(String),
    /// One user-filter button enabled or disabled (PX-012).
    OwnerEnabled {
        node: NodeId,
        enabled: bool,
    },
    /// The user or PID-set field's value set to what the server keeps (PX-012).
    OwnerValue {
        node: NodeId,
        value: String,
    },
    /// One inspector line's text set (PX-013).
    InspectorText {
        node: NodeId,
        text: String,
    },
    /// The sampling status text set (PX-015).
    SamplingStatus(String),
    /// The Pause view/Resume view button's label set (PX-015).
    SamplingLabel(String),
    /// The Refresh now button enabled or disabled (PX-015).
    SamplingEnabled(bool),
    /// The interval field's value set to what the server keeps (PX-015).
    SamplingValue(String),
    /// The follow toggle's value set to what the server holds (PX-017).
    FollowValue(bool),
    /// The follow status text set (PX-017).
    FollowStatus(String),
    /// The threads status text set (PX-019).
    ThreadsStatus(String),
    /// One thread checkbox's value set to what the server holds (PX-019).
    ThreadToggle {
        node: NodeId,
        on: bool,
    },
    Delete(Vec<ItemId>),
    Insert(u64, Vec<Row>),
    Update(Vec<Row>),
}

/// What this view has published, as one committed transaction updates it.
struct Published<'a> {
    rows: &'a mut Vec<Row>,
    status: &'a mut String,
    summary: &'a mut summary::Properties,
    /// The order the labels have announced (PX-009).
    sort: &'a mut Option<SortOrder>,
    /// The filter status text announced (PX-010).
    filter_status: &'a mut String,
    /// The column layout the headings announce (PX-068).
    columns: &'a mut ColumnLayout,
    /// The search controls as announced (PX-011).
    search: &'a mut SearchView,
    /// The table's promoted selection as the store holds it (PX-011).
    promoted: &'a mut Option<ItemId>,
    /// The table's `keep_visible` hint as the store holds it (PX-017).
    keep_visible: &'a mut Option<ItemId>,
    /// The user-filter controls as announced (PX-012).
    owner: &'a mut OwnerView,
    /// The inspector's lines as announced (PX-013).
    inspector: &'a mut InspectorView,
    /// The sampling controls as announced (PX-015).
    sampling: &'a mut SamplingView,
    /// The follow controls as announced (PX-017).
    follow: &'a mut FollowView,
    /// The view the switch announced (PX-018).
    mode: &'a mut ViewMode,
    /// The view status as announced (PX-018).
    view_status: &'a mut String,
    /// The thread controls as announced (PX-019).
    threads: &'a mut ThreadsView,
}

/// The planned operations that turn the `published` summary into `target`:
/// one per property a client would see change, and nothing for the rest.
fn summary_effects(
    published: &summary::Properties,
    target: &summary::Properties,
) -> impl Iterator<Item = Planned> {
    summary::changes(published, target)
        .into_iter()
        .map(|change| Planned::new(Effect::Summary(change)))
}

impl Planned {
    fn new(effect: Effect) -> Self {
        let op = match &effect {
            Effect::Status(text) => {
                Operation::set_property(STATUS, TEXT, Value::String(text.clone()))
            }
            Effect::Summary(change) => match &change.value {
                Some(value) => Operation::set_property(change.node, change.property, value.clone()),
                None => Operation::clear_property(change.node, change.property),
            },
            Effect::SortLabel { node, label, .. } => {
                Operation::set_property(*node, LABEL, Value::String(label.clone()))
            }
            Effect::FilterStatus(text) => {
                Operation::set_property(FILTER_STATUS, TEXT, Value::String(text.clone()))
            }
            Effect::FilterValue(val) => {
                Operation::set_property(FILTER_INPUT, VALUE, Value::String(val.clone()))
            }
            Effect::Columns(layout) => {
                Operation::set_property(TABLE, COLUMNS, Value::List(column_headings(layout)))
            }
            Effect::ChooserLabel { node, label } => {
                Operation::set_property(*node, LABEL, Value::String(label.clone()))
            }
            Effect::ChooserEnabled { node, enabled } => {
                Operation::set_property(*node, ENABLED, Value::Bool(*enabled))
            }
            Effect::SearchStatus(text) => {
                Operation::set_property(SEARCH_STATUS, TEXT, Value::String(text.clone()))
            }
            Effect::SearchEnabled { node, enabled } => {
                Operation::set_property(*node, ENABLED, Value::Bool(*enabled))
            }
            Effect::SearchValue { node, value } => {
                Operation::set_property(*node, VALUE, Value::String(value.clone()))
            }
            // What the promotion sets; `commit` records it through the SDK.
            Effect::Promote(node, row) => {
                Operation::set_property(*node, VALUE, Value::ItemId(row.item_id))
            }
            // What the hint sets and the clears clear; `commit` records them
            // through the SDK.
            Effect::KeepVisible(node, row) => {
                Operation::set_property(*node, KEEP_VISIBLE, Value::ItemId(row.item_id))
            }
            Effect::ClearKeepVisible(node) => Operation::clear_property(*node, KEEP_VISIBLE),
            Effect::Withdraw(node) => Operation::clear_property(*node, VALUE),
            Effect::TreeColumns(layout) => {
                Operation::set_property(TREE, COLUMNS, Value::List(tree_headings(layout)))
            }
            Effect::Visibility { node, shown } => Tree::op_set_visibility_for(
                *node,
                if *shown {
                    Visibility::Visible
                } else {
                    Visibility::Collapsed
                },
            ),
            Effect::Shortcut { node, set: true } => {
                Operation::set_property(*node, PropertyRef::SHORTCUT, LIST_SHORTCUT)
            }
            Effect::Shortcut { node, set: false } => {
                Operation::clear_property(*node, PropertyRef::SHORTCUT)
            }
            Effect::ViewMode(mode) => {
                Operation::set_property(VIEW_TOGGLE, VALUE, *mode == ViewMode::Tree)
            }
            Effect::ViewStatus(text) => {
                Operation::set_property(VIEW_STATUS, TEXT, Value::String(text.clone()))
            }
            Effect::DeleteRange {
                model,
                index,
                count,
            } => Operation::model_delete_range(*model, *index, *count),
            Effect::TreeInsert(index, rows) => {
                Operation::model_insert(TREE_MODEL, *index, rows.iter().map(Row::to_model_item))
            }
            Effect::TreeUpdate(index, rows) => Operation::model_update(
                TREE_MODEL,
                Some(*index),
                rows.iter().map(Row::to_model_item),
            ),
            Effect::OwnerStatus(text) => {
                Operation::set_property(OWNER_STATUS, TEXT, Value::String(text.clone()))
            }
            Effect::OwnerEnabled { node, enabled } => {
                Operation::set_property(*node, ENABLED, Value::Bool(*enabled))
            }
            Effect::OwnerValue { node, value } => {
                Operation::set_property(*node, VALUE, Value::String(value.clone()))
            }
            Effect::InspectorText { node, text } => {
                Operation::set_property(*node, TEXT, Value::String(text.clone()))
            }
            Effect::SamplingStatus(text) => {
                Operation::set_property(SAMPLING_STATUS, TEXT, Value::String(text.clone()))
            }
            Effect::SamplingLabel(label) => {
                Operation::set_property(SAMPLING_PAUSE, LABEL, Value::String(label.clone()))
            }
            Effect::SamplingEnabled(enabled) => {
                Operation::set_property(SAMPLING_REFRESH, ENABLED, Value::Bool(*enabled))
            }
            Effect::SamplingValue(value) => {
                Operation::set_property(INTERVAL_INPUT, VALUE, Value::String(value.clone()))
            }
            Effect::FollowValue(on) => {
                Operation::set_property(FOLLOW_TOGGLE, VALUE, Value::Bool(*on))
            }
            Effect::FollowStatus(text) => {
                Operation::set_property(FOLLOW_STATUS, TEXT, Value::String(text.clone()))
            }
            Effect::ThreadsStatus(text) => {
                Operation::set_property(THREADS_STATUS, TEXT, Value::String(text.clone()))
            }
            Effect::ThreadToggle { node, on } => {
                Operation::set_property(*node, VALUE, Value::Bool(*on))
            }
            Effect::Delete(ids) => Operation::model_delete_items(MODEL, ids.iter().copied()),
            Effect::Insert(index, rows) => {
                Operation::model_insert(MODEL, *index, rows.iter().map(Row::to_model_item))
            }
            Effect::Update(rows) => {
                Operation::model_update(MODEL, None, rows.iter().map(Row::to_model_item))
            }
        };
        let bytes = OPERATION_FRAMING_BYTES + OPERATION_HEADER_BYTES + effect.payload_bytes();
        Self {
            op,
            effect,
            bytes,
            joins_next: false,
            moving: false,
        }
    }

    /// How many operations this entry records: two for a promotion or a
    /// `keep_visible` hint (its item's one-item `MODEL_RESET_RANGE` and the
    /// `SET_PROPERTY`), one otherwise.
    fn operations(&self) -> usize {
        if matches!(self.effect, Effect::Promote(..) | Effect::KeepVisible(..)) {
            2
        } else {
            1
        }
    }

    /// The same operation as part of a move.
    fn in_move(effect: Effect) -> Self {
        Self {
            moving: true,
            ..Self::new(effect)
        }
    }
}

impl Effect {
    /// An upper bound on the bytes this effect's operation body carries beyond
    /// its fixed header.
    fn payload_bytes(&self) -> usize {
        match self {
            Self::Status(text) => VALUE_FRAMING_BYTES + text.len(),
            // The node and property are in the fixed header; a clear carries no
            // value at all.
            Self::Summary(change) => change.value.as_ref().map_or(0, value_wire_bytes),
            Self::SortLabel { label, .. } => VALUE_FRAMING_BYTES + label.len(),
            Self::FilterStatus(text) => VALUE_FRAMING_BYTES + text.len(),
            Self::FilterValue(val) => VALUE_FRAMING_BYTES + val.len(),
            Self::Columns(layout) => {
                VALUE_FRAMING_BYTES
                    + column_headings(layout)
                        .iter()
                        .map(value_wire_bytes)
                        .sum::<usize>()
            }
            Self::ChooserLabel { label, .. } => VALUE_FRAMING_BYTES + label.len(),
            Self::ChooserEnabled { .. }
            | Self::SearchEnabled { .. }
            | Self::OwnerEnabled { .. } => VALUE_FRAMING_BYTES + SCALAR_VALUE_BYTES,
            Self::OwnerStatus(text) => VALUE_FRAMING_BYTES + text.len(),
            Self::OwnerValue { value, .. } => VALUE_FRAMING_BYTES + value.len(),
            Self::InspectorText { text, .. } => VALUE_FRAMING_BYTES + text.len(),
            Self::SamplingStatus(text) | Self::SamplingLabel(text) | Self::SamplingValue(text) => {
                VALUE_FRAMING_BYTES + text.len()
            }
            Self::SamplingEnabled(_) | Self::FollowValue(_) => {
                VALUE_FRAMING_BYTES + SCALAR_VALUE_BYTES
            }
            Self::FollowStatus(text) | Self::ThreadsStatus(text) => {
                VALUE_FRAMING_BYTES + text.len()
            }
            Self::ThreadToggle { .. } => VALUE_FRAMING_BYTES + SCALAR_VALUE_BYTES,
            Self::SearchStatus(text) => VALUE_FRAMING_BYTES + text.len(),
            Self::SearchValue { value, .. } => VALUE_FRAMING_BYTES + value.len(),
            // The `SET_PROPERTY` of the item ID, and a whole second operation:
            // the one-item `MODEL_RESET_RANGE` carrying the row.
            Self::Promote(_, row) | Self::KeepVisible(_, row) => {
                VALUE_FRAMING_BYTES
                    + SCALAR_VALUE_BYTES
                    + OPERATION_FRAMING_BYTES
                    + OPERATION_HEADER_BYTES
                    + row_wire_bytes(row)
            }
            // A clear carries no value.
            Self::ClearKeepVisible(_) | Self::Withdraw(_) => 0,
            Self::TreeColumns(layout) => {
                VALUE_FRAMING_BYTES
                    + tree_headings(layout)
                        .iter()
                        .map(value_wire_bytes)
                        .sum::<usize>()
            }
            Self::Visibility { .. } | Self::ViewMode(_) => VALUE_FRAMING_BYTES + SCALAR_VALUE_BYTES,
            Self::Shortcut { .. } => VALUE_FRAMING_BYTES + LIST_SHORTCUT.len(),
            Self::ViewStatus(text) => VALUE_FRAMING_BYTES + text.len(),
            // An index range is the header's two integers.
            Self::DeleteRange { .. } => 0,
            Self::TreeInsert(_, rows) | Self::TreeUpdate(_, rows) => {
                rows.iter().map(row_wire_bytes).sum()
            }
            Self::Delete(ids) => ids.len() * ITEM_ID_BYTES,
            Self::Insert(_, rows) | Self::Update(rows) => rows.iter().map(row_wire_bytes).sum(),
        }
    }

    /// Applies this effect to the view's record of what the client holds. Called
    /// only after the transaction carrying the matching operation committed.
    fn record(&self, published: &mut Published<'_>) {
        let Published {
            rows,
            status,
            summary,
            sort,
            filter_status,
            columns,
            search,
            promoted,
            keep_visible,
            owner,
            inspector,
            sampling,
            follow,
            mode,
            view_status,
            threads,
        } = published;
        match self {
            Self::ViewMode(shown) => **mode = *shown,
            Self::ViewStatus(text) => view_status.clone_from(text),
            Self::TreeColumns(_) | Self::Visibility { .. } | Self::Shortcut { .. } => {}
            Self::DeleteRange { index, count, .. } => {
                let start = (*index as usize).min(rows.len());
                let end = start.saturating_add(*count as usize).min(rows.len());
                rows.drain(start..end);
            }
            Self::TreeInsert(index, inserted) => {
                let at = (*index as usize).min(rows.len());
                rows.splice(at..at, inserted.iter().cloned());
            }
            Self::TreeUpdate(index, updated) => {
                let start = (*index as usize).min(rows.len());
                for (slot, row) in rows[start..].iter_mut().zip(updated) {
                    *slot = row.clone();
                }
            }
            Self::InspectorText { node, text } => {
                let line = if *node == INSPECTOR_STATUS {
                    &mut inspector.status
                } else if *node == INSPECTOR_PROCESS {
                    &mut inspector.process
                } else if *node == INSPECTOR_DETAILS {
                    &mut inspector.details
                } else if *node == INSPECTOR_COMMAND {
                    &mut inspector.command
                } else {
                    &mut inspector.metrics
                };
                line.clone_from(text);
            }
            Self::OwnerStatus(text) => owner.status.clone_from(text),
            Self::OwnerEnabled { node, enabled } => {
                if *node == OWNER_MINE {
                    owner.mine = *enabled;
                } else if *node == OWNER_ALL {
                    owner.all = *enabled;
                }
            }
            Self::OwnerValue { .. } | Self::SamplingValue(_) => {}
            Self::SamplingStatus(text) => sampling.status.clone_from(text),
            Self::SamplingLabel(label) => sampling.pause_label.clone_from(label),
            Self::SamplingEnabled(enabled) => sampling.refresh_enabled = *enabled,
            Self::FollowValue(checked) => follow.checked = *checked,
            Self::FollowStatus(text) => follow.status.clone_from(text),
            Self::ThreadsStatus(text) => threads.status.clone_from(text),
            Self::ThreadToggle { node, on } => {
                if *node == USER_THREADS_TOGGLE {
                    threads.user = *on;
                } else {
                    threads.kernel = *on;
                }
            }
            Self::Promote(_, row) => **promoted = Some(row.item_id),
            Self::KeepVisible(_, row) => **keep_visible = Some(row.item_id),
            Self::ClearKeepVisible(_) => **keep_visible = None,
            Self::Withdraw(_) => **promoted = None,
            Self::Status(text) => status.clone_from(text),
            Self::FilterStatus(text) => filter_status.clone_from(text),
            Self::FilterValue(_)
            | Self::ChooserLabel { .. }
            | Self::ChooserEnabled { .. }
            | Self::SearchValue { .. } => {}
            Self::SearchStatus(text) => search.status.clone_from(text),
            Self::SearchEnabled { node, enabled } => {
                if *node == SEARCH_PREVIOUS {
                    search.previous = *enabled;
                } else if *node == SEARCH_NEXT {
                    search.next = *enabled;
                } else if *node == PID_GO {
                    search.go = *enabled;
                }
            }
            Self::Columns(layout) => columns.clone_from(layout),
            Self::Summary(change) => {
                let key = (change.node, change.property);
                match &change.value {
                    Some(value) => {
                        summary.insert(key, value.clone());
                    }
                    None => {
                        summary.remove(&key);
                    }
                }
            }
            Self::SortLabel { activates, .. } => {
                if let Some(order) = activates {
                    **sort = Some(*order);
                }
            }
            Self::Delete(ids) => {
                let gone: HashSet<ItemId> = ids.iter().copied().collect();
                rows.retain(|row| !gone.contains(&row.item_id));
            }
            Self::Insert(index, inserted) => {
                let at = (*index as usize).min(rows.len());
                rows.splice(at..at, inserted.iter().cloned());
            }
            Self::Update(updated) => {
                let values: HashMap<ItemId, &Row> =
                    updated.iter().map(|row| (row.item_id, row)).collect();
                for row in rows.iter_mut() {
                    if let Some(update) = values.get(&row.item_id) {
                        row.value = update.value.clone();
                        row.adopt_sample(update);
                    }
                }
            }
        }
    }
}

/// Records everything one committed transaction did to the published rows.
///
/// Consecutive insertions are merged in a single pass: a refresh of a large
/// collection can carry thousands of them, and splicing each one separately
/// would cost a full copy of the row list per insertion.
fn record_committed(chunk: &[Planned], published: &mut Published<'_>) {
    let mut index = 0;
    while index < chunk.len() {
        if matches!(chunk[index].effect, Effect::Insert(..)) {
            // Merge only a run whose indices ascend past the rows inserted
            // before them, which is what `insert_run` assumes. A move's
            // reinsertion can be followed by an arrival at a lower index
            // (PX-009); that one opens a new run, applied after the first,
            // exactly as the store applied it.
            let start = index;
            let mut floor = 0u64;
            while let Some(Effect::Insert(at, rows)) =
                chunk.get(index).map(|planned| &planned.effect)
            {
                if index > start && *at < floor {
                    break;
                }
                floor = *at + rows.len() as u64;
                index += 1;
            }
            insert_run(published.rows, &chunk[start..index]);
            continue;
        }
        chunk[index].effect.record(published);
        index += 1;
    }
}

/// Applies a run of insertions whose indices ascend, each already counting the
/// rows inserted before it.
fn insert_run(rows: &mut Vec<Row>, inserts: &[Planned]) {
    let added: usize = inserts
        .iter()
        .map(|planned| match &planned.effect {
            Effect::Insert(_, inserted) => inserted.len(),
            _ => 0,
        })
        .sum();
    let mut remaining = std::mem::take(rows).into_iter();
    let mut merged = Vec::with_capacity(remaining.len() + added);
    for planned in inserts {
        let Effect::Insert(index, inserted) = &planned.effect else {
            continue;
        };
        while merged.len() < *index as usize {
            match remaining.next() {
                Some(row) => merged.push(row),
                None => break,
            }
        }
        merged.extend(inserted.iter().cloned());
    }
    merged.extend(remaining);
    *rows = merged;
}

/// The bounded operations that turn `published` into `target`.
///
/// Deletions are emitted first, so an insertion index is the row's final index.
/// A retained row is re-sent only when its displayed value changed; a retained
/// row that moved backwards relative to its neighbours is deleted and reinserted
/// rather than left in an inconsistent position. The shape of this algorithm
/// follows the collection diff already proven in `examples/process-monitor`.
///
/// Every operation carries at most `batch` items and at most `max_bytes` of
/// encoded transaction payload. The byte bound is applied here, not only when
/// the plan is chunked into transactions, because a transaction must hold at
/// least one operation: an operation that alone exceeded the frame bound could
/// not be rescued by any later split.
fn diff(published: &[Row], target: &[Row], batch: usize, max_bytes: usize) -> Vec<Planned> {
    let target_positions: HashMap<ItemId, usize> = target
        .iter()
        .enumerate()
        .map(|(index, row)| (row.item_id, index))
        .collect();

    let mut deleted: Vec<ItemId> = Vec::new();
    let mut retained: HashSet<ItemId> = HashSet::new();
    let mut highest_kept: Option<usize> = None;
    for row in published {
        match target_positions.get(&row.item_id) {
            None => deleted.push(row.item_id),
            Some(&position) => {
                if highest_kept.is_some_and(|kept| position < kept) {
                    deleted.push(row.item_id);
                } else {
                    highest_kept = Some(position);
                    retained.insert(row.item_id);
                }
            }
        }
    }

    // What one operation may spend on items once its own framing and header are
    // paid for.
    let budget = max_bytes
        .saturating_sub(OPERATION_FRAMING_BYTES + OPERATION_HEADER_BYTES)
        .max(1);

    let mut plan = Vec::new();
    // Every deleted row costs the same fixed number of bytes, so the byte bound
    // is a second cap on the chunk length rather than a running total.
    let deleted_batch = batch.min(budget / ITEM_ID_BYTES).max(1);
    for chunk in deleted.chunks(deleted_batch) {
        plan.push(Planned::new(Effect::Delete(chunk.to_vec())));
    }

    let mut run_start: u64 = 0;
    let mut run: Vec<Row> = Vec::new();
    let mut run_bytes: usize = 0;
    for (index, row) in target.iter().enumerate() {
        if retained.contains(&row.item_id) {
            flush_insert(&mut plan, run_start, &mut run, &mut run_bytes);
            continue;
        }
        let cost = row_wire_bytes(row);
        if !run.is_empty() && (run.len() == batch || run_bytes + cost > budget) {
            flush_insert(&mut plan, run_start, &mut run, &mut run_bytes);
        }
        if run.is_empty() {
            run_start = index as u64;
        }
        run.push(row.clone());
        run_bytes += cost;
    }
    flush_insert(&mut plan, run_start, &mut run, &mut run_bytes);

    let previous: HashMap<ItemId, &Value> = published
        .iter()
        .map(|row| (row.item_id, &row.value))
        .collect();
    let changed: Vec<Row> = target
        .iter()
        .filter(|row| retained.contains(&row.item_id))
        .filter(|row| {
            previous
                .get(&row.item_id)
                .is_some_and(|old| **old != row.value)
        })
        .cloned()
        .collect();
    let mut updates: Vec<Row> = Vec::new();
    let mut update_bytes: usize = 0;
    for row in changed {
        let cost = row_wire_bytes(&row);
        if !updates.is_empty() && (updates.len() == batch || update_bytes + cost > budget) {
            plan.push(Planned::new(Effect::Update(std::mem::take(&mut updates))));
            update_bytes = 0;
        }
        updates.push(row);
        update_bytes += cost;
    }
    if !updates.is_empty() {
        plan.push(Planned::new(Effect::Update(updates)));
    }
    plan
}

/// The tree's headings for `layout` (PX-018): the table's, in the same order
/// — the first is the outline column — and the parent column last.
pub fn tree_headings(layout: &ColumnLayout) -> Vec<Value> {
    let mut headings = column_headings(layout);
    headings.push(Value::String(tree::PARENT_HEADING.into()));
    headings
}

/// Rows inserted into `mode`'s model from `start`, in operations of at most
/// `batch` items and `max_bytes` (PX-018).
fn push_inserts(
    plan: &mut Vec<Planned>,
    mode: ViewMode,
    start: usize,
    rows: &[Row],
    batch: usize,
    max_bytes: usize,
) {
    let budget = max_bytes
        .saturating_sub(OPERATION_FRAMING_BYTES + OPERATION_HEADER_BYTES)
        .max(1);
    let mut at = start;
    let mut run: Vec<Row> = Vec::new();
    let mut run_bytes = 0usize;
    let flush = |plan: &mut Vec<Planned>, at: &mut usize, run: &mut Vec<Row>| {
        if run.is_empty() {
            return;
        }
        let rows = std::mem::take(run);
        let count = rows.len();
        plan.push(Planned::new(match mode {
            ViewMode::Table => Effect::Insert(*at as u64, rows),
            ViewMode::Tree => Effect::TreeInsert(*at as u64, rows),
        }));
        *at += count;
    };
    for row in rows {
        let cost = row_wire_bytes(row);
        if !run.is_empty() && (run.len() == batch || run_bytes + cost > budget) {
            flush(plan, &mut at, &mut run);
            run_bytes = 0;
        }
        run.push(row.clone());
        run_bytes += cost;
    }
    flush(plan, &mut at, &mut run);
}

/// In-place replacements of the tree rows at `indices` (ascending) by
/// `target`'s, one operation per consecutive run of at most `batch` items
/// and `max_bytes` (PX-018).
fn push_tree_updates(
    plan: &mut Vec<Planned>,
    indices: impl IntoIterator<Item = usize>,
    target: &[Row],
    batch: usize,
    max_bytes: usize,
) {
    let budget = max_bytes
        .saturating_sub(OPERATION_FRAMING_BYTES + OPERATION_HEADER_BYTES)
        .max(1);
    let mut run_start = 0usize;
    let mut run: Vec<Row> = Vec::new();
    let mut run_bytes = 0usize;
    for index in indices {
        let row = &target[index];
        let cost = row_wire_bytes(row);
        let contiguous = run_start + run.len() == index;
        if !run.is_empty() && (!contiguous || run.len() == batch || run_bytes + cost > budget) {
            plan.push(Planned::new(Effect::TreeUpdate(
                run_start as u64,
                std::mem::take(&mut run),
            )));
            run_bytes = 0;
        }
        if run.is_empty() {
            run_start = index;
        }
        run.push(row.clone());
        run_bytes += cost;
    }
    if !run.is_empty() {
        plan.push(Planned::new(Effect::TreeUpdate(run_start as u64, run)));
    }
}

/// The operations that turn the published preorder `published` into `target`
/// on the tree's model (PX-018), as one unit [`commit`] never splits.
///
/// A hierarchical model is edited by index, never by item identity: a sparse
/// replica cannot shift the indices it caches for an identity delete of an
/// item it does not cache (PX-018-G01). And the store judges the shape once,
/// after a transaction's last operation on the model, so an edit and the
/// ancestors' `descendant_count` updates it implies must commit together: a
/// tree edit is therefore one joined unit, which the row ceiling sizes to fit
/// one transaction (a full replacement is no larger than a catch-up snapshot).
///
/// The rows `published` and `target` share as a leading run and a trailing
/// run stay in place, updated where their cells or place changed; the middle
/// between them is deleted as one range and reinserted from `target`, under
/// the same item IDs where the rows are the same processes. Every row keeps
/// its item identity, so a client's selection and collapsed set, both
/// anchored to item IDs, survive the transaction (PX-018-G01). When even that
/// does not fit `max_ops` and `max_bytes`, the whole preorder is replaced.
///
/// lc-debt: a reorder in the middle of a large tree resends every row between
/// the first and the last that moved; an upgrade path is a subtree-move planner
/// that deletes and reinserts only the moved subtrees, in index ranges.
fn plan_tree(
    published: &[Row],
    target: &[Row],
    batch: usize,
    max_ops: usize,
    max_bytes: usize,
) -> Vec<Planned> {
    let prefix = published
        .iter()
        .zip(target)
        .take_while(|(old, new)| old.item_id == new.item_id)
        .count();
    let suffix = published[prefix..]
        .iter()
        .rev()
        .zip(target[prefix..].iter().rev())
        .take_while(|(old, new)| old.item_id == new.item_id)
        .count();
    let old_middle = published.len() - prefix - suffix;
    let new_middle = target.len() - prefix - suffix;
    let changed = |old: &Row, new: &Row| old.value != new.value || old.tree != new.tree;
    let mut plan = Vec::new();
    if old_middle > 0 {
        plan.push(Planned::new(Effect::DeleteRange {
            model: TREE_MODEL,
            index: prefix as u64,
            count: old_middle as u64,
        }));
    }
    push_inserts(
        &mut plan,
        ViewMode::Tree,
        prefix,
        &target[prefix..prefix + new_middle],
        batch,
        max_bytes,
    );
    let kept_before = (0..prefix).filter(|&index| changed(&published[index], &target[index]));
    let kept_after = (0..suffix)
        .map(|offset| {
            (
                published.len() - suffix + offset,
                target.len() - suffix + offset,
            )
        })
        .filter(|&(old, new)| changed(&published[old], &target[new]))
        .map(|(_, new)| new);
    push_tree_updates(
        &mut plan,
        kept_before.chain(kept_after),
        target,
        batch,
        max_bytes,
    );
    let ops: usize = plan.iter().map(Planned::operations).sum();
    let bytes: usize = plan.iter().map(|planned| planned.bytes).sum();
    if ops > max_ops || bytes > max_bytes {
        plan.clear();
        if !published.is_empty() {
            plan.push(Planned::new(Effect::DeleteRange {
                model: TREE_MODEL,
                index: 0,
                count: published.len() as u64,
            }));
        }
        push_inserts(&mut plan, ViewMode::Tree, 0, target, batch, max_bytes);
    }
    if let Some(last) = plan.len().checked_sub(1) {
        for planned in &mut plan[..last] {
            planned.joins_next = true;
        }
    }
    plan
}

fn flush_insert(plan: &mut Vec<Planned>, start: u64, rows: &mut Vec<Row>, bytes: &mut usize) {
    *bytes = 0;
    if rows.is_empty() {
        return;
    }
    plan.push(Planned::new(Effect::Insert(start, std::mem::take(rows))));
}

/// Gives every published row the order values `target` holds for it: those of
/// the latest sample, or the ones a retained row was last published with.
///
/// Order values are server-side only and are not compared when a refresh
/// decides what to send, so a value that changed without changing its cell —
/// bytes inside one displayed unit — would otherwise never reach the view, and
/// a later click would sort by it (PX-009).
///
/// The catalogued cells of hidden columns are server-side in the same way: a
/// hidden cell that changed leaves the published value as it was, so it is
/// brought up to `target` here too, and showing the column again publishes the
/// latest sample rather than the one the column was hidden under (PX-068).
///
/// The display name and command text the filter and search match against are
/// server-side too (PX-010, PX-011): a command-only change alters no cell and
/// publishes nothing, and a rename's `MODEL_UPDATE` carries only cells. They
/// are brought up to `target` here, so search counts, steps, selects and
/// resolves PIDs against the same, current metadata as the collection it
/// counts over.
fn refresh_sort_values(rows: &mut [Row], target: &[Row]) {
    let current: HashMap<ItemId, &Row> = target.iter().map(|row| (row.item_id, row)).collect();
    for row in rows.iter_mut() {
        if let Some(latest) = current.get(&row.item_id) {
            row.adopt_sample(latest);
        }
    }
}

/// A move plan, and the rows a client holds once all of it committed.
struct Moves {
    plan: Vec<Planned>,
    /// `published`, with every moved row in its new place and carrying the
    /// value `target` gives it.
    rows: Vec<Row>,
}

/// The moves that put the rows `published` and `target` share into `target`'s
/// order, every row keeping its item ID (§6.2, §8; PX-009).
///
/// The standard model operations have no move (§13: insert, delete, update and
/// reset a range), and §8 offers insert and delete *by item identity*. A row
/// therefore changes place by being deleted and reinserted under its own item
/// ID; no new row ever stands in for it. Three properties make that a move
/// rather than a replacement:
///
/// * Only the rows that must move do. The rows left in place are a longest
///   increasing subsequence of the published rows' ranks in `target`, so a
///   process that climbed the CPU ranking moves alone, not with every row it
///   passed.
/// * No revision a client can hold is missing a moved row. A row's deletion and
///   its reinsertion are one unit that [`commit`] never splits
///   ([`Planned::joins_next`]), and each unit is sized to fit one transaction by
///   itself, by operations (`max_ops`), items per operation (`batch`) and bytes
///   (`max_bytes`). A client applies a transaction whole, so a selection it
///   anchors to item IDs is restored onto the same item after each one.
/// * Units are planned in `target` order, and each moved row is reinserted
///   right after the row that precedes it in `target`, which is already in its
///   final relative place. A reorder too large for one transaction therefore
///   passes through revisions that hold every row, with the rows placed so far
///   already in their final order.
///
/// A store whose transactions hold a single operation cannot carry a move at
/// all; there each operation travels alone, as every other plan's does.
///
/// Rows only in `published` — about to be deleted — stay where they are, and rows
/// only in `target` — about to be inserted — are not this function's. A moved row
/// is reinserted with `target`'s value, so a row that moved and changed is sent
/// once.
fn plan_moves(
    published: &[Row],
    target: &[Row],
    batch: usize,
    max_ops: usize,
    max_bytes: usize,
) -> Moves {
    let in_published: HashSet<ItemId> = published.iter().map(|row| row.item_id).collect();
    // The shared rows, in their target order; a row's rank is its place there.
    let order: Vec<&Row> = target
        .iter()
        .filter(|row| in_published.contains(&row.item_id))
        .collect();
    let rank: HashMap<ItemId, usize> = order
        .iter()
        .enumerate()
        .map(|(index, row)| (row.item_id, index))
        .collect();
    let ranks: Vec<usize> = published
        .iter()
        .filter_map(|row| rank.get(&row.item_id).copied())
        .collect();
    let kept = longest_increasing(&ranks, order.len());
    let mut rows = published.to_vec();
    let mut plan = Vec::new();
    if kept.iter().all(|&kept| kept) {
        return Moves { plan, rows };
    }

    let joined = max_ops >= 2;
    let operation = OPERATION_FRAMING_BYTES + OPERATION_HEADER_BYTES;
    // What one operation may spend on items once its framing and header are
    // paid for, exactly as `diff` charges it.
    let budget = max_bytes.saturating_sub(operation).max(1);
    let deleted_batch = batch.min(budget / ITEM_ID_BYTES).max(1);
    let mut unit = MoveUnit::default();
    for (index, row) in order.iter().enumerate() {
        if kept[index] {
            continue;
        }
        let cost = row_wire_bytes(row);
        // A row continues the last run when the row before it in `target` is
        // that run's last row, and the run has room for it.
        let mut extends = index > 0
            && unit.runs.last().is_some_and(|run| {
                run.last_rank == index - 1 && run.rows.len() < batch && run.bytes + cost <= budget
            });
        let runs = unit.runs.len() + usize::from(!extends);
        let moved = unit.moved + 1;
        let deletions = moved.div_ceil(deleted_batch);
        let ops = deletions + runs;
        let bytes = deletions * operation
            + moved * ITEM_ID_BYTES
            + runs * operation
            + unit.row_bytes
            + cost;
        if unit.moved > 0 && joined && (ops > max_ops || bytes > max_bytes) {
            // This row would not fit the unit's transaction: the unit is
            // complete, and the row opens the next one.
            flush_moves(
                std::mem::take(&mut unit),
                &mut rows,
                &mut plan,
                deleted_batch,
                joined,
            );
            extends = false;
        }
        if extends {
            let run = unit.runs.last_mut().expect("a run continues");
            run.rows.push((*row).clone());
            run.bytes += cost;
            run.last_rank = index;
        } else {
            unit.runs.push(MoveRun {
                anchor: index.checked_sub(1).map(|previous| order[previous].item_id),
                rows: vec![(*row).clone()],
                bytes: cost,
                last_rank: index,
            });
        }
        unit.moved += 1;
        unit.row_bytes += cost;
    }
    if unit.moved > 0 {
        flush_moves(unit, &mut rows, &mut plan, deleted_batch, joined);
    }
    Moves { plan, rows }
}

/// The rows of one transaction-sized group of moves.
#[derive(Default)]
struct MoveUnit {
    runs: Vec<MoveRun>,
    moved: usize,
    row_bytes: usize,
}

/// Rows consecutive in the target order, reinserted by one `MODEL_INSERT`
/// right after `anchor`: the row before the first of them in the target order,
/// or the front of the collection when there is none.
struct MoveRun {
    anchor: Option<ItemId>,
    rows: Vec<Row>,
    bytes: usize,
    last_rank: usize,
}

/// Plans one unit of moves — its deletions, then its reinsertions in ascending
/// order, each at its final index — and applies it to `rows`.
fn flush_moves(
    unit: MoveUnit,
    rows: &mut Vec<Row>,
    plan: &mut Vec<Planned>,
    deleted_batch: usize,
    joined: bool,
) {
    let first = plan.len();
    let ids: Vec<ItemId> = unit
        .runs
        .iter()
        .flat_map(|run| run.rows.iter().map(|row| row.item_id))
        .collect();
    for chunk in ids.chunks(deleted_batch) {
        plan.push(Planned::in_move(Effect::Delete(chunk.to_vec())));
    }
    let moving: HashSet<ItemId> = ids.into_iter().collect();
    let mut runs: HashMap<Option<ItemId>, Vec<Row>> = unit
        .runs
        .into_iter()
        .map(|run| (run.anchor, run.rows))
        .collect();
    let mut placed: Vec<Row> = Vec::with_capacity(rows.len());
    let mut inserts: Vec<(u64, Vec<Row>)> = Vec::new();
    place_runs(None, &mut runs, &mut placed, &mut inserts);
    for row in std::mem::take(rows) {
        if moving.contains(&row.item_id) {
            continue;
        }
        let id = row.item_id;
        placed.push(row);
        place_runs(Some(id), &mut runs, &mut placed, &mut inserts);
    }
    debug_assert!(runs.is_empty(), "every run's anchor is placed");
    *rows = placed;
    for (index, inserted) in inserts {
        plan.push(Planned::in_move(Effect::Insert(index, inserted)));
    }
    if joined {
        let last = plan.len() - 1;
        for planned in &mut plan[first..last] {
            planned.joins_next = true;
        }
    }
}

/// Places the run anchored after `anchor`, then any run anchored after that
/// run's last row, recording where each one starts.
fn place_runs(
    anchor: Option<ItemId>,
    runs: &mut HashMap<Option<ItemId>, Vec<Row>>,
    placed: &mut Vec<Row>,
    inserts: &mut Vec<(u64, Vec<Row>)>,
) {
    let mut anchor = anchor;
    while let Some(run) = runs.remove(&anchor) {
        anchor = run.last().map(|row| row.item_id);
        inserts.push((placed.len() as u64, run.clone()));
        placed.extend(run);
    }
}

/// Which of `0..len` lie on one longest strictly increasing subsequence of
/// `sequence`, a permutation of them: patience sorting, O(n log n), and the
/// same answer for the same input every time.
fn longest_increasing(sequence: &[usize], len: usize) -> Vec<bool> {
    // `tails[k]` is the position of the smallest last value of any increasing
    // subsequence of length `k + 1` found so far.
    let mut tails: Vec<usize> = Vec::new();
    let mut previous: Vec<Option<usize>> = vec![None; sequence.len()];
    for (position, &value) in sequence.iter().enumerate() {
        let length = tails.partition_point(|&tail| sequence[tail] < value);
        if length > 0 {
            previous[position] = Some(tails[length - 1]);
        }
        if length == tails.len() {
            tails.push(position);
        } else {
            tails[length] = position;
        }
    }
    let mut kept = vec![false; len];
    let mut cursor = tails.last().copied();
    while let Some(position) = cursor {
        kept[sequence[position]] = true;
        cursor = previous[position];
    }
    kept
}

/// A view shared between the poll loop that refreshes it and the sort buttons'
/// handlers that reorder it (PX-009). Each holds the lock only while it
/// publishes, never while the source is sampled, so a click is applied while a
/// slow scan is still running.
pub type SharedView = Arc<Mutex<ProcessView>>;

/// Locks a shared view. A panic while publishing leaves the view recording what
/// committed, as every publish records each transaction as it commits, so a
/// poisoned lock is taken over rather than ending the session.
pub fn lock_view(view: &Mutex<ProcessView>) -> std::sync::MutexGuard<'_, ProcessView> {
    view.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The loop's one ticker: its first tick one `interval` from now, never at
/// once, so a new interval neither fires a burst nor keeps the old phase.
fn ticker_every(interval: Duration) -> tokio::time::Interval {
    let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
    // A slow scan must not make the next ticks fire back to back.
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker
}

/// Polls `source` until `shutdown` is cancelled, publishing each difference.
///
/// A failed sample never ends the loop and never empties the table: the error is
/// published over the last-known rows and the next successful scan converges.
///
/// The view is locked only to publish, after the sample is taken, and is
/// published in the order it holds at that moment (PX-009): a sort request
/// applied while a scan was running is never overwritten by that scan's
/// result, which is published in the newer order.
///
/// Since PX-015 the loop follows the view's sampling controls
/// ([`crate::sampling`]). It owns the only timer: one ticker, replaced — never
/// joined by a second — when the interval changes or the view resumes, and
/// not polled at all while the view is paused, when no sample is taken. A
/// Refresh now press or a Resume view wakes it through the view's one
/// [`tokio::sync::Notify`] for one sample through this same path. A sample is
/// published only if the view's epoch is still the one it was taken under.
/// The ticker and the wake belong to this future, so cancelling `shutdown`
/// ends them with it; nothing is left running.
pub async fn poll<S>(
    view: SharedView,
    mut source: S,
    session: Arc<Session>,
    interval: Duration,
    shutdown: CancellationToken,
) where
    S: ProcessSource + Send + 'static,
{
    let wake = {
        let mut held = lock_view(&view);
        if let Err(error) = held.adopt_interval(&session, interval) {
            eprintln!("srtop: interval not published: {error}");
        }
        held.sampling.wake_handle()
    };
    let mut interval = lock_view(&view).sampling.interval();
    let mut ticker = ticker_every(interval);
    let mut paused = false;
    loop {
        // The first snapshot is already published, so the first periodic
        // sample is one interval from now. `biased`, so a ready shutdown always
        // wins; which of the wake and the tick is taken when both are ready
        // decides nothing, because the request read below does: a tick of a
        // ticker the request retires is ignored either way (PR #95 review, F2).
        let mut ticked = tokio::select! {
            biased;
            _ = shutdown.cancelled() => return,
            _ = wake.notified() => false,
            _ = ticker.tick(), if !paused => true,
        };
        // The request's epoch and the thread setting are read together, so a
        // sample is always taken under the setting of the epoch it is
        // applied under (PX-019).
        let (request, threads) = {
            let mut held = lock_view(&view);
            (held.sampling.take_request(), held.threads_requested())
        };
        paused = request.paused;
        if request.retime || request.interval != interval {
            interval = request.interval;
            ticker = ticker_every(interval);
            // That tick belonged to the ticker just retired. Only an explicit
            // request (Resume view) samples now; a new interval waits one new
            // interval, however long the scan before it ran.
            ticked = false;
        }
        if paused || !(ticked || request.sample_now) {
            continue;
        }
        if request.sample_now && !request.retime {
            // The next periodic sample is one interval after this one.
            ticker = ticker_every(interval);
        }
        // User threads are read only while they are shown (PX-019).
        // Reading a host's process filesystem blocks; keep it off the runtime.
        let sampled = tokio::task::spawn_blocking(move || {
            source.collect_threads(threads);
            let snapshot = source.snapshot();
            (source, snapshot)
        })
        .await;
        let (returned, snapshot) = match sampled {
            Ok(sampled) => sampled,
            Err(error) => {
                eprintln!("srtop: process sampling stopped: {error}");
                return;
            }
        };
        source = returned;
        let status = source.status_text().to_string();
        let published = lock_view(&view).apply_sampled(&session, request.epoch, &status, &snapshot);
        if let Err(error) = published {
            // The store is unchanged by a failed transaction and the view still
            // holds what the client holds, so the next tick simply tries again.
            eprintln!("srtop: refresh not published: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{
        CappedRecords, Completeness, CreationToken, DisplayName, EnumerationIssue,
        FakeProcessSource, IssueScope, LoadAverages, MemoryFigures, MissingReason, Observed,
        ProcessKey, ProcessRecord, ScriptedFakeSource, SkippedRecords, SwapFigures, SystemCpu,
        SystemCpuInterval, SystemSample, FAKE_STATUS_TEXT, MAX_DISPLAY_NAME_CHARS,
    };
    use crate::MAX_SOURCE_STATUS_BYTES;
    use std::collections::BTreeSet;

    /// Retention that keeps every row a scan did not confirm.
    fn everything() -> Retention {
        Retention::Unenumerable
    }

    /// Retention that keeps exactly the rows of `rows`, matched by their PIDs.
    fn only(rows: &[Row]) -> Retention {
        Retention::Skipped(
            rows.iter()
                .filter_map(|row| match row.key.pid {
                    Observed::Known(pid) => Some(pid),
                    Observed::Missing(_) => None,
                })
                .collect::<BTreeSet<u32>>(),
        )
    }

    fn rows_of(view: &ProcessView) -> Vec<(u64, Value)> {
        view.rows
            .iter()
            .map(|row| (row.item_id.get(), row.value.clone()))
            .collect()
    }

    fn keyed(rows: &[Row]) -> Vec<ProcessKey> {
        rows.iter().map(|row| row.key.clone()).collect()
    }

    #[test]
    fn an_unconfirmed_row_keeps_the_place_it_already_holds() {
        let mut ids = SessionItemIds::default();
        let mut source = ScriptedFakeSource::default();
        let first = ids
            .project(&source.snapshot(), &ColumnLayout::default())
            .unwrap();
        assert_eq!(first.len(), 3);
        // The middle row is not confirmed by this scan.
        let confirmed = vec![first[0].clone(), first[2].clone()];
        let merged = retain_unconfirmed(&first, confirmed, &everything());
        assert_eq!(keyed(&merged), keyed(&first), "order must not churn");
        // A scan that named the record it skipped keeps exactly that row.
        let confirmed = vec![first[0].clone(), first[2].clone()];
        let scoped = retain_unconfirmed(&first, confirmed.clone(), &only(&first[1..2]));
        assert_eq!(keyed(&scoped), keyed(&first));
        // The same scan, having accounted for that absence, deletes it instead.
        let believed = retain_unconfirmed(&first, confirmed.clone(), &only(&[]));
        assert_eq!(keyed(&believed), keyed(&confirmed));
    }

    /// A display name of the longest kind this app can publish: every character
    /// four UTF-8 bytes, none of them one the sanitizer replaces or trims.
    fn widest_display_name() -> DisplayName {
        let name = DisplayName::from("\u{20000}".repeat(MAX_DISPLAY_NAME_CHARS).as_str());
        assert_eq!(
            name.as_str().chars().count(),
            MAX_DISPLAY_NAME_CHARS,
            "the sanitizer must keep this name whole"
        );
        assert_eq!(
            name.as_str().len(),
            MAX_DISPLAY_NAME_CHARS * 4,
            "every character must cost four encoded bytes"
        );
        name
    }

    /// The longest text a metric cell can carry. Asserted against the metric
    /// module's own reserved bound, so a later metric that publishes something
    /// wider cannot quietly leave this fixture — and the frame ceiling it proves
    /// — measuring a row narrower than the real one.
    fn widest_metric_cell() -> String {
        let cell = crate::metric::missing_text(MissingReason::Unavailable).to_string();
        assert_eq!(cell.len(), crate::metric::MAX_CELL_BYTES);
        cell
    }

    /// `rows` rows carrying the widest name this app can publish, with item IDs
    /// no fixture in this module has already published.
    fn widest_rows(rows: u64) -> Vec<Row> {
        let name = widest_display_name();
        let template = crate::source::FakeProcessSource.snapshot().records[0]
            .key
            .clone();
        (0..rows)
            .map(|index| {
                let key = ProcessKey {
                    pid: Observed::Known(index as u32 + 1),
                    creation: crate::source::CreationToken::Opaque(format!("widest-{index}")),
                    thread: None,
                    ..template.clone()
                };
                // The cells are the widest missing words, so the order values
                // are missing in both metric columns (PX-009).
                let record = ProcessRecord {
                    kind: crate::source::KIND_NOT_COLLECTED,
                    lineage: crate::source::Lineage::NOT_COLLECTED,
                    owner: crate::source::OWNER_NOT_COLLECTED,
                    key: key.clone(),
                    display_name: name.clone(),
                    resident: Observed::Missing(MissingReason::Unavailable),
                    cpu: crate::source::CpuUsage::Missing(MissingReason::Unavailable),
                };
                Row {
                    owner: crate::source::OWNER_NOT_COLLECTED,
                    lineage: crate::source::Lineage::NOT_COLLECTED,
                    kind: crate::source::KIND_NOT_COLLECTED,
                    tree: None,
                    sort: crate::sort::SortValues::of_record(&key, &record),
                    display_name: name.clone(),
                    item_id: ItemId::new(1_000 + index),
                    cells: crate::catalogue::PROCESS_LIST
                        .iter()
                        .map(|field| field.cell(&key, &record))
                        .collect(),
                    key,
                    value: Value::List(vec![
                        Value::UnsignedInt(index + 1),
                        Value::String(name.as_str().to_string()),
                        // The widest metric cell this app can publish, so the
                        // ceiling this fixture proves covers every real row
                        // (PX-005), in both metric columns (PX-006).
                        Value::String(widest_metric_cell()),
                        Value::String(widest_metric_cell()),
                    ]),
                }
            })
            .collect()
    }

    /// The bytes a transaction carrying `plan` really occupies, measured by the
    /// protocol's own encoder exactly as `SruiCodec` measures a frame (§26).
    fn encoded_frame_bytes(plan: &[Planned]) -> usize {
        let transaction = srui_protocol::Transaction {
            // The widest revisions and priority any transaction could carry, so
            // the measurement never flatters the envelope allowance.
            base_revision: u64::MAX,
            new_revision: u64::MAX,
            priority: u32::MAX,
            operations: plan.iter().map(|planned| planned.op.to_wire()).collect(),
        };
        srui_protocol::framed_payload_len(&srui_protocol::SruiMessage {
            msg: Some(srui_protocol::srui_message::Msg::Transaction(transaction)),
        })
    }

    /// The planned size of an operation is a bound the encoder cannot exceed,
    /// not an average: every string is charged its encoded UTF-8 length, so the
    /// widest name this app publishes is counted at four bytes a character.
    #[test]
    fn a_planned_operation_is_never_smaller_than_what_the_encoder_emits() {
        let rows = widest_rows(64);
        let plan = vec![
            Planned::new(Effect::Status("x".repeat(4096))),
            // A summary line at its widest, a bar value, and a cleared one
            // (PX-007).
            Planned::new(Effect::Summary(summary::Change {
                node: summary::FRESHNESS_TEXT,
                property: TEXT,
                value: Some(Value::String(
                    "\u{20000}".repeat(summary::MAX_SUMMARY_TEXT_BYTES / 4),
                )),
            })),
            Planned::new(Effect::Summary(summary::Change {
                node: summary::CPU_BAR,
                property: srui_sdk::VALUE,
                value: Some(Value::Float64(0.999)),
            })),
            Planned::new(Effect::Summary(summary::Change {
                node: summary::CPU_BAR,
                property: srui_sdk::VALUE,
                value: None,
            })),
            Planned::new(Effect::Delete(
                rows.iter().map(|row| row.item_id).collect::<Vec<ItemId>>(),
            )),
            Planned::new(Effect::Insert(0, rows.clone())),
            Planned::new(Effect::Update(rows)),
        ];
        for planned in &plan {
            let alone = std::slice::from_ref(planned);
            assert!(
                encoded_frame_bytes(alone) <= planned.bytes + TRANSACTION_ENVELOPE_BYTES,
                "planned {} bytes but the encoder emitted {}",
                planned.bytes,
                encoded_frame_bytes(alone)
            );
        }
        let planned_total: usize = plan.iter().map(|planned| planned.bytes).sum();
        assert!(
            encoded_frame_bytes(&plan) <= planned_total + TRANSACTION_ENVELOPE_BYTES,
            "a whole transaction must also stay inside the sum of its planned bytes"
        );
    }

    /// A plan no single frame can carry commits as consecutive transactions,
    /// each one inside the byte ceiling it was given, with the deletion still
    /// ahead of the insertions.
    #[test]
    fn a_plan_too_large_for_one_frame_commits_as_several() {
        let session = srui_sessiond::Session::mint();
        let mut source = ScriptedFakeSource::default();
        let (mut view, _) = ProcessView::start(&session, &mut source).unwrap();
        let rows = widest_rows(8);
        let plan = vec![
            Planned::new(Effect::Delete(vec![view.rows[0].item_id])),
            Planned::new(Effect::Insert(2, rows[..4].to_vec())),
            Planned::new(Effect::Insert(6, rows[4..].to_vec())),
        ];
        // Above any one operation and below the first two together, so only the
        // byte bound can split this plan: three operations are far inside the
        // operation bound it is also given.
        let ceiling = plan[0].bytes + plan[1].bytes;
        let (transactions, outcome) =
            commit(&session, &plan, 1_000, ceiling, &mut view.published());
        assert!(outcome.is_ok());
        assert_eq!(transactions, 2, "three operations, two frames");
        assert_eq!(session.current_revision(), 3);
        assert_eq!(view.row_count(), 10);
        assert_eq!(rows_of(&view), store_rows(&session));

        // Measured on the real encoder, not on the planner's own arithmetic.
        let frames: Vec<usize> = session
            .collect_replayed_transactions(1)
            .expect("the journal holds this run")
            .into_iter()
            .map(|transaction| {
                srui_protocol::framed_payload_len(&srui_protocol::SruiMessage {
                    msg: Some(srui_protocol::srui_message::Msg::Transaction(transaction)),
                })
            })
            .collect();
        assert_eq!(frames.len(), 2);
        for frame in frames {
            assert!(
                frame <= ceiling + TRANSACTION_ENVELOPE_BYTES,
                "a committed frame of {frame} bytes exceeded the {ceiling}-byte ceiling"
            );
        }
        let committed = session
            .collect_replayed_transactions(1)
            .expect("the journal holds this run");
        let ops: Vec<Operation> = committed
            .into_iter()
            .flat_map(|transaction| transaction.operations)
            .map(|op| Operation::try_from(op).expect("a committed operation decodes"))
            .collect();
        assert!(matches!(ops[0], Operation::ModelDelete { .. }));
        assert!(ops[1..]
            .iter()
            .all(|op| matches!(op, Operation::ModelInsert { .. })));
    }

    #[test]
    fn an_unconfirmed_leading_row_stays_ahead_of_the_rows_that_followed_it() {
        let mut ids = SessionItemIds::default();
        let mut source = ScriptedFakeSource::default();
        let first = ids
            .project(&source.snapshot(), &ColumnLayout::default())
            .unwrap();
        let merged = retain_unconfirmed(&first, vec![first[1].clone()], &everything());
        assert_eq!(keyed(&merged), keyed(&first));
        let merged = retain_unconfirmed(&first, Vec::new(), &everything());
        assert_eq!(
            keyed(&merged),
            keyed(&first),
            "a scan that saw nothing changes nothing"
        );
    }

    /// A refresh's counts, with everything the caller did not name left at zero.
    fn counted(retained: usize, rejected: Option<&str>, truncated: usize) -> RefreshCounts {
        RefreshCounts {
            retained,
            rejected: rejected.map(str::to_string),
            truncated,
            ..RefreshCounts::default()
        }
    }

    #[test]
    fn a_status_clause_is_added_only_for_what_actually_happened() {
        let mut source = ScriptedFakeSource::default();
        let snapshot = source.snapshot();
        let label = ScriptedFakeSource::STATUS_TEXT;
        assert_eq!(
            refresh_status(label, &snapshot, &RefreshCounts::default()),
            label
        );
        assert_eq!(
            refresh_status(label, &snapshot, &counted(1, None, 0)),
            format!("{label} · 1 row retained from an earlier scan")
        );
        assert_eq!(
            refresh_status(
                label,
                &snapshot,
                &counted(2, Some("duplicate process instance identity"), 0)
            ),
            format!(
                "{label} · snapshot rejected: duplicate process instance identity · \
                 2 rows retained from an earlier scan"
            )
        );
        // Rows left unpublished for size are their own clause: they were read,
        // so they are never "unreadable", and they are not the entries the
        // collector's record bound never read either.
        assert_eq!(
            refresh_status(label, &snapshot, &counted(0, None, 1)),
            format!("{label} · 1 row beyond the publishable size limit")
        );
        let truncated = refresh_status(label, &snapshot, &counted(2, None, 7));
        assert_eq!(
            truncated,
            format!(
                "{label} · 2 rows retained from an earlier scan · \
                 7 rows beyond the publishable size limit"
            )
        );
        assert!(!truncated.contains("unreadable"), "{truncated}");
        assert!(!truncated.contains("record limit"), "{truncated}");
        // A row dropped because the collection may not grow says exactly that,
        // and is never confused with a row that would not fit, with a record
        // that could not be read, or with an entry beyond the record bound.
        let evicted = refresh_status(
            label,
            &snapshot,
            &RefreshCounts {
                retained: 4,
                evicted: 1,
                ..RefreshCounts::default()
            },
        );
        assert_eq!(
            evicted,
            format!(
                "{label} · 4 rows retained from an earlier scan · \
                 1 unconfirmed row dropped to stay inside the published row limit"
            )
        );
        assert!(!evicted.contains("unreadable"), "{evicted}");
        assert!(!evicted.contains("record limit"), "{evicted}");
        assert!(!evicted.contains("publishable size"), "{evicted}");
        assert_eq!(
            refresh_status(
                label,
                &snapshot,
                &RefreshCounts {
                    evicted: 2,
                    ..RefreshCounts::default()
                }
            ),
            format!("{label} · 2 unconfirmed rows dropped to stay inside the published row limit")
        );
        let mut failed = snapshot.clone();
        failed.records.clear();
        failed.completeness = Completeness::from_scan(
            SkippedRecords::unenumerable(),
            vec![EnumerationIssue {
                scope: IssueScope::Root,
                reason: MissingReason::Denied,
                detail: "scripted failure".into(),
            }],
        );
        assert_eq!(
            refresh_status(label, &failed, &counted(3, None, 0)),
            format!(
                "{label} · incomplete scan · process list unavailable: permission denied · \
                 3 rows retained from an earlier scan"
            )
        );
    }

    /// The status is charged a fixed allowance before the rows are budgeted, so
    /// that allowance must cover the longest status the builder can produce —
    /// otherwise the status could outgrow the frame the rows were chosen to fit.
    ///
    /// Measured against the builder itself, on every clause it can emit with
    /// every count at `usize::MAX`, rather than against a status a fixture
    /// happens to be able to construct: a count is a number, and no ledger bound
    /// limits how large a number a host can produce.
    #[test]
    fn the_reserved_status_allowance_covers_the_longest_status_the_builder_can_emit() {
        // A label longer than any source may publish, in four-byte characters so
        // the cut lands mid-character if it is done by bytes alone.
        let label = "\u{20000}".repeat(MAX_SOURCE_STATUS_BYTES);
        let widest_scan = |root| ScanReport {
            degraded: true,
            root,
            listed: usize::MAX,
            unreadable: usize::MAX,
            capped: usize::MAX,
            identity_incomplete: true,
        };
        let widest_counts = RefreshCounts {
            retained: usize::MAX,
            rejected: Some("\u{20000}".repeat(MAX_REJECTION_BYTES)),
            evicted: usize::MAX,
            truncated: usize::MAX,
        };
        // Both branches of the scan clause: a root that could not be listed
        // publishes its reason, any other degradation publishes its counts.
        for root in [
            None,
            Some(MissingReason::Denied),
            Some(MissingReason::Unavailable),
        ] {
            let status = refresh_status_from(&label, &widest_scan(root), &widest_counts);
            assert!(
                status.len() <= STATUS_RESERVE_BYTES,
                "the widest status is {} bytes, above the {STATUS_RESERVE_BYTES} reserved for \
                 it: {status}",
                status.len()
            );
            assert!(
                status.len() > STATUS_RESERVE_BYTES / 2,
                "the reserve must be measured against a status that really is wide; this one \
                 was {} bytes",
                status.len()
            );
        }
        // The label is cut, marked, and never cut through a character.
        let cut = published_status_from(&label, &ScanReport::default());
        assert!(cut.len() <= MAX_PUBLISHED_LABEL_BYTES);
        assert!(cut.ends_with('…'), "{cut}");
        assert!(label.starts_with(cut.trim_end_matches('…')));
        // A label that fits is published exactly as the source wrote it.
        assert_eq!(
            published_status_from(ScriptedFakeSource::STATUS_TEXT, &ScanReport::default()),
            ScriptedFakeSource::STATUS_TEXT
        );
    }

    /// A host large enough to sit at the snapshot ceiling, whose rows are narrow
    /// enough that the longest status this app can publish costs more than one
    /// of them — so a row budget charged the live status could not pass
    /// [`the_row_budget_never_shrinks_when_the_status_lengthens`] by luck.
    struct Host {
        rows: u32,
        name: DisplayName,
    }

    impl Host {
        fn new(rows: u32, name_chars: usize) -> Self {
            let name = DisplayName::from("\u{20000}".repeat(name_chars).as_str());
            assert_eq!(
                name.as_str().len(),
                name_chars * 4,
                "every character must cost four encoded bytes"
            );
            Self { rows, name }
        }

        /// A complete scan of every process from `first` on, in publishing order.
        fn scan(&self, first: u32) -> ProcessSnapshot {
            let template = FakeProcessSource.snapshot().records[0].key.clone();
            let mut snapshot = FakeProcessSource.snapshot();
            snapshot.records = (first..self.rows)
                .map(|index| ProcessRecord {
                    kind: crate::source::KIND_NOT_COLLECTED,
                    lineage: crate::source::Lineage::NOT_COLLECTED,
                    owner: crate::source::OWNER_NOT_COLLECTED,
                    key: ProcessKey {
                        pid: Observed::Known(index + 1),
                        creation: CreationToken::Opaque(format!("host-{index}")),
                        thread: None,
                        ..template.clone()
                    },
                    display_name: self.name.clone(),
                    // Unread, which is the widest cell a metric publishes: this
                    // host exists to measure the largest collection that fits.
                    resident: Observed::Missing(MissingReason::Unavailable),
                    cpu: crate::source::CpuUsage::Missing(MissingReason::Unavailable),
                })
                .collect();
            snapshot
        }

        /// The same scan, degraded in every way its own status can report: an
        /// entry it could not examine at all, a global identity file it could
        /// not read, and a million entries listed past its record bound.
        fn degraded(&self, first: u32) -> ProcessSnapshot {
            let mut snapshot = self.scan(first);
            let mut skipped = SkippedRecords::with_limit(0);
            skipped.unnamed();
            let mut capped = CappedRecords::with_limit(0);
            for _ in 0..1_000_000u32 {
                capped.record(0);
            }
            snapshot.capped = capped;
            snapshot.completeness = Completeness::from_scan(
                skipped,
                vec![
                    EnumerationIssue {
                        scope: IssueScope::Entry,
                        reason: MissingReason::Unavailable,
                        detail: "directory entry: Input/output error (os error 5)".into(),
                    },
                    EnumerationIssue {
                        scope: IssueScope::BootIdentity,
                        reason: MissingReason::Denied,
                        detail: "sys/kernel/random/boot_id: Permission denied".into(),
                    },
                ],
            );
            snapshot
        }
    }

    /// A [`Host`] whose every scan states `.1` as its system-wide figures.
    struct HostWith(Host, SystemSample);

    impl ProcessSource for HostWith {
        fn status_text(&self) -> &str {
            FAKE_STATUS_TEXT
        }

        fn snapshot(&mut self) -> ProcessSnapshot {
            let mut snapshot = self.0.scan(0);
            snapshot.system = self.1;
            snapshot
        }
    }

    /// Review round 1 (F1): the rows that fit at the snapshot ceiling are the
    /// frame less its fixed allowances — the shell's, the status's and the
    /// summary's 8 KiB — and so the same number of rows whatever the summary
    /// says, from a summary with no figure at all to the widest one. Charging
    /// the summary nothing, or its current length, publishes another number of
    /// rows here. The snapshot a fresh client is sent still fits one frame.
    #[test]
    fn the_rows_at_the_ceiling_are_the_frame_less_the_fixed_allowances_whatever_the_summary() {
        let widest = SystemSample {
            cpu: SystemCpu::Measured(SystemCpuInterval {
                busy: u64::MAX,
                total: u64::MAX,
                cpus: Some(u32::MAX),
            }),
            memory: Ok(MemoryFigures {
                total: u64::MAX,
                available: 1,
            }),
            swap: Ok(SwapFigures {
                total: u64::MAX,
                free: 1,
            }),
            uptime: Ok(u64::MAX),
            load: Ok(LoadAverages {
                one: u64::MAX,
                five: u64::MAX,
                fifteen: u64::MAX,
            }),
        };
        let budget = MAX_TRANSACTION_PAYLOAD_BYTES
            - (SNAPSHOT_SHELL_BYTES
                + STATUS_RESERVE_BYTES
                + SUMMARY_RESERVE_BYTES
                + FILTER_RESERVE_BYTES
                + SEARCH_RESERVE_BYTES
                + OWNER_RESERVE_BYTES
                + INSPECTOR_RESERVE_BYTES
                + COMMAND_RESERVE_BYTES
                + SAMPLING_RESERVE_BYTES
                + FOLLOW_RESERVE_BYTES
                + VIEW_RESERVE_BYTES
                + THREADS_RESERVE_BYTES);
        let mut published = Vec::new();
        for system in [
            SystemSample::not_provided(),
            FakeProcessSource::system(),
            widest,
        ] {
            let session = srui_sessiond::Session::mint();
            let mut source = HostWith(Host::new(58_000, 48), system);
            let (mut view, snapshot) = ProcessView::start(&session, &mut source).unwrap();
            // Every row of this host costs the same, so the rows that fit are a
            // function of the budget alone.
            let cost = row_wire_bytes(&view.rows[0]);
            assert!(view.rows.iter().all(|row| row_wire_bytes(row) == cost));
            let (mut spent, mut fits) = (0usize, 0usize);
            loop {
                let opens = if fits.is_multiple_of(DEFAULT_MAX_ITEMS_PER_MODEL_OPERATION) {
                    SNAPSHOT_RANGE_BYTES
                } else {
                    0
                };
                if spent + cost + opens > budget {
                    break;
                }
                spent += cost + opens;
                fits += 1;
            }
            assert!(fits < snapshot.records.len(), "the ceiling must bind here");
            assert_eq!(
                view.row_count(),
                fits,
                "{system:?}: the rows that fit are the frame less the fixed allowances"
            );
            published.push(view.row_count());
            if system == widest {
                // PX-011: the table's promoted selection travels in the same
                // snapshot and is covered by the search reserve.
                let Observed::Known(pid) = view.rows[view.rows.len() - 1].key.pid else {
                    panic!("the host's rows have PIDs")
                };
                view.jump_to_pid(&session, &pid.to_string()).unwrap();
                assert!(view.promoted.is_some());
                let hello = srui_protocol::ClientHello {
                    core_version: "0.5.0".to_string(),
                    profiles: vec!["org.srui.standard-widgets/1".to_string()],
                    limits: None,
                    client_instance_id: vec![1, 2, 3],
                    client_metadata: Default::default(),
                    known_resource_hashes: vec![],
                };
                let snapshot = session
                    .bootstrap_fresh_client(&hello)
                    .expect("a fresh client attaches")
                    .snapshot
                    .expect("a populated session sends a catch-up snapshot");
                let frame = srui_protocol::framed_payload_len(&srui_protocol::SruiMessage {
                    msg: Some(srui_protocol::srui_message::Msg::Transaction(snapshot)),
                });
                assert!(frame <= DEFAULT_MAX_FRAME_SIZE, "{frame}");
            }
        }
        assert!(
            published.windows(2).all(|pair| pair[0] == pair[1]),
            "{published:?}: the summary's length moved the row budget"
        );
    }

    impl ProcessSource for Host {
        fn status_text(&self) -> &str {
            FAKE_STATUS_TEXT
        }

        fn snapshot(&mut self) -> ProcessSnapshot {
            self.scan(0)
        }
    }

    /// The regression the fixed reserve exists for. A collection sitting at the
    /// snapshot ceiling is published; the next scan is degraded in every way it
    /// can report, under the longest label a source may publish, so its status
    /// is far longer than the one the rows were chosen under. Charging that text
    /// against the frame would shrink the row budget and delete trailing rows
    /// the scan never said had ended.
    #[test]
    fn the_row_budget_never_shrinks_when_the_status_lengthens() {
        let session = srui_sessiond::Session::mint();
        let mut source = Host::new(58_000, 48);
        let (mut view, snapshot) = ProcessView::start(&session, &mut source).unwrap();
        let published = view.row_count();
        let truncated = snapshot.records.len() - published;
        assert!(
            truncated > 0,
            "this collection must really sit at the ceiling: all {published} rows fit"
        );
        let before: Vec<ItemId> = view.rows.iter().map(|row| row.item_id).collect();
        let short = view.status().to_string();

        // The same host, degraded, missing its three leading records — so the
        // status carries the retained clause too — under a label at this app's
        // own label bound.
        let label = "\u{20000}".repeat(MAX_SOURCE_STATUS_BYTES);
        let outcome = view.apply(&session, &label, &source.degraded(3)).unwrap();
        let long = view.status().to_string();
        assert!(
            long.len() - short.len() > row_wire_bytes(&view.rows[0]),
            "this status must cost more than one row — {} bytes against a {}-byte row — or a \
             budget charged the live status could pass this test by luck",
            long.len() - short.len(),
            row_wire_bytes(&view.rows[0])
        );
        assert_eq!(
            (outcome.deleted, outcome.evicted),
            (0, 0),
            "a longer status must not cost the collection a row"
        );
        assert_eq!(outcome.retained, 3);
        assert_eq!(view.row_count(), published);
        assert_eq!(
            view.rows
                .iter()
                .map(|row| row.item_id)
                .collect::<Vec<ItemId>>(),
            before,
            "every row keeps the identity and the place it already had"
        );
    }

    /// An upper bound on the bytes one `CREATE_NODE` occupies, on the same
    /// accounting the planner uses for every other operation.
    fn create_node_bytes(operation: &Operation) -> usize {
        let Operation::CreateNode { properties, .. } = operation else {
            panic!("not a node creation: {operation:?}")
        };
        OPERATION_FRAMING_BYTES
            + OPERATION_HEADER_BYTES
            + properties
                .iter()
                .map(|(_, value)| RECORD_PROPERTY_BYTES + value_wire_bytes(value))
                .sum::<usize>()
    }

    /// The widest summary the publication path can emit: every line at the
    /// summary's own text bound and every bar shown, with a value and a
    /// description at its bound. [`summary::properties`] cuts every line and
    /// description to those bounds, so no summary is wider than this one.
    fn widest_summary() -> summary::Properties {
        let mut widest = summary::properties(&summary::summarize(&summary::State::NotStarted));
        let text = Value::String("\u{20000}".repeat(summary::MAX_SUMMARY_TEXT_BYTES / 4));
        let description = Value::String("\u{20000}".repeat(summary::MAX_BAR_DESCRIPTION_BYTES / 4));
        for (&(_, property), value) in widest.iter_mut() {
            if property == TEXT {
                *value = text.clone();
            } else if property == srui_sdk::VALUE_DESCRIPTION {
                *value = description.clone();
            }
        }
        for bar in [summary::CPU_BAR, summary::MEMORY_BAR, summary::SWAP_BAR] {
            widest.insert((bar, srui_sdk::VALUE), Value::Float64(1.0));
            widest.insert(
                (bar, srui_sdk::VISIBILITY),
                Value::from(srui_sdk::EnumToken::from(srui_sdk::Visibility::Visible)),
            );
        }
        widest
    }

    /// PX-007: the summary is charged a fixed reserve against the catch-up
    /// snapshot, so that reserve must cover the widest summary this app can
    /// publish — on the planner's own accounting, which must in turn cover what
    /// the protocol's encoder really emits for those nodes.
    #[test]
    fn the_summary_reserve_covers_the_widest_summary_on_the_real_encoder() {
        let widest = widest_summary();
        for (node, property) in widest.keys() {
            if *property == TEXT {
                let Some(Value::String(text)) = widest.get(&(*node, *property)) else {
                    unreachable!()
                };
                assert_eq!(text.len(), summary::MAX_SUMMARY_TEXT_BYTES);
            }
        }
        let operations = summary::create_operations(&widest);
        let planned: usize = operations.iter().map(create_node_bytes).sum();
        assert!(
            planned <= SUMMARY_RESERVE_BYTES,
            "the widest summary is planned at {planned} bytes, above the \
             {SUMMARY_RESERVE_BYTES} reserved for it"
        );
        assert!(
            planned > SUMMARY_RESERVE_BYTES / 2,
            "the reserve must be measured against a summary that really is wide: {planned}"
        );
        let transaction = srui_protocol::Transaction {
            base_revision: u64::MAX,
            new_revision: u64::MAX,
            priority: u32::MAX,
            operations: operations.iter().map(Operation::to_wire).collect(),
        };
        let encoded = srui_protocol::framed_payload_len(&srui_protocol::SruiMessage {
            msg: Some(srui_protocol::srui_message::Msg::Transaction(transaction)),
        });
        assert!(
            encoded <= planned + TRANSACTION_ENVELOPE_BYTES,
            "planned {planned} bytes but the encoder emitted {encoded}"
        );
        println!(
            "PX-007 summary reserve: widest summary planned={planned} encoded={encoded} \
             reserve={SUMMARY_RESERVE_BYTES}"
        );
    }

    /// PX-007: everything in a catch-up snapshot that is not a row — the shell,
    /// the widest status and the widest summary — fits the fixed allowances the
    /// row budget is charged before any row, measured on the snapshot a fresh
    /// client is really sent.
    #[test]
    fn the_shell_status_and_summary_fit_their_snapshot_allowances() {
        let session = srui_sessiond::Session::mint();
        let status = "\u{20000}".repeat(STATUS_RESERVE_BYTES / 4);
        // PX-012: the user-filter status at its widest, ellipsis included.
        let owner = OwnerView {
            status: crate::bounded(
                &"\u{20000}".repeat(crate::owner::MAX_OWNER_STATUS_BYTES / 4 + 1),
                crate::owner::MAX_OWNER_STATUS_BYTES,
            ),
            mine: true,
            all: true,
        };
        // PX-015: the sampling controls of a collecting view.
        let sampling = crate::sampling::Sampling::new(MAX_REFRESH_INTERVAL);
        crate::initialize_rows(
            &session,
            Vec::new(),
            0,
            &status,
            &widest_summary(),
            &owner,
            false,
            Some(&sampling),
            &"t".repeat(crate::threads::MAX_THREADS_STATUS_BYTES),
        )
        .unwrap();
        // PX-011: every search value at its widest — the status at its bound,
        // the query at the filter's cap and the PID field at its own.
        session
            .transaction(|ui| {
                ui.set(
                    SEARCH_STATUS,
                    TEXT,
                    "\u{20000}".repeat(crate::search::MAX_SEARCH_STATUS_BYTES / 4),
                )?;
                ui.set(
                    SEARCH_INPUT,
                    VALUE,
                    "\u{20000}".repeat(crate::filter::MAX_FILTER_QUERY_BYTES / 4),
                )?;
                ui.set(
                    PID_INPUT,
                    VALUE,
                    "9".repeat(crate::search::MAX_PID_INPUT_BYTES),
                )?;
                // PX-012: the user and PID-set fields at their caps.
                ui.set(
                    OWNER_INPUT,
                    VALUE,
                    "\u{20000}".repeat(crate::owner::MAX_OWNER_INPUT_BYTES / 4),
                )?;
                ui.set(
                    PID_SET_INPUT,
                    VALUE,
                    "9".repeat(crate::owner::MAX_PID_SET_INPUT_BYTES),
                )?;
                // PX-013: every inspector line at its widest, cut mark
                // included.
                let widest = crate::bounded(
                    &"\u{20000}".repeat(crate::inspector::MAX_INSPECTOR_LINE_BYTES / 4 + 1),
                    crate::inspector::MAX_INSPECTOR_LINE_BYTES,
                );
                for node in [
                    INSPECTOR_STATUS,
                    INSPECTOR_PROCESS,
                    INSPECTOR_DETAILS,
                    INSPECTOR_METRICS,
                ] {
                    ui.set(node, TEXT, widest.as_str())?;
                }
                // PX-014: the command-line view at its widest.
                let widest_command = crate::bounded(
                    &"\u{20000}".repeat(crate::command::MAX_COMMAND_TEXT_BYTES / 4 + 1),
                    crate::command::MAX_COMMAND_TEXT_BYTES,
                );
                ui.set(INSPECTOR_COMMAND, TEXT, widest_command.as_str())?;
                // PX-015: the sampling status and the interval field at
                // their widest, cut mark included.
                ui.set(
                    SAMPLING_STATUS,
                    TEXT,
                    crate::bounded(
                        &"\u{20000}".repeat(crate::sampling::MAX_SAMPLING_STATUS_BYTES / 4 + 1),
                        crate::sampling::MAX_SAMPLING_STATUS_BYTES,
                    ),
                )?;
                ui.set(
                    INTERVAL_INPUT,
                    VALUE,
                    "\u{20000}".repeat(crate::sampling::MAX_INTERVAL_INPUT_BYTES / 4),
                )?;
                ui.set(SAMPLING_PAUSE, LABEL, crate::sampling::RESUME_LABEL)?;
                // PX-017: the follow status at its widest, cut mark
                // included, with follow on.
                ui.set(
                    crate::FOLLOW_STATUS,
                    TEXT,
                    crate::bounded(
                        &"\u{20000}".repeat(crate::follow::MAX_FOLLOW_STATUS_BYTES / 4 + 1),
                        crate::follow::MAX_FOLLOW_STATUS_BYTES,
                    ),
                )?;
                ui.set(crate::FOLLOW_TOGGLE, VALUE, true)?;
                // PX-018: the view status at its widest, cut mark included,
                // with the tree shown.
                ui.set(
                    VIEW_STATUS,
                    TEXT,
                    crate::bounded(
                        &"\u{20000}".repeat(MAX_VIEW_STATUS_BYTES / 4 + 1),
                        MAX_VIEW_STATUS_BYTES,
                    ),
                )?;
                ui.set(VIEW_TOGGLE, VALUE, true)?;
                // PX-016: the filter field at its cap, so the filter group
                // below is measured with its value at its widest.
                ui.set(
                    crate::FILTER_INPUT,
                    VALUE,
                    "\u{20000}".repeat(crate::filter::MAX_FILTER_QUERY_BYTES / 4),
                )?;
                Ok(())
            })
            .unwrap();
        // PX-013: the inspector alone, measured on the encoder against its
        // own reserve.
        let encoded = |ids: &[NodeId]| {
            session.with_store(|store| {
                ids.iter()
                    .map(|id| {
                        let node = store.get_node(*id).unwrap();
                        let operation = Operation::create_node(
                            *id,
                            node.node_type,
                            node.parent_id,
                            None,
                            node.properties
                                .iter()
                                .map(|(property, value)| (*property, value.clone())),
                        );
                        srui_protocol::framed_payload_len(&srui_protocol::SruiMessage {
                            msg: Some(srui_protocol::srui_message::Msg::Transaction(
                                srui_protocol::Transaction {
                                    base_revision: 0,
                                    new_revision: 0,
                                    priority: 0,
                                    operations: vec![operation.to_wire()],
                                },
                            )),
                        })
                    })
                    .sum::<usize>()
            })
        };
        let inspector = encoded(&[
            crate::INSPECTOR,
            INSPECTOR_STATUS,
            INSPECTOR_PROCESS,
            INSPECTOR_DETAILS,
            INSPECTOR_METRICS,
        ]);
        let command = encoded(&[INSPECTOR_COMMAND]);
        // PX-017: the follow group, and the table's `keep_visible` hint at
        // its widest item ID, which the follow reserve also covers.
        let hint = srui_protocol::framed_payload_len(&srui_protocol::SruiMessage {
            msg: Some(srui_protocol::srui_message::Msg::Transaction(
                srui_protocol::Transaction {
                    base_revision: 0,
                    new_revision: 0,
                    priority: 0,
                    operations: vec![Operation::set_property(
                        TABLE,
                        KEEP_VISIBLE,
                        Value::ItemId(ItemId::new(u64::MAX)),
                    )
                    .to_wire()],
                },
            )),
        });
        let follow = encoded(&[
            crate::FOLLOW_ROW,
            crate::FOLLOW_TOGGLE,
            crate::FOLLOW_STATUS,
        ]) + hint;
        // PX-018: the view group, the tree with its headings, its model, and
        // its promoted selection and hint at their widest item IDs, which the
        // view reserve covers.
        let tree_extras = srui_protocol::framed_payload_len(&srui_protocol::SruiMessage {
            msg: Some(srui_protocol::srui_message::Msg::Transaction(
                srui_protocol::Transaction {
                    base_revision: 0,
                    new_revision: 0,
                    priority: 0,
                    operations: vec![
                        Operation::create_model(TREE_MODEL, srui_sdk::TypeRef::TREE, u64::MAX)
                            .to_wire(),
                        Operation::set_property(TREE, VALUE, Value::ItemId(ItemId::new(u64::MAX)))
                            .to_wire(),
                        Operation::set_property(
                            TREE,
                            KEEP_VISIBLE,
                            Value::ItemId(ItemId::new(u64::MAX)),
                        )
                        .to_wire(),
                    ],
                },
            )),
        });
        let view = encoded(&[crate::VIEW_ROW, VIEW_TOGGLE, VIEW_STATUS, TREE]) + tree_extras;
        // PX-019: the thread controls, their status at its bound.
        let threads = encoded(&[
            crate::THREADS_ROW,
            USER_THREADS_TOGGLE,
            KERNEL_THREADS_TOGGLE,
            THREADS_STATUS,
        ]);
        assert!(
            threads <= THREADS_RESERVE_BYTES,
            "the widest thread controls encode to {threads} bytes, above their {THREADS_RESERVE_BYTES} byte reserve"
        );
        assert!(
            threads > THREADS_RESERVE_BYTES / 4,
            "the reserve must be measured against thread controls that really are wide: {threads}"
        );
        let sampling = encoded(&[
            crate::SAMPLING_ROW,
            SAMPLING_PAUSE,
            SAMPLING_REFRESH,
            INTERVAL_INPUT,
            crate::INTERVAL_APPLY,
            SAMPLING_STATUS,
        ]);
        // PX-016: the keyboard shortcuts travel on controls in five of the
        // charged groups. Each group is measured with its shortcuts against
        // its reserve, and the shortcuts' own cost is stated.
        let without_shortcuts = |ids: &[NodeId]| {
            session.with_store(|store| {
                ids.iter()
                    .map(|id| {
                        let node = store.get_node(*id).unwrap();
                        let operation = Operation::create_node(
                            *id,
                            node.node_type,
                            node.parent_id,
                            None,
                            node.properties
                                .iter()
                                .filter(|(property, _)| {
                                    **property != srui_semantic_tree::PropertyRef::SHORTCUT
                                })
                                .map(|(property, value)| (*property, value.clone())),
                        );
                        srui_protocol::framed_payload_len(&srui_protocol::SruiMessage {
                            msg: Some(srui_protocol::srui_message::Msg::Transaction(
                                srui_protocol::Transaction {
                                    base_revision: 0,
                                    new_revision: 0,
                                    priority: 0,
                                    operations: vec![operation.to_wire()],
                                },
                            )),
                        })
                    })
                    .sum::<usize>()
            })
        };
        let bound: Vec<NodeId> = crate::KEYBOARD_SHORTCUTS
            .iter()
            .map(|(node, _)| *node)
            .collect();
        let shortcut_bytes = encoded(&bound) - without_shortcuts(&bound);
        assert!(
            shortcut_bytes > 0 && shortcut_bytes <= 32 * crate::KEYBOARD_SHORTCUTS.len(),
            "{} shortcuts cost {shortcut_bytes} bytes",
            crate::KEYBOARD_SHORTCUTS.len()
        );
        let filter = encoded(&[crate::FILTER_ROW, crate::FILTER_INPUT, crate::FILTER_STATUS]);
        let search = encoded(&[
            crate::SEARCH_ROW,
            SEARCH_INPUT,
            crate::SEARCH_PREVIOUS,
            crate::SEARCH_NEXT,
            PID_INPUT,
            crate::PID_GO,
            SEARCH_STATUS,
        ]);
        let owner = encoded(&[
            crate::OWNER_ROW,
            crate::OWNER_MINE,
            crate::OWNER_ALL,
            OWNER_INPUT,
            PID_SET_INPUT,
            crate::OWNER_STATUS,
        ]);
        let mut structure = vec![
            crate::SURFACE,
            crate::COLUMN,
            crate::HEADING,
            crate::TABLE,
            crate::SORT_ROW,
            crate::COLUMN_CHOOSER_ROW,
        ];
        structure.extend(crate::SORT_BUTTONS.map(|(button, _)| button));
        structure.extend(crate::COLUMN_CHOOSER_BUTTONS.map(|(button, _)| button));
        let structure = encoded(&structure);
        for (group, size, reserve) in [
            ("filter controls", filter, FILTER_RESERVE_BYTES),
            ("search controls", search, SEARCH_RESERVE_BYTES),
            ("user filter", owner, OWNER_RESERVE_BYTES),
            ("shell structure", structure, SNAPSHOT_SHELL_BYTES),
        ] {
            assert!(
                size <= reserve,
                "the widest {group} encode to {size} bytes, above their {reserve} byte reserve"
            );
        }
        assert!(
            sampling <= SAMPLING_RESERVE_BYTES,
            "the widest sampling controls encode to {sampling} bytes, above their {SAMPLING_RESERVE_BYTES} byte reserve"
        );
        assert!(
            sampling > SAMPLING_RESERVE_BYTES / 2,
            "the reserve must be measured against sampling controls that really are wide: {sampling}"
        );
        assert!(
            follow <= FOLLOW_RESERVE_BYTES,
            "the widest follow controls encode to {follow} bytes, above their {FOLLOW_RESERVE_BYTES} byte reserve"
        );
        assert!(
            follow > FOLLOW_RESERVE_BYTES / 2,
            "the reserve must be measured against follow controls that really are wide: {follow}"
        );
        assert!(
            view <= VIEW_RESERVE_BYTES,
            "the widest view switch and tree encode to {view} bytes, above their {VIEW_RESERVE_BYTES} byte reserve"
        );
        assert!(
            view > VIEW_RESERVE_BYTES / 4,
            "the reserve must be measured against a view group that really is wide: {view}"
        );
        assert!(
            command <= COMMAND_RESERVE_BYTES,
            "the widest command line encodes to {command} bytes, above its {COMMAND_RESERVE_BYTES} byte reserve"
        );
        assert!(
            command > COMMAND_RESERVE_BYTES / 2,
            "the reserve must be measured against a command line that really is wide: {command}"
        );
        assert!(
            inspector <= INSPECTOR_RESERVE_BYTES,
            "the widest inspector encodes to {inspector} bytes, above its {INSPECTOR_RESERVE_BYTES} byte reserve"
        );
        assert!(
            inspector > INSPECTOR_RESERVE_BYTES / 2,
            "the reserve must be measured against an inspector that really is wide: {inspector}"
        );
        let hello = srui_protocol::ClientHello {
            core_version: "0.5.0".to_string(),
            profiles: vec!["org.srui.standard-widgets/1".to_string()],
            limits: None,
            client_instance_id: vec![1, 2, 3],
            client_metadata: Default::default(),
            known_resource_hashes: vec![],
        };
        let snapshot = session
            .bootstrap_fresh_client(&hello)
            .expect("a fresh client attaches")
            .snapshot
            .expect("a populated session sends a catch-up snapshot");
        let frame = srui_protocol::framed_payload_len(&srui_protocol::SruiMessage {
            msg: Some(srui_protocol::srui_message::Msg::Transaction(snapshot)),
        });
        assert!(
            frame
                <= SNAPSHOT_SHELL_BYTES
                    + STATUS_RESERVE_BYTES
                    + SUMMARY_RESERVE_BYTES
                    + FILTER_RESERVE_BYTES
                    + SEARCH_RESERVE_BYTES
                    + OWNER_RESERVE_BYTES
                    + INSPECTOR_RESERVE_BYTES
                    + COMMAND_RESERVE_BYTES
                    + SAMPLING_RESERVE_BYTES
                    + FOLLOW_RESERVE_BYTES
                    + VIEW_RESERVE_BYTES
                    + THREADS_RESERVE_BYTES
                    + TRANSACTION_ENVELOPE_BYTES,
            "a snapshot with no row is {frame} bytes, above the allowances the row budget \
             leaves for it"
        );
        println!(
            "PX-007 snapshot without rows: frame={frame} allowances={}",
            SNAPSHOT_SHELL_BYTES
                + STATUS_RESERVE_BYTES
                + SUMMARY_RESERVE_BYTES
                + FILTER_RESERVE_BYTES
                + SEARCH_RESERVE_BYTES
                + OWNER_RESERVE_BYTES
                + INSPECTOR_RESERVE_BYTES
                + COMMAND_RESERVE_BYTES
                + SAMPLING_RESERVE_BYTES
                + FOLLOW_RESERVE_BYTES
                + VIEW_RESERVE_BYTES
                + THREADS_RESERVE_BYTES
        );
        println!(
            "PX-018 view switch and tree at their widest: encoded={view} reserve={VIEW_RESERVE_BYTES}"
        );
        println!(
            "PX-019 thread controls at their widest: encoded={threads} reserve={THREADS_RESERVE_BYTES}"
        );
        println!(
            "PX-013 inspector at its widest: encoded={inspector} reserve={INSPECTOR_RESERVE_BYTES}"
        );
        println!(
            "PX-014 command line at its widest: encoded={command} reserve={COMMAND_RESERVE_BYTES}"
        );
        println!(
            "PX-015 sampling controls at their widest: encoded={sampling} reserve={SAMPLING_RESERVE_BYTES}"
        );
        println!(
            "PX-017 follow controls at their widest: encoded={follow} reserve={FOLLOW_RESERVE_BYTES}"
        );
        println!(
            "PX-016 shortcuts: {} bound, {shortcut_bytes} bytes; filter={filter}/{FILTER_RESERVE_BYTES} \
             search={search}/{SEARCH_RESERVE_BYTES} owner={owner}/{OWNER_RESERVE_BYTES} \
             structure={structure}/{SNAPSHOT_SHELL_BYTES}",
            crate::KEYBOARD_SHORTCUTS.len()
        );
    }

    /// The published-row ceiling is declared, not negotiated: a caller may lower
    /// it, and nothing can raise it above the model's own §26 item bound.
    #[test]
    fn the_published_row_ceiling_is_declared_and_can_only_be_lowered() {
        let session = srui_sessiond::Session::mint();
        let store_limit = session.with_store(|store| store.limits().max_cached_items_per_model);
        let (view, _) = ProcessView::start(&session, &mut ScriptedFakeSource::default()).unwrap();
        assert_eq!(view.published_limit(), MAX_PUBLISHED_ROWS);
        assert_eq!(
            MAX_PUBLISHED_ROWS, store_limit,
            "the ceiling is the model's own cached-item bound"
        );
        let raised = view.with_published_limit(usize::MAX);
        assert_eq!(
            raised.published_limit(),
            MAX_PUBLISHED_ROWS,
            "no caller may raise the ceiling"
        );
        let lowered = raised.with_published_limit(7);
        assert_eq!(lowered.published_limit(), 7);
        assert_eq!(
            lowered.with_published_limit(0).published_limit(),
            1,
            "a view always publishes at least one row"
        );
    }

    /// Eviction is a function of the published order alone: the unconfirmed rows
    /// kept are the leading ones, a confirmed row is never evicted, and a
    /// collection inside the bound loses nothing.
    #[test]
    fn eviction_keeps_the_leading_unconfirmed_rows_and_never_a_confirmed_one() {
        let rows = widest_rows(10);
        let ids = |rows: &[Row]| rows.iter().map(|row| row.item_id).collect::<Vec<ItemId>>();
        let confirmed: HashSet<ItemId> = [rows[1].item_id, rows[8].item_id].into_iter().collect();

        let mut inside = rows.clone();
        assert_eq!(
            evict_unconfirmed_beyond(&mut inside, &confirmed, 10).len(),
            0
        );
        assert_eq!(
            ids(&inside),
            ids(&rows),
            "nothing is evicted to meet a bound"
        );

        let mut bounded_rows = rows.clone();
        assert_eq!(
            evict_unconfirmed_beyond(&mut bounded_rows, &confirmed, 6).len(),
            4
        );
        assert_eq!(
            ids(&bounded_rows),
            vec![
                rows[0].item_id,
                rows[1].item_id,
                rows[2].item_id,
                rows[3].item_id,
                rows[4].item_id,
                rows[8].item_id,
            ],
            "the rows kept are the leading unconfirmed ones and every confirmed one"
        );
        // Repeating the same eviction is the same eviction: nothing here depends
        // on iteration order or on when a row was last seen.
        let mut again = rows.clone();
        assert_eq!(evict_unconfirmed_beyond(&mut again, &confirmed, 6).len(), 4);
        assert_eq!(ids(&again), ids(&bounded_rows));

        let every: HashSet<ItemId> = rows.iter().map(|row| row.item_id).collect();
        let mut confirmed_only = rows.clone();
        assert_eq!(
            evict_unconfirmed_beyond(&mut confirmed_only, &every, 3).len(),
            0,
            "a row this scan confirmed is never evicted"
        );
        assert_eq!(ids(&confirmed_only), ids(&rows));
    }

    /// A plan that does not fit in one transaction commits as several, and a
    /// plan that stops partway leaves the view holding exactly what the store
    /// holds — never a row it failed to publish.
    /// PX-011: while the table holds a promoted selection, a plan that may
    /// remove rows leaves one operation and its bytes free for the
    /// `CLEAR_PROPERTY` the SDK's settlement adds, a promotion weighs its two
    /// operations in chunking, and its planned bytes cover what is encoded.
    #[test]
    fn a_promotion_is_budgeted_as_what_it_records_and_what_settlement_adds() {
        let session = srui_sessiond::Session::mint();
        let (mut view, _) = ProcessView::start(&session, &mut FakeProcessSource).unwrap();
        let (_, max_ops) = mutation_limits(&session);
        let (_, ops, bytes) = view.planning_limits(&session);
        assert_eq!((ops, bytes), (max_ops, MAX_TRANSACTION_PAYLOAD_BYTES));
        view.jump_to_pid(&session, "4102").unwrap();
        assert_eq!(view.promoted, Some(view.rows[1].item_id));
        let (_, ops, bytes) = view.planning_limits(&session);
        assert_eq!(ops, max_ops - 1);
        assert_eq!(bytes, MAX_TRANSACTION_PAYLOAD_BYTES - PROMOTION_CLEAR_BYTES);

        let promote = Planned::new(Effect::Promote(TABLE, Box::new(view.rows[0].clone())));
        assert_eq!(promote.operations(), 2);
        let planned = promote.bytes;
        let revision = session.current_revision();
        let plan = vec![promote, Planned::new(Effect::SearchStatus("x".into()))];
        let (transactions, outcome) = commit(
            &session,
            &plan,
            2,
            MAX_TRANSACTION_PAYLOAD_BYTES,
            &mut view.published(),
        );
        assert!(outcome.is_ok());
        assert_eq!(
            transactions, 2,
            "a promotion fills a two-operation transaction"
        );
        assert_eq!(view.promoted, Some(view.rows[0].item_id));
        let committed = session.collect_replayed_transactions(revision).unwrap();
        assert_eq!(committed[0].operations.len(), 2);
        let encoded = srui_protocol::framed_payload_len(&srui_protocol::SruiMessage {
            msg: Some(srui_protocol::srui_message::Msg::Transaction(
                committed[0].clone(),
            )),
        });
        assert!(
            encoded <= planned + TRANSACTION_ENVELOPE_BYTES,
            "planned {planned} bytes but the encoder emitted {encoded}"
        );
    }

    #[test]
    fn a_plan_is_committed_in_bounded_transactions_and_recorded_as_it_commits() {
        let session = srui_sessiond::Session::mint();
        let mut source = ScriptedFakeSource::default();
        let (mut view, _) = ProcessView::start(&session, &mut source).unwrap();
        let template = view.rows[0].clone();
        let appended = |offset: u64| {
            Planned::new(Effect::Insert(
                3 + offset,
                vec![Row {
                    owner: crate::source::OWNER_NOT_COLLECTED,
                    lineage: crate::source::Lineage::NOT_COLLECTED,
                    kind: crate::source::KIND_NOT_COLLECTED,
                    tree: None,
                    key: template.key.clone(),
                    display_name: template.display_name.clone(),
                    item_id: ItemId::new(100 + offset),
                    cells: Vec::new(),
                    value: Value::List(vec![
                        Value::UnsignedInt(9000 + offset),
                        Value::String("appended".into()),
                        Value::String("0 B".into()),
                    ]),
                    sort: template.sort.clone(),
                }],
            ))
        };

        let plan: Vec<Planned> = (0..4).map(appended).collect();
        let (transactions, outcome) = commit(
            &session,
            &plan,
            2,
            MAX_TRANSACTION_PAYLOAD_BYTES,
            &mut view.published(),
        );
        assert!(outcome.is_ok());
        assert_eq!(transactions, 2, "four operations, two per transaction");
        assert_eq!(session.current_revision(), 3);
        assert_eq!(view.row_count(), 7);
        assert_eq!(rows_of(&view), store_rows(&session));

        // The second chunk reuses an item ID the model already holds, so its
        // transaction is refused whole.
        let mut plan: Vec<Planned> = (4..8).map(appended).collect();
        plan[2] = Planned::new(Effect::Insert(
            9,
            vec![Row {
                owner: crate::source::OWNER_NOT_COLLECTED,
                lineage: crate::source::Lineage::NOT_COLLECTED,
                tree: None,
                item_id: ItemId::new(100),
                ..template.clone()
            }],
        ));
        let (transactions, outcome) = commit(
            &session,
            &plan,
            2,
            MAX_TRANSACTION_PAYLOAD_BYTES,
            &mut view.published(),
        );
        assert!(outcome.is_err(), "a duplicate item ID must be refused");
        assert_eq!(transactions, 1);
        assert_eq!(view.row_count(), 9, "only the committed chunk is recorded");
        assert_eq!(rows_of(&view), store_rows(&session));
        assert_eq!(session.current_revision(), 4);
    }

    fn store_rows(session: &srui_sessiond::Session) -> Vec<(u64, Value)> {
        session.with_store(|store| {
            store
                .get_model(MODEL)
                .unwrap()
                .items
                .values()
                .map(|item| (item.item_id.get(), item.value.clone()))
                .collect()
        })
    }

    #[test]
    fn every_planned_operation_matches_the_effect_it_records() {
        let session = srui_sessiond::Session::mint();
        let mut source = ScriptedFakeSource::default();
        let (mut view, _) = ProcessView::start(&session, &mut source).unwrap();
        let published = rows_of(&view);
        for _ in 0..ScriptedFakeSource::STEPS * 3 {
            view.refresh(&session, &mut source).unwrap();
            // The view's record of the published rows is the store's own model.
            let model = session.with_store(|store| store.get_model(MODEL).cloned().unwrap());
            let held: Vec<(u64, Value)> = model
                .items
                .values()
                .map(|item| (item.item_id.get(), item.value.clone()))
                .collect();
            assert_eq!(rows_of(&view), held);
            assert_eq!(model.item_count, view.row_count() as u64);
            assert_eq!(
                view.status(),
                session.with_store(|store| {
                    let Some(Value::String(text)) =
                        store.get_node(STATUS).unwrap().get_property(TEXT).cloned()
                    else {
                        panic!("the status node must carry text")
                    };
                    text
                })
            );
            // And its record of the summary is the store's own (PX-007): every
            // property a refresh may set or clear, present or absent alike.
            session.with_store(|store| {
                for (id, _, _) in summary::LAYOUT {
                    let node = store.get_node(id).unwrap();
                    for property in [
                        TEXT,
                        srui_sdk::ROLE,
                        srui_sdk::VALUE,
                        srui_sdk::VALUE_DESCRIPTION,
                        srui_sdk::VISIBILITY,
                    ] {
                        assert_eq!(
                            node.get_property(property),
                            view.summary.get(&(id, property)),
                            "{id:?} {property:?}"
                        );
                    }
                }
            });
        }
        assert_ne!(rows_of(&view), published, "the script must move the rows");
    }

    #[test]
    fn the_rows_kept_in_place_are_one_longest_increasing_run() {
        let kept = |sequence: &[usize]| -> Vec<usize> {
            longest_increasing(sequence, sequence.len())
                .iter()
                .enumerate()
                .filter_map(|(rank, kept)| kept.then_some(rank))
                .collect()
        };
        assert_eq!(kept(&[0, 1, 2, 3]), [0, 1, 2, 3]);
        assert_eq!(kept(&[3, 2, 1, 0]).len(), 1);
        // One row that sank to the end moves alone, not the rows it passed.
        assert_eq!(kept(&[1, 2, 3, 0]), [1, 2, 3]);
        assert_eq!(kept(&[3, 0, 1, 2]), [0, 1, 2]);
        assert_eq!(kept(&[5, 0, 4, 1, 2, 3]), [0, 1, 2, 3]);
        assert_eq!(kept(&[]), Vec::<usize>::new());
    }

    /// `count` consecutive processes, PIDs 1 to `count`, as one snapshot.
    struct Numbered(u32);

    impl ProcessSource for Numbered {
        fn status_text(&self) -> &str {
            FAKE_STATUS_TEXT
        }

        fn snapshot(&mut self) -> ProcessSnapshot {
            let mut snapshot = FakeProcessSource.snapshot();
            let template = snapshot.records[0].clone();
            snapshot.records = (1..=self.0)
                .map(|pid| ProcessRecord {
                    kind: crate::source::KIND_NOT_COLLECTED,
                    lineage: crate::source::Lineage::NOT_COLLECTED,
                    key: ProcessKey {
                        pid: Observed::Known(pid),
                        creation: CreationToken::Opaque(format!("numbered-{pid}")),
                        thread: None,
                        ..template.key.clone()
                    },
                    ..template.clone()
                })
                .collect();
            snapshot
        }
    }

    /// The item IDs each committed transaction since `revision` deleted and
    /// inserted, and its operation count.
    fn committed_moves(
        session: &srui_sessiond::Session,
        revision: u64,
    ) -> Vec<(Vec<u64>, Vec<u64>, usize)> {
        use srui_protocol::operation::Op;
        session
            .collect_replayed_transactions(revision)
            .expect("the journal holds this run")
            .into_iter()
            .map(|transaction| {
                let mut deleted = Vec::new();
                let mut inserted = Vec::new();
                for operation in &transaction.operations {
                    match &operation.op {
                        Some(Op::ModelDelete(delete)) => deleted.extend(delete.item_ids.clone()),
                        Some(Op::ModelInsert(insert)) => {
                            inserted.extend(insert.items.iter().map(|item| item.item_id))
                        }
                        _ => {}
                    }
                }
                deleted.sort_unstable();
                inserted.sort_unstable();
                (deleted, inserted, transaction.operations.len())
            })
            .collect()
    }

    /// PX-009: a reorder larger than one transaction commits as several, each a
    /// complete set of moves — no revision a client can hold lacks a row — and
    /// the rows placed so far are already in their final order.
    #[test]
    fn a_reorder_larger_than_one_transaction_never_drops_a_row() {
        let session = srui_sessiond::Session::mint();
        let (mut view, _) = ProcessView::start(&session, &mut Numbered(40)).unwrap();
        let revision = session.current_revision();
        let mut target = view.rows.clone();
        target.reverse();
        // At most three items an operation and four operations a transaction:
        // a unit moves at most six rows (two deletions, two insertions).
        let moves = plan_moves(&view.rows, &target, 3, 4, MAX_TRANSACTION_PAYLOAD_BYTES);
        assert_eq!(
            moves.rows.iter().map(|row| row.item_id).collect::<Vec<_>>(),
            target.iter().map(|row| row.item_id).collect::<Vec<_>>()
        );
        // A reversal keeps exactly one row in place.
        let moved: usize = moves
            .plan
            .iter()
            .map(|planned| match &planned.effect {
                Effect::Insert(_, rows) => rows.len(),
                _ => 0,
            })
            .sum();
        assert_eq!(moved, 39);
        assert!(moves.plan.iter().all(|planned| planned.moving));
        // A status change ahead of the moves: it may not share a transaction
        // with a unit it would split.
        let mut plan = vec![Planned::new(Effect::Status("reordering".into()))];
        plan.extend(moves.plan);
        let (transactions, outcome) = commit(
            &session,
            &plan,
            4,
            MAX_TRANSACTION_PAYLOAD_BYTES,
            &mut view.published(),
        );
        assert!(outcome.is_ok());
        let committed = committed_moves(&session, revision);
        assert_eq!(committed.len(), transactions);
        assert_eq!(transactions, 1 + 39_usize.div_ceil(6));
        assert_eq!(
            committed[0],
            (vec![], vec![], 1),
            "the status travels alone"
        );
        for (deleted, inserted, operations) in &committed[1..] {
            assert!(*operations <= 4);
            assert!(!deleted.is_empty());
            assert_eq!(
                deleted, inserted,
                "a transaction put back every row it removed"
            );
        }
        assert_eq!(rows_of(&view), store_rows(&session));
        assert_eq!(
            view.item_ids(),
            target.iter().map(|row| row.item_id).collect::<Vec<_>>()
        );
    }

    /// PX-009: one row that changes place moves alone, inside one transaction,
    /// carrying its new value; the rows it passed are not touched.
    #[test]
    fn one_row_that_changes_place_moves_alone() {
        let session = srui_sessiond::Session::mint();
        let (view, _) = ProcessView::start(&session, &mut Numbered(6)).unwrap();
        let mut target = view.rows.clone();
        let mut last = target.remove(5);
        last.value = Value::String("changed".into());
        target.insert(1, last.clone());
        let moves = plan_moves(
            &view.rows,
            &target,
            1_000,
            1_000,
            MAX_TRANSACTION_PAYLOAD_BYTES,
        );
        assert_eq!(moves.plan.len(), 2);
        assert!(moves.plan[0].joins_next && !moves.plan[1].joins_next);
        assert!(matches!(&moves.plan[0].effect, Effect::Delete(ids) if ids == &[last.item_id]));
        assert!(
            matches!(&moves.plan[1].effect, Effect::Insert(1, rows) if rows == &[last.clone()])
        );
        assert_eq!(moves.rows, target);
        // Nothing left for the refresh diff to send: the move carried the value.
        assert!(diff(&moves.rows, &target, 1_000, MAX_TRANSACTION_PAYLOAD_BYTES).is_empty());
        // And rows that need no move plan nothing at all.
        assert!(plan_moves(
            &view.rows,
            &view.rows,
            1_000,
            1_000,
            MAX_TRANSACTION_PAYLOAD_BYTES
        )
        .plan
        .is_empty());
    }
}

/// PX-018, verification of Codex review round 1: what a plan that does not
/// commit leaves of the server's disclosure mirror and inspector, through
/// the test-only [`commit_fault`] seam, and the reveal rule's exact scope.
#[cfg(test)]
mod disclosure_commit_tests {
    use super::*;
    use crate::source::{
        BootId, CappedRecords, Completeness, CpuInterval, CpuUsage, CreationToken, DisplayName,
        FakeProcessSource, HostId, Lineage, Observed, PidNamespaceId, ProcessKey, ProcessRecord,
        SnapshotTime, SourceId, OWNER_NOT_COLLECTED,
    };

    /// The one client these tests report from.
    const CLIENT: &[u8] = b"disclosure-client";

    struct Family(Vec<ProcessRecord>);

    impl Family {
        fn new(records: &[(u32, &str, u32, u64)]) -> Self {
            Self(
                records
                    .iter()
                    .map(|&(pid, name, parent, started)| ProcessRecord {
                        lineage: Lineage::new(parent, started),
                        owner: OWNER_NOT_COLLECTED,
                        key: ProcessKey {
                            source: SourceId("disclosure-fixture".into()),
                            host: Observed::Known(HostId("h".into())),
                            boot: Observed::Known(BootId("b".into())),
                            pid_namespace: Observed::Known(PidNamespaceId(1)),
                            pid: Observed::Known(pid),
                            creation: CreationToken::Opaque(format!("{name}-{pid}")),
                            thread: None,
                        },
                        kind: crate::source::KIND_NOT_COLLECTED,
                        display_name: DisplayName::from(name),
                        resident: Observed::Known(4096),
                        cpu: CpuUsage::Measured(CpuInterval {
                            ticks: u64::from(pid),
                            ticks_per_second: 100,
                            elapsed: Duration::from_secs(1),
                        }),
                    })
                    .collect(),
            )
        }

        fn reparent(&mut self, pid: u32, parent: u32) {
            for record in &mut self.0 {
                if record.key.pid == Observed::Known(pid) {
                    if let Observed::Known(started) = record.lineage.started {
                        record.lineage = Lineage::new(parent, started);
                    }
                }
            }
        }
    }

    impl ProcessSource for Family {
        fn status_text(&self) -> &str {
            "Read-only · Disclosure fixture"
        }

        fn snapshot(&mut self) -> ProcessSnapshot {
            ProcessSnapshot {
                source: SourceId("disclosure-fixture".into()),
                sampled_at: SnapshotTime(SystemTime::UNIX_EPOCH),
                records: self.0.clone(),
                vanished: 0,
                capped: CappedRecords::none(),
                completeness: Completeness::Complete,
                system: FakeProcessSource::system(),
                threads: crate::source::ThreadScan::NotProvided,
            }
        }
    }

    fn tree(source: &mut Family) -> (Session, ProcessView) {
        let session = Session::mint();
        let (mut view, _) = ProcessView::start(&session, source).unwrap();
        view.set_view(&session, Some(true)).unwrap();
        (session, view)
    }

    fn item(view: &ProcessView, pid: u32) -> ItemId {
        view.rows
            .iter()
            .find(|row| row.key.pid == Observed::Known(pid))
            .map(|row| row.item_id)
            .unwrap_or_else(|| panic!("PID {pid} is shown"))
    }

    fn collapse(session: &Session, view: &mut ProcessView, pid: u32) {
        let item = item(view, pid);
        view.expansion_changed(session, CLIENT, TREE, item, false)
            .unwrap();
        assert!(
            view.collapsed_by(CLIENT).contains(&item),
            "PID {pid} collapsed"
        );
    }

    #[test]
    fn the_fault_seam_fails_exactly_the_armed_transaction() {
        let mut source = Family::new(&[(10, "alpha", 0, 10), (11, "beta", 10, 11)]);
        let (session, mut view) = tree(&mut source);
        let revision = session.current_revision();
        let armed = commit_fault::arm(0);
        assert!(view.set_view(&session, Some(false)).is_err());
        assert_eq!(session.current_revision(), revision, "nothing committed");
        assert_eq!(view.view_mode(), ViewMode::Tree);
        drop(armed);
        view.set_view(&session, Some(false)).unwrap();
        assert_eq!(view.view_mode(), ViewMode::Table);
    }

    /// F1: a reparent that keeps the preorder's item IDs in place
    /// (`[alpha, beta, xray]` both before and after `xray` moves under the
    /// collapsed `alpha`) and does not commit leaves the mirror and the
    /// inspector as they were: the client still shows `xray` selected.
    #[test]
    fn a_failed_reparent_that_keeps_the_preorder_ids_restores_the_inspector() {
        let mut source = Family::new(&[
            (10, "alpha", 0, 10),
            (11, "beta", 10, 11),
            (30, "xray", 0, 60),
        ]);
        let (session, mut view) = tree(&mut source);
        let xray = item(&view, 30);
        view.select_item_in(&session, CLIENT, TREE, Some(xray))
            .unwrap();
        collapse(&session, &mut view, 10);
        assert_eq!(view.inspector().selected(), Some(xray));
        let before: Vec<ItemId> = view.item_ids();
        source.reparent(30, 10);
        {
            let _armed = commit_fault::arm(0);
            assert!(view.refresh(&session, &mut source).is_err());
        }
        assert_eq!(view.item_ids(), before, "the preorder's IDs are the same");
        assert_eq!(
            view.inspector().selected(),
            Some(xray),
            "the client still shows xray selected"
        );
        assert!(view.collapsed_by(CLIENT).contains(&item(&view, 10)));
        // The next sample commits, and hides xray then.
        view.refresh(&session, &mut source).unwrap();
        assert_eq!(view.inspector().selected(), None);
    }

    /// F2: re-promoting the match already promoted (Go to PID again, after
    /// the user collapsed its parent) that does not commit leaves the parent
    /// collapsed in the mirror, as on the client.
    #[test]
    fn a_failed_repromotion_of_the_same_match_keeps_its_parent_collapsed() {
        let mut source = Family::new(&[
            (10, "alpha", 0, 10),
            (11, "beta", 10, 11),
            (30, "xray", 0, 60),
        ]);
        let (session, mut view) = tree(&mut source);
        let alpha = item(&view, 10);
        let beta = item(&view, 11);
        view.jump_to_pid(&session, "11").unwrap();
        assert_eq!(view.promoted_selection(), Some(beta));
        collapse(&session, &mut view, 10);
        {
            let _armed = commit_fault::arm(0);
            assert!(view.go_to_pid(&session).is_err());
        }
        assert_eq!(
            view.promoted_selection(),
            Some(beta),
            "still the old promotion"
        );
        assert!(
            view.collapsed_by(CLIENT).contains(&alpha),
            "the client keeps alpha collapsed"
        );
        // When the promotion commits, alpha is revealed.
        view.go_to_pid(&session).unwrap();
        assert!(!view.collapsed_by(CLIENT).contains(&alpha));
    }

    /// A view switch that does not commit (`republish_in`, with no entry
    /// before the switch's rows) leaves the mirror and the inspector as they
    /// were: the client still shows the tree, `alpha` collapsed and `xray`
    /// selected.
    #[test]
    fn a_failed_switch_away_from_the_tree_keeps_the_mirror_and_the_inspector() {
        let mut source = Family::new(&[
            (10, "alpha", 0, 10),
            (11, "beta", 10, 11),
            (30, "xray", 0, 60),
        ]);
        let (session, mut view) = tree(&mut source);
        let alpha = item(&view, 10);
        let xray = item(&view, 30);
        view.select_item_in(&session, CLIENT, TREE, Some(xray))
            .unwrap();
        collapse(&session, &mut view, 10);
        {
            let _armed = commit_fault::arm(0);
            assert!(view.set_view(&session, Some(false)).is_err());
        }
        assert_eq!(view.view_mode(), ViewMode::Tree);
        assert!(
            view.collapsed_by(CLIENT).contains(&alpha),
            "the client keeps alpha collapsed"
        );
        assert_eq!(view.inspector().selected(), Some(xray));
        // The switch that commits starts the table with nothing collapsed.
        view.set_view(&session, Some(false)).unwrap();
        assert!(view.collapsed_by(CLIENT).is_empty());
        assert_eq!(view.inspector().selected(), Some(xray));
    }

    /// A filter republish that does not commit leaves the mirror and the
    /// inspector as they were, although it would have pruned the collapsed
    /// `rho` it hides and cleared the inspector for the `xray` it moves
    /// under the collapsed `alpha`.
    #[test]
    fn a_failed_filter_republish_keeps_the_mirror_and_the_inspector() {
        let mut source = Family::new(&[
            (10, "alpha", 0, 10),
            (11, "beta", 10, 11),
            (12, "mid", 10, 12),
            (13, "xray", 12, 13),
            (20, "rho", 0, 20),
            (21, "sigma", 20, 21),
        ]);
        let (session, mut view) = tree(&mut source);
        view.set_filter(&session, "alpha | beta | xray | rho | sigma")
            .unwrap();
        let alpha = item(&view, 10);
        let rho = item(&view, 20);
        let xray = item(&view, 13);
        view.select_item_in(&session, CLIENT, TREE, Some(xray))
            .unwrap();
        collapse(&session, &mut view, 10);
        collapse(&session, &mut view, 20);
        let both: HashSet<ItemId> = [alpha, rho].into_iter().collect();
        assert_eq!(
            view.inspector().selected(),
            Some(xray),
            "xray is a shown root"
        );
        {
            // The filter's pending status commits first; its republish fails.
            let _armed = commit_fault::arm(1);
            assert!(view
                .set_filter(&session, "alpha | beta | mid | xray")
                .is_err());
        }
        assert_eq!(view.collapsed_by(CLIENT), both, "nothing pruned");
        assert_eq!(view.inspector().selected(), Some(xray), "nothing cleared");
        // The filter that commits hides rho and puts xray under alpha.
        view.set_filter(&session, "alpha | beta | mid | xray")
            .unwrap();
        assert_eq!(view.collapsed_by(CLIENT), [alpha].into_iter().collect());
        assert_eq!(view.inspector().selected(), None);
    }

    /// The reveal opens exactly the collapsed items whose subtree holds the
    /// match, at every level, and no collapsed sibling or other root.
    #[test]
    fn a_reveal_opens_only_the_collapsed_ancestors_of_its_match() {
        let mut source = Family::new(&[
            (1, "a", 0, 1),
            (2, "b", 1, 2),
            (3, "c", 2, 3),
            (4, "match", 3, 4),
            (5, "s", 1, 5),
            (6, "t", 5, 6),
            (7, "r", 0, 7),
            (8, "u", 7, 8),
        ]);
        let (session, mut view) = tree(&mut source);
        for pid in [3, 2, 5, 1, 7] {
            collapse(&session, &mut view, pid);
        }
        view.jump_to_pid(&session, "4").unwrap();
        let open: HashSet<ItemId> = [item(&view, 5), item(&view, 7)].into_iter().collect();
        assert_eq!(
            view.collapsed_by(CLIENT),
            open,
            "only the match's ancestors opened"
        );
    }

    /// Verification note V5-I1: a promotion that does not commit leaves the
    /// inspector on the selection the clients still show, not on the match
    /// its requester's restored mirror hides.
    #[test]
    fn a_failed_promotion_keeps_the_inspector_on_the_shown_selection() {
        let mut source = Family::new(&[
            (10, "alpha", 0, 10),
            (11, "beta", 10, 11),
            (30, "xray", 0, 60),
        ]);
        let (session, mut view) = tree(&mut source);
        let alpha = item(&view, 10);
        let xray = item(&view, 30);
        view.select_item_in(&session, CLIENT, TREE, Some(xray))
            .unwrap();
        collapse(&session, &mut view, 10);
        view.set_pid_input(&session, "11").unwrap();
        {
            let _armed = commit_fault::arm(0);
            assert!(view.go_to_pid_from(&session, CLIENT).is_err());
        }
        assert!(view.collapsed_by(CLIENT).contains(&alpha));
        assert_eq!(view.inspector().selected(), Some(xray), "still xray");
        // The promotion that commits selects beta and reveals it.
        view.go_to_pid_from(&session, CLIENT).unwrap();
        assert_eq!(view.inspector().selected(), Some(item(&view, 11)));
        assert!(view.collapsed_by(CLIENT).is_empty());
    }

    /// Another client instance, for the tests that need two.
    const OTHER: &[u8] = b"disclosure-other-client";

    /// Verification V6-L1 (N9): a promotion that does not commit records no
    /// reveal, so a delayed collapse above its match, which the client keeps
    /// because it never received the promotion, is mirrored.
    #[test]
    fn a_failed_promotion_records_no_reveal() {
        let mut source = Family::new(&[
            (10, "alpha", 0, 10),
            (11, "beta", 10, 11),
            (30, "xray", 0, 60),
        ]);
        let (session, mut view) = tree(&mut source);
        let alpha = item(&view, 10);
        let xray = item(&view, 30);
        view.select_item_in(&session, CLIENT, TREE, Some(xray))
            .unwrap();
        view.set_pid_input(&session, "11").unwrap();
        let before = session.current_revision();
        {
            let _armed = commit_fault::arm(0);
            assert!(view.go_to_pid_from(&session, CLIENT).is_err());
        }
        view.expansion_changed_at(&session, CLIENT, TREE, alpha, false, before)
            .unwrap();
        assert!(
            view.collapsed_by(CLIENT).contains(&alpha),
            "the client keeps alpha collapsed"
        );
    }

    /// Verification V6-L1 (N12): a promotion that does not commit puts back
    /// the selection's holder too, so the requester's collapse cannot hide
    /// the other client's selection the inspector still describes.
    #[test]
    fn a_failed_promotion_restores_the_holder() {
        let mut source = Family::new(&[
            (10, "alpha", 0, 10),
            (11, "beta", 10, 11),
            (12, "gamma", 10, 12),
        ]);
        let (session, mut view) = tree(&mut source);
        let gamma = item(&view, 12);
        view.select_item_in(&session, OTHER, TREE, Some(gamma))
            .unwrap();
        view.set_pid_input(&session, "11").unwrap();
        {
            let _armed = commit_fault::arm(0);
            assert!(view.go_to_pid_from(&session, CLIENT).is_err());
        }
        assert_eq!(view.inspector().selected(), Some(gamma));
        collapse(&session, &mut view, 10);
        assert_eq!(
            view.inspector().selected(),
            Some(gamma),
            "this client's collapse never hides the other client's selection"
        );
    }

    /// Verification V6-L1 (N14): only an item whose subtree holds the match
    /// is undone; a delayed collapse of an unrelated parent that comes before
    /// the match in preorder is mirrored.
    #[test]
    fn a_delayed_collapse_of_a_parent_before_the_match_is_mirrored() {
        let mut source = Family::new(&[
            (10, "alpha", 0, 10),
            (11, "beta", 10, 11),
            (20, "delta", 0, 20),
            (21, "echo", 20, 21),
        ]);
        let (session, mut view) = tree(&mut source);
        let alpha = item(&view, 10);
        view.set_pid_input(&session, "21").unwrap();
        let before = session.current_revision();
        view.go_to_pid_from(&session, CLIENT).unwrap();
        view.expansion_changed_at(&session, CLIENT, TREE, alpha, false, before)
            .unwrap();
        assert!(
            view.collapsed_by(CLIENT).contains(&alpha),
            "alpha does not hold echo"
        );
    }

    /// Codex review round 4: the per-client mirror is bounded. A client with
    /// nothing collapsed holds no entry; past [`MAX_DISCLOSURE_CLIENTS`] the
    /// least recently reporting client is evicted (and mirrored all
    /// expanded); leaving the tree forgets every client.
    #[test]
    fn the_per_client_mirror_is_bounded_and_closed() {
        let mut source = Family::new(&[(10, "alpha", 0, 10), (11, "beta", 10, 11)]);
        let (session, mut view) = tree(&mut source);
        let alpha = item(&view, 10);
        let beta = item(&view, 11);
        let client = |index: usize| format!("client-{index}").into_bytes();
        for index in 0..MAX_DISCLOSURE_CLIENTS {
            view.expansion_changed(&session, &client(index), TREE, alpha, false)
                .unwrap();
        }
        assert_eq!(
            view.disclosure_clients(),
            MAX_DISCLOSURE_CLIENTS,
            "none evicted at the bound"
        );
        // The first client reports again, so the second is now the least
        // recently reporting one, and the next new client evicts it.
        view.expansion_changed(&session, &client(0), TREE, beta, false)
            .unwrap();
        view.expansion_changed(
            &session,
            &client(MAX_DISCLOSURE_CLIENTS),
            TREE,
            alpha,
            false,
        )
        .unwrap();
        assert_eq!(view.disclosure_clients(), MAX_DISCLOSURE_CLIENTS);
        assert!(
            view.collapsed_by(&client(1)).is_empty(),
            "the least recently reporting evicted"
        );
        assert_eq!(
            view.collapsed_by(&client(0)),
            HashSet::from([alpha, beta]),
            "a client that reported again is kept"
        );
        assert!(view.collapsed_by(&client(2)).contains(&alpha));
        let last = client(MAX_DISCLOSURE_CLIENTS);
        assert!(view.collapsed_by(&last).contains(&alpha));
        // Expanding its last collapsed item drops the client's entry.
        view.expansion_changed(&session, &last, TREE, alpha, true)
            .unwrap();
        assert_eq!(view.disclosure_clients(), MAX_DISCLOSURE_CLIENTS - 1);
        // An expansion from a client never mirrored adds nothing.
        view.expansion_changed(&session, b"never-collapsed", TREE, alpha, true)
            .unwrap();
        assert_eq!(view.disclosure_clients(), MAX_DISCLOSURE_CLIENTS - 1);
        view.set_view(&session, Some(false)).unwrap();
        assert_eq!(view.disclosure_clients(), 0);
    }
}

/// PX-019 on PX-018's disclosure mirror: what a thread request, or a sample
/// that hides a selected thread, leaves behind when its plan does not
/// commit, through the test-only [`commit_fault`] seam.
#[cfg(test)]
mod thread_commit_tests {
    use super::*;
    use crate::source::{
        BootId, CappedRecords, Completeness, CpuUsage, CreationToken, DisplayName,
        FakeProcessSource, HostId, Lineage, Observed, PidNamespaceId, ProcessKey, ProcessRecord,
        SnapshotTime, SourceId, TaskKind, ThreadIdentity, ThreadRecords, ThreadScan, ThreadsSource,
        OWNER_NOT_COLLECTED,
    };

    /// The one client these tests report from (PX-018's per-client mirror).
    const CLIENT: &[u8] = b"thread-client";

    fn item(view: &ProcessView, pid: u32, tid: Option<u32>) -> ItemId {
        view.rows
            .iter()
            .find(|row| {
                row.key.pid == Observed::Known(pid)
                    && row.key.thread.as_ref().map(|thread| thread.tid) == tid
            })
            .map(|row| row.item_id)
            .unwrap_or_else(|| panic!("{pid}/{tid:?} is shown"))
    }

    fn threads_shown() -> (Session, ThreadsSource, ProcessView) {
        let session = Session::mint();
        let mut source = ThreadsSource::default();
        let (mut view, _) = ProcessView::start(&session, &mut source).unwrap();
        view.set_user_threads(&session, Some(true)).unwrap();
        view.refresh(&session, &mut source).unwrap();
        (session, source, view)
    }

    #[test]
    fn a_failed_hide_of_user_threads_keeps_the_setting_and_the_inspector() {
        let (session, _source, mut view) = threads_shown();
        let worker = item(&view, 200, Some(201));
        view.select_item_in(&session, CLIENT, TABLE, Some(worker))
            .unwrap();
        assert_eq!(view.inspector().selected(), Some(worker));
        {
            let _armed = commit_fault::arm(0);
            assert!(view.set_user_threads(&session, Some(false)).is_err());
        }
        assert!(
            view.threads_requested(),
            "the checkbox never changed on the client"
        );
        assert_eq!(
            view.inspector().selected(),
            Some(worker),
            "still selected there"
        );
        assert!(view.rows.iter().any(|row| row.item_id == worker));
        view.set_user_threads(&session, Some(false)).unwrap();
        assert!(!view.threads_requested());
        assert_eq!(view.inspector().selected(), None);
        assert!(view.rows.iter().all(|row| !row.key.is_thread()));
    }

    #[test]
    fn a_failed_hide_of_kernel_threads_keeps_the_setting() {
        let (session, _source, mut view) = threads_shown();
        let shown = view.item_ids();
        {
            let _armed = commit_fault::arm(0);
            assert!(view.set_kernel_threads(&session, Some(false)).is_err());
        }
        assert!(view.thread_settings().kernel);
        assert_eq!(view.item_ids(), shown);
        view.set_kernel_threads(&session, Some(false)).unwrap();
        assert!(!view.thread_settings().kernel);
        assert_eq!(view.item_ids().len(), shown.len() - 2);
    }

    /// `p` with two threads, which a later sample moves under `q`.
    struct Movable {
        moved: bool,
    }

    impl Movable {
        fn process(pid: u32, name: &str, parent: u32) -> ProcessRecord {
            ProcessRecord {
                lineage: Lineage::new(parent, u64::from(pid)),
                owner: OWNER_NOT_COLLECTED,
                key: ProcessKey {
                    source: SourceId("movable-threads".into()),
                    host: Observed::Known(HostId("h".into())),
                    boot: Observed::Known(BootId("b".into())),
                    pid_namespace: Observed::Known(PidNamespaceId(1)),
                    pid: Observed::Known(pid),
                    creation: CreationToken::Opaque(format!("{name}-{pid}")),
                    thread: None,
                },
                display_name: DisplayName::from(name),
                resident: Observed::Known(4096),
                cpu: CpuUsage::WarmingUp,
                kind: Observed::Known(TaskKind::Process {
                    threads: Observed::Known(2),
                }),
            }
        }
    }

    impl ProcessSource for Movable {
        fn status_text(&self) -> &str {
            "Read-only · Movable thread fixture"
        }

        fn snapshot(&mut self) -> ProcessSnapshot {
            let p = Self::process(20, "p", if self.moved { 10 } else { 1 });
            let mut read = ThreadRecords::with_limit(16);
            read.records = [(20, "p"), (21, "p-worker")]
                .into_iter()
                .map(|(tid, name)| ProcessRecord {
                    key: ProcessKey {
                        thread: Some(ThreadIdentity {
                            tid,
                            creation: CreationToken::Opaque(format!("t{tid}")),
                        }),
                        ..p.key.clone()
                    },
                    display_name: DisplayName::from(name),
                    resident: Observed::Missing(crate::source::MissingReason::Unavailable),
                    kind: Observed::Known(TaskKind::Thread),
                    ..p.clone()
                })
                .collect();
            ProcessSnapshot {
                source: SourceId("movable-threads".into()),
                sampled_at: SnapshotTime(SystemTime::UNIX_EPOCH),
                records: vec![
                    Self::process(1, "init", 0),
                    Self::process(10, "q", 1),
                    Self::process(11, "q-child", 10),
                    p,
                ],
                vanished: 0,
                capped: CappedRecords::none(),
                completeness: Completeness::Complete,
                system: FakeProcessSource::system(),
                threads: ThreadScan::Read(read),
            }
        }
    }

    /// A sample that would move a selected thread, with its process, under a
    /// parent the client keeps collapsed and does not commit leaves the
    /// mirror and the inspector as they were; the one that commits clears
    /// the inspector. The thread rows are part of the rows the plan's
    /// `rows_end` waits for.
    #[test]
    fn a_failed_sample_that_would_hide_a_selected_thread_restores_the_inspector() {
        let session = Session::mint();
        let mut source = Movable { moved: false };
        let (mut view, _) = ProcessView::start(&session, &mut source).unwrap();
        view.set_user_threads(&session, Some(true)).unwrap();
        view.refresh(&session, &mut source).unwrap();
        view.set_view(&session, Some(true)).unwrap();
        let worker = item(&view, 20, Some(21));
        let q = item(&view, 10, None);
        view.select_item_in(&session, CLIENT, TREE, Some(worker))
            .unwrap();
        view.expansion_changed(&session, CLIENT, TREE, q, false)
            .unwrap();
        assert_eq!(view.inspector().selected(), Some(worker));
        source.moved = true;
        {
            let _armed = commit_fault::arm(0);
            assert!(view.refresh(&session, &mut source).is_err());
        }
        assert_eq!(
            view.inspector().selected(),
            Some(worker),
            "still shown there"
        );
        assert!(view.collapsed_by(CLIENT).contains(&q));
        view.refresh(&session, &mut source).unwrap();
        assert_eq!(view.inspector().selected(), None, "hidden under q now");
        assert!(view.collapsed_by(CLIENT).contains(&q));
        let depth = |row_item: ItemId| {
            view.rows
                .iter()
                .find(|row| row.item_id == row_item)
                .and_then(|row| row.tree.as_ref())
                .map(|place| place.depth)
        };
        assert_eq!(depth(worker), Some(3), "under p, under q");
    }
}
