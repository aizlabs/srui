//! Periodic incremental refresh of the published process collection
//! (design §§8, 12.1, 13, 23; PX-004) and of the system summary beside it
//! (PX-007). The semantic node tree is built once, by
//! [`crate::initialize_from_source`]; every later tick emits nothing but model
//! mutations and — only when they actually changed — the status text and the
//! summary's scalar properties ([`crate::summary`]). Since PX-009 the
//! collection can also be sorted ([`crate::sort`]): a sort request reorders the
//! published rows, and every later refresh publishes in the active order. A row
//! that changes place keeps its item identity: it is moved, never replaced —
//! see [`plan_moves`].
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
use crate::projection::{Row, SessionItemIds};
use crate::sort::{self, SortOrder};
use crate::source::{ProcessSnapshot, ProcessSource, Retention};
use crate::summary::{self, CollectorError, Sample};
use crate::{
    bounded, initialize_rows, published_status_from, sort_button_of, ColumnAction, ScanReport,
    COLUMN_CHOOSER_BUTTONS, FILTER_INPUT, FILTER_STATUS, MAX_PUBLISHED_LABEL_BYTES, MODEL, STATUS,
    TABLE,
};
use srui_protocol::DEFAULT_MAX_FRAME_SIZE;
use srui_sdk::{ItemId, NodeId, Operation, Value, COLUMNS, ENABLED, LABEL, TEXT, VALUE};
use srui_semantic_tree::{
    DEFAULT_MAX_CACHED_ITEMS_PER_MODEL, DEFAULT_MAX_ITEMS_PER_MODEL_OPERATION,
};
use srui_sessiond::{Session, SessionError};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
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
    MODEL_ITEM_FRAMING_BYTES + value_wire_bytes(&row.value)
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
        }
    }

    /// Publishes the shell and the first snapshot in one transaction, and keeps
    /// the state a later [`Self::refresh`] diffs against.
    pub fn start(
        session: &Session,
        source: &mut impl ProcessSource,
    ) -> Result<(Self, ProcessSnapshot), Box<dyn std::error::Error>> {
        let source_status = source.status_text().to_string();
        let snapshot = source.snapshot();
        let mut ids = SessionItemIds::default();
        // The shell starts in the default layout (PX-068).
        let columns = ColumnLayout::default();
        let unfiltered_rows = ids.project(&snapshot, &columns)?;
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
        let mut rows = unfiltered_rows.clone();
        let truncated = bound_to_publishable_collection(&mut rows, published_limit);
        let status = refresh_status(
            &source_status,
            &snapshot,
            &RefreshCounts {
                truncated,
                ..RefreshCounts::default()
            },
        );
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
        initialize_rows(
            session,
            rows.iter().map(Row::to_model_item).collect(),
            &status,
            &summary,
            true,
        )?;
        ids.retain(&unfiltered_rows);
        let committed_filter = FilterQuery::default();
        let committed_generation = FilterGeneration::INITIAL;
        let filter_status = committed_filter.format_committed_status(
            committed_generation,
            rows.len(),
            unfiltered_rows.len(),
            unfiltered_rows.len(),
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

    /// Samples the source once and publishes the difference.
    pub fn refresh(
        &mut self,
        session: &Session,
        source: &mut impl ProcessSource,
    ) -> Result<Refreshed, SessionError> {
        let source_status = source.status_text().to_string();
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
        let mut unfiltered_target =
            retain_unconfirmed(&self.unfiltered_rows, confirmed, &retention);
        // Once a sort is active the target is published in it, confirmed and
        // retained rows alike: a retained row sorts under the values it was
        // published with, not last as though its values were missing. The order
        // is the view's at publication time, never the one in force when the
        // snapshot was sampled (PX-009).
        if let Some(order) = self.sort {
            unfiltered_target.sort_by(|left, right| sort::compare(order, left, right));
        }
        // Retention is bounded by construction, whatever a scan can and cannot
        // enumerate: the rows carried forward are evicted down to what the
        // published-row ceiling leaves once this scan's own rows are counted.
        let evicted =
            evict_unconfirmed_beyond(&mut unfiltered_target, &confirmed_ids, self.published_limit);
        let (target, matched, filter_truncated) = project_visible_rows(
            &unfiltered_target,
            &self.committed_filter,
            self.published_limit,
        );
        let filter_matches = if self.committed_filter.is_empty() {
            None
        } else {
            Some(matched)
        };
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
        let status = refresh_status(
            source_status,
            snapshot,
            &RefreshCounts {
                retained,
                rejected,
                evicted,
                truncated,
            },
        );
        debug_assert!(status.len() <= STATUS_RESERVE_BYTES);
        let filter_status = self.committed_filter.format_committed_status(
            self.committed_generation,
            target.len(),
            matched,
            unfiltered_target.len(),
        );

        let (batch, max_ops) = mutation_limits(session);

        let mut plan = Vec::new();
        if status != self.status {
            // First, so a scan that failed explains itself even if a later batch
            // of the same refresh does not commit.
            plan.push(Planned::new(Effect::Status(status)));
        }
        if filter_status != self.filter_status {
            plan.push(Planned::new(Effect::FilterStatus(filter_status)));
        }
        // Then the summary, whose lines are scalar properties: only the ones a
        // client would see change are planned (PX-007).
        plan.extend(summary_effects(&self.summary, &summary));
        // Then the rows that changed place, each moved under its own item ID
        // (PX-009), and only then what really appeared, ended or changed.
        let moved = plan_moves(
            &self.rows,
            &target,
            batch,
            max_ops,
            MAX_TRANSACTION_PAYLOAD_BYTES,
        );
        plan.extend(moved.plan);
        plan.extend(diff(
            &moved.rows,
            &target,
            batch,
            MAX_TRANSACTION_PAYLOAD_BYTES,
        ));

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
            return Ok(result);
        }

        let (transactions, outcome) = commit(
            session,
            &plan,
            max_ops,
            MAX_TRANSACTION_PAYLOAD_BYTES,
            &mut self.published(),
        );
        result.transactions = transactions;
        // Order values never reach the wire, so every published row takes this
        // sample's whatever committed: a later click sorts by current values,
        // never by the ones a row was first published with.
        refresh_sort_values(&mut self.rows, &target);
        refresh_sort_values(&mut self.unfiltered_rows, &unfiltered_target);
        self.unfiltered_rows = unfiltered_target;
        self.ids.retain(&self.unfiltered_rows);
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
    /// (§6.2, §8). The rows are the ones already published; nothing is sampled,
    /// and the published set does not change, only its order. A request for the
    /// order already published commits nothing at all.
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
        let (target, _, _) = project_visible_rows(
            &self.unfiltered_rows,
            &self.committed_filter,
            self.published_limit,
        );
        let (batch, max_ops) = mutation_limits(session);
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
        let moved = plan_moves(
            &self.rows,
            &target,
            batch,
            max_ops,
            MAX_TRANSACTION_PAYLOAD_BYTES,
        );
        debug_assert_eq!(
            moved.rows.iter().map(|row| row.item_id).collect::<Vec<_>>(),
            target.iter().map(|row| row.item_id).collect::<Vec<_>>()
        );
        plan.extend(moved.plan);
        let mut result = Refreshed::default();
        count(&plan, &mut result);
        let (transactions, outcome) = commit(
            session,
            &plan,
            max_ops,
            MAX_TRANSACTION_PAYLOAD_BYTES,
            &mut self.published(),
        );
        result.transactions = transactions;
        refresh_sort_values(&mut self.rows, &target);
        self.ids.retain(&self.unfiltered_rows);
        outcome.map(|()| result)
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
        let (batch, max_ops) = mutation_limits(session);
        let mut plan = vec![Planned::new(Effect::Columns(layout.clone()))];
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
        plan.extend(diff(
            &self.rows,
            &target,
            batch,
            MAX_TRANSACTION_PAYLOAD_BYTES,
        ));
        let bytes: usize = plan.iter().map(|planned| planned.bytes).sum();
        if plan.len() <= max_ops && bytes <= MAX_TRANSACTION_PAYLOAD_BYTES {
            let last = plan.len() - 1;
            for planned in &mut plan[..last] {
                planned.joins_next = true;
            }
        }
        let mut result = Refreshed::default();
        count(&plan, &mut result);
        debug_assert_eq!(
            (result.inserted, result.deleted, result.moved),
            (0, 0, 0),
            "a layout change only updates rows in place"
        );
        let (transactions, outcome) = commit(
            session,
            &plan,
            max_ops,
            MAX_TRANSACTION_PAYLOAD_BYTES,
            &mut self.published(),
        );
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
        let status = refresh_status(
            source_status,
            snapshot,
            &RefreshCounts {
                retained,
                rejected,
                ..RefreshCounts::default()
            },
        );
        debug_assert!(status.len() <= STATUS_RESERVE_BYTES);
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
            self.unfiltered_rows.len(),
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

        let (target, matched, _) = project_visible_rows(
            &self.unfiltered_rows,
            &self.committed_filter,
            self.published_limit,
        );

        let filter_matches = if self.committed_filter.is_empty() {
            None
        } else {
            Some(matched)
        };
        let summary = self.summarize_with_matches(filter_matches);

        let new_filter_status = self.committed_filter.format_committed_status(
            self.committed_generation,
            target.len(),
            matched,
            self.unfiltered_rows.len(),
        );

        let (batch, max_ops) = mutation_limits(session);
        let mut plan = Vec::new();

        let current_input_val = session.with_store(|store| {
            store
                .get_node(FILTER_INPUT)
                .and_then(|n| n.get_property(VALUE))
                .and_then(Value::as_string)
                .unwrap_or("")
                .to_string()
        });
        if current_input_val != self.committed_filter.raw() {
            plan.push(Planned::new(Effect::FilterValue(
                self.committed_filter.raw().to_string(),
            )));
        }

        if new_filter_status != self.filter_status {
            plan.push(Planned::new(Effect::FilterStatus(new_filter_status)));
        }
        plan.extend(summary_effects(&self.summary, &summary));

        let moved = plan_moves(
            &self.rows,
            &target,
            batch,
            max_ops,
            MAX_TRANSACTION_PAYLOAD_BYTES,
        );
        plan.extend(moved.plan);
        plan.extend(diff(
            &moved.rows,
            &target,
            batch,
            MAX_TRANSACTION_PAYLOAD_BYTES,
        ));

        let mut result = Refreshed::default();
        count(&plan, &mut result);

        if plan.is_empty() {
            refresh_sort_values(&mut self.rows, &target);
            self.ids.retain(&self.unfiltered_rows);
            return Ok(result);
        }

        let (transactions, outcome) = commit(
            session,
            &plan,
            max_ops,
            MAX_TRANSACTION_PAYLOAD_BYTES,
            &mut self.published(),
        );
        result.transactions = transactions;
        refresh_sort_values(&mut self.rows, &target);
        self.ids.retain(&self.unfiltered_rows);
        outcome.map(|()| result)
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
}

/// Filters `unfiltered` rows according to `filter`, preserving their active order,
/// and bounds them to `limit`. Returns the matching visible rows, the total match count,
/// and the count of truncated matching rows beyond `limit`.
fn project_visible_rows(
    unfiltered: &[Row],
    filter: &FilterQuery,
    limit: usize,
) -> (Vec<Row>, usize, usize) {
    let mut matching = Vec::new();
    let mut total_matches = 0;
    for row in unfiltered {
        if filter.matches_row(row) {
            total_matches += 1;
            matching.push(row.clone());
        }
    }
    let truncated = bound_to_publishable_collection(&mut matching, limit);
    (matching, total_matches, truncated)
}

/// Dispatches a `TEXT_EDIT` event on the filter input to the shared view (PX-010).
pub fn filter_edited(
    view: &std::sync::Mutex<ProcessView>,
    session: &Session,
    event: &srui_protocol::Event,
) {
    let text = srui_semantic_tree::Event::try_from(event.clone())
        .ok()
        .and_then(|e| e.text_arg().map(|s| s.to_string()))
        .unwrap_or_else(|| {
            session
                .with_store(|store| {
                    store.get_node(crate::FILTER_INPUT).and_then(|node| {
                        match node.properties.get(&srui_sdk::VALUE) {
                            Some(srui_sdk::Value::String(s)) => Some(s.clone()),
                            _ => None,
                        }
                    })
                })
                .unwrap_or_default()
        });
    if let Err(error) = lock_view(view).handle_client_text_edit(session, &text) {
        eprintln!("srtop: filter not published: {error}");
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
            Effect::Columns(_) | Effect::ChooserLabel { .. } | Effect::ChooserEnabled { .. } => {
                result.controls += 1
            }
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
    let mut committed = 0;
    let mut start = 0;
    while start < plan.len() {
        let mut end = start;
        let mut bytes = 0;
        // At least one operation always travels, so a plan always makes
        // progress; the planner keeps any single operation inside `max_bytes`.
        while end < plan.len() && end - start < max_ops {
            let cost = plan[end].bytes;
            if end > start && bytes + cost > max_bytes {
                break;
            }
            bytes += cost;
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
            for planned in chunk {
                ui.apply_op(&planned.op)?;
            }
            Ok(())
        }) {
            Ok(()) => {
                record_committed(chunk, published);
                committed += 1;
            }
            Err(error) => return (committed, Err(error)),
        }
        start = end;
    }
    (committed, Ok(()))
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
/// the command filter controls [`FILTER_RESERVE_BYTES`] rather than their own (PX-010), so
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
        SNAPSHOT_SHELL_BYTES + STATUS_RESERVE_BYTES + SUMMARY_RESERVE_BYTES + FILTER_RESERVE_BYTES,
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
        let mut cost = MODEL_ITEM_FRAMING_BYTES
            + VALUE_FRAMING_BYTES
            + row.cells.iter().map(value_wire_bytes).sum::<usize>();
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
) -> usize {
    if rows.len() <= limit {
        return 0;
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
    if evicted.is_empty() {
        return 0;
    }
    rows.retain(|row| !evicted.contains(&row.item_id));
    evicted.len()
}

/// Keeps rows the scan did not confirm and could not account for, in the
/// position they already hold relative to the rows it did confirm.
///
/// `retention` decides which unconfirmed rows those are: a scan that named the
/// records it skipped keeps only those, so an unrelated process that really
/// ended is still deleted while one denied record is unreadable. Only a scan
/// whose uncertainty cannot be enumerated at all keeps every absent row.
///
/// A retained row is anchored to the confirmed row that followed it, so a
/// scan that misses a record for one tick does not move that row to the end of
/// the table and back again on the next.
fn retain_unconfirmed(published: &[Row], confirmed: Vec<Row>, retention: &Retention) -> Vec<Row> {
    let position: HashMap<ItemId, usize> = confirmed
        .iter()
        .enumerate()
        .map(|(index, row)| (row.item_id, index))
        .collect();
    let uncertain = |row: &Row| !position.contains_key(&row.item_id) && retention.keeps(&row.key);
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
            None if !retention.keeps(&row.key) => {}
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
            Self::ChooserEnabled { .. } => VALUE_FRAMING_BYTES + SCALAR_VALUE_BYTES,
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
        } = published;
        match self {
            Self::Status(text) => status.clone_from(text),
            Self::FilterStatus(text) => filter_status.clone_from(text),
            Self::FilterValue(_) | Self::ChooserLabel { .. } | Self::ChooserEnabled { .. } => {}
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
                        row.cells = update.cells.clone();
                        row.sort = update.sort.clone();
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
fn refresh_sort_values(rows: &mut [Row], target: &[Row]) {
    let current: HashMap<ItemId, &Row> = target.iter().map(|row| (row.item_id, row)).collect();
    for row in rows.iter_mut() {
        if let Some(latest) = current.get(&row.item_id) {
            if row.sort != latest.sort {
                row.sort = latest.sort.clone();
            }
            if row.cells != latest.cells {
                row.cells = latest.cells.clone();
            }
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

/// Polls `source` until `shutdown` is cancelled, publishing each difference.
///
/// A failed sample never ends the loop and never empties the table: the error is
/// published over the last-known rows and the next successful scan converges.
///
/// The view is locked only to publish, after the sample is taken, and is
/// published in the order it holds at that moment (PX-009): a sort request
/// applied while a scan was running is never overwritten by that scan's
/// result, which is published in the newer order.
pub async fn poll<S>(
    view: SharedView,
    mut source: S,
    session: Arc<Session>,
    interval: Duration,
    shutdown: CancellationToken,
) where
    S: ProcessSource + Send + 'static,
{
    let mut ticker = tokio::time::interval(interval);
    // A slow scan must not make the next ticks fire back to back.
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // The first tick is immediate, and the first snapshot is already published.
    ticker.tick().await;
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return,
            _ = ticker.tick() => {}
        }
        // Reading a host's process filesystem blocks; keep it off the runtime.
        let sampled = tokio::task::spawn_blocking(move || {
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
        if let Err(error) = lock_view(&view).apply(&session, &status, &snapshot) {
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
                    ..template.clone()
                };
                // The cells are the widest missing words, so the order values
                // are missing in both metric columns (PX-009).
                let record = ProcessRecord {
                    key: key.clone(),
                    display_name: name.clone(),
                    resident: Observed::Missing(MissingReason::Unavailable),
                    cpu: crate::source::CpuUsage::Missing(MissingReason::Unavailable),
                };
                Row {
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
                    key: ProcessKey {
                        pid: Observed::Known(index + 1),
                        creation: CreationToken::Opaque(format!("host-{index}")),
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
                + FILTER_RESERVE_BYTES);
        let mut published = Vec::new();
        for system in [
            SystemSample::not_provided(),
            FakeProcessSource::system(),
            widest,
        ] {
            let session = srui_sessiond::Session::mint();
            let mut source = HostWith(Host::new(58_000, 48), system);
            let (view, snapshot) = ProcessView::start(&session, &mut source).unwrap();
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
        crate::initialize_rows(&session, Vec::new(), &status, &widest_summary(), false).unwrap();
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
        assert_eq!(evict_unconfirmed_beyond(&mut inside, &confirmed, 10), 0);
        assert_eq!(
            ids(&inside),
            ids(&rows),
            "nothing is evicted to meet a bound"
        );

        let mut bounded_rows = rows.clone();
        assert_eq!(
            evict_unconfirmed_beyond(&mut bounded_rows, &confirmed, 6),
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
        assert_eq!(evict_unconfirmed_beyond(&mut again, &confirmed, 6), 4);
        assert_eq!(ids(&again), ids(&bounded_rows));

        let every: HashSet<ItemId> = rows.iter().map(|row| row.item_id).collect();
        let mut confirmed_only = rows.clone();
        assert_eq!(
            evict_unconfirmed_beyond(&mut confirmed_only, &every, 3),
            0,
            "a row this scan confirmed is never evicted"
        );
        assert_eq!(ids(&confirmed_only), ids(&rows));
    }

    /// A plan that does not fit in one transaction commits as several, and a
    /// plan that stops partway leaves the view holding exactly what the store
    /// holds — never a row it failed to publish.
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
                    key: ProcessKey {
                        pid: Observed::Known(pid),
                        creation: CreationToken::Opaque(format!("numbered-{pid}")),
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
