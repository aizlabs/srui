//! Periodic incremental refresh of the published process collection
//! (design §§8, 12.1, 13, 23; PX-004). The semantic node tree is built once,
//! by [`crate::initialize_from_source`]; every later tick emits nothing but
//! model mutations and — only when it actually changed — the status text.
//!
//! Four rules shape this module:
//!
//! * A tick that observed no user-visible change opens no transaction at all.
//!   Nothing that changes on its own — the sample time, an issue count, a
//!   sequence number — is ever serialized, so an idle host produces an idle
//!   wire (§12.2).
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

use crate::projection::{Row, SessionItemIds};
use crate::source::{ProcessSnapshot, ProcessSource, Retention};
use crate::{initialize_rows, published_status, MODEL, STATUS};
use srui_protocol::DEFAULT_MAX_FRAME_SIZE;
use srui_sdk::{ItemId, Operation, Value, TEXT};
use srui_semantic_tree::DEFAULT_MAX_ITEMS_PER_MODEL_OPERATION;
use srui_sessiond::{Session, SessionError};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;
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
/// The shell is five nodes with fixed labels and a two-column header, so the
/// real cost is a few hundred bytes; 4 KiB is far above every encoding of them
/// and leaves the ceiling insensitive to a later label. The status text is the
/// one property a scan can lengthen, so it is charged separately and exactly.
const SNAPSHOT_SHELL_BYTES: usize = 4096;

/// Bytes one `MODEL_RESET_RANGE` operation of a snapshot costs beyond its items.
/// A snapshot emits one per [`DEFAULT_MAX_ITEMS_PER_MODEL_OPERATION`] rows.
const SNAPSHOT_RANGE_BYTES: usize = OPERATION_FRAMING_BYTES + OPERATION_HEADER_BYTES;

/// Bytes reserved for the status clause that truncation adds: fixed wording and
/// one decimal count. Reserving it rather than measuring it keeps the budget
/// independent of the count it produces.
const TRUNCATION_CLAUSE_BYTES: usize = 64;

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
    /// Rows the scan did not confirm and that were kept rather than deleted.
    pub retained: usize,
    /// Rows this app declined to publish because the resulting collection would
    /// not fit in one catch-up snapshot frame (§18, §26).
    pub truncated: usize,
    pub status_changed: bool,
    /// Transactions this refresh committed. Zero means the snapshot held no
    /// user-visible change and nothing was sent.
    pub transactions: usize,
}

impl Refreshed {
    pub fn published(&self) -> bool {
        self.transactions > 0
    }
}

/// The rows and status this session has published, and the identity allocator
/// that owns their item IDs.
pub struct ProcessView {
    ids: SessionItemIds,
    rows: Vec<Row>,
    status: String,
}

impl ProcessView {
    /// Publishes the shell and the first snapshot in one transaction, and keeps
    /// the state a later [`Self::refresh`] diffs against.
    pub fn start(
        session: &Session,
        source: &mut impl ProcessSource,
    ) -> Result<(Self, ProcessSnapshot), Box<dyn std::error::Error>> {
        let source_status = source.status_text().to_string();
        let snapshot = source.snapshot();
        let mut ids = SessionItemIds::default();
        let mut rows = ids.project(&snapshot)?;
        // The start path is exactly the attach path the ceiling exists for: the
        // first publication is one transaction, and every client that attaches
        // later is brought up by one snapshot of this same model.
        let provisional = refresh_status(&source_status, &snapshot, 0, None, 0);
        let truncated =
            bound_to_snapshot_frame(&mut rows, provisional.len() + TRUNCATION_CLAUSE_BYTES);
        let status = refresh_status(&source_status, &snapshot, 0, None, truncated);
        debug_assert!(status.len() <= provisional.len() + TRUNCATION_CLAUSE_BYTES);
        initialize_rows(
            session,
            rows.iter().map(Row::to_model_item).collect(),
            &status,
        )?;
        ids.retain(&rows);
        Ok((Self { ids, rows, status }, snapshot))
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
        let (confirmed, rejected) = match self.ids.project(snapshot) {
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
        let mut target = retain_unconfirmed(&self.rows, confirmed, &retention);
        let unconfirmed_in = |rows: &[Row]| {
            rows.iter()
                .filter(|row| !confirmed_ids.contains(&row.item_id))
                .count()
        };
        // A client receives the status and the rows in the same catch-up
        // snapshot, so the status is charged against the frame before the rows
        // are. The clause truncation adds is charged a fixed allowance instead
        // of being measured, so the count it states cannot move the budget that
        // produced it; every other clause can only shrink when rows are dropped.
        let provisional = refresh_status(
            source_status,
            snapshot,
            unconfirmed_in(&target),
            rejected.as_deref(),
            0,
        );
        let truncated =
            bound_to_snapshot_frame(&mut target, provisional.len() + TRUNCATION_CLAUSE_BYTES);
        let retained = unconfirmed_in(&target);
        let status = refresh_status(
            source_status,
            snapshot,
            retained,
            rejected.as_deref(),
            truncated,
        );
        debug_assert!(status.len() <= provisional.len() + TRUNCATION_CLAUSE_BYTES);

        let (batch, max_ops) = session.with_store(|store| {
            let limits = store.limits();
            (
                // Both bounds apply: this store's own limit, and the wire
                // default every conforming decoder enforces (§26).
                limits
                    .max_items_per_model_operation
                    .clamp(1, DEFAULT_MAX_ITEMS_PER_MODEL_OPERATION),
                limits.max_transaction_operations.max(1),
            )
        });

        let mut plan = Vec::new();
        if status != self.status {
            // First, so a scan that failed explains itself even if a later batch
            // of the same refresh does not commit.
            plan.push(Planned::new(Effect::Status(status)));
        }
        plan.extend(diff(
            &self.rows,
            &target,
            batch,
            MAX_TRANSACTION_PAYLOAD_BYTES,
        ));

        let mut result = Refreshed {
            retained,
            truncated,
            ..Refreshed::default()
        };
        for planned in &plan {
            match &planned.effect {
                Effect::Status(_) => result.status_changed = true,
                Effect::Delete(ids) => result.deleted += ids.len(),
                Effect::Insert(_, rows) => result.inserted += rows.len(),
                Effect::Update(rows) => result.updated += rows.len(),
            }
        }
        if plan.is_empty() {
            // Nothing a client can see changed: no transaction is opened, so an
            // unchanging host produces no traffic at all. The rows stand, so the
            // identities worth keeping are still exactly theirs.
            self.ids.retain(&self.rows);
            return Ok(result);
        }

        let (transactions, outcome) = commit(
            session,
            &plan,
            max_ops,
            MAX_TRANSACTION_PAYLOAD_BYTES,
            &mut self.rows,
            &mut self.status,
        );
        result.transactions = transactions;
        self.ids.retain(&self.rows);
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
/// each is recorded into `rows` and `status` as it commits, so a plan that stops
/// partway leaves this app's idea of what the client holds equal to what the
/// store holds. Returns how many transactions committed and the first failure.
fn commit(
    session: &Session,
    plan: &[Planned],
    max_ops: usize,
    max_bytes: usize,
    rows: &mut Vec<Row>,
    status: &mut String,
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
        let chunk = &plan[start..end];
        match session.transaction(|ui| {
            for planned in chunk {
                ui.apply_op(&planned.op)?;
            }
            Ok(())
        }) {
            Ok(()) => {
                record_committed(chunk, rows, status);
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
pub fn refresh_status(
    source_status: &str,
    snapshot: &ProcessSnapshot,
    retained: usize,
    rejected: Option<&str>,
    truncated: usize,
) -> String {
    let mut status = published_status(source_status, snapshot);
    if let Some(reason) = rejected {
        let _ = write!(status, " · snapshot rejected: {reason}");
    }
    if retained > 0 {
        let _ = write!(
            status,
            " · {retained} {} retained from an earlier scan",
            if retained == 1 { "row" } else { "rows" }
        );
    }
    if truncated > 0 {
        let _ = write!(
            status,
            " · {truncated} {} beyond the publishable size limit",
            if truncated == 1 { "row" } else { "rows" }
        );
    }
    status
}

/// Drops the trailing rows a catch-up snapshot of the resulting model could not
/// carry, and returns how many were dropped (§18, §26).
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
/// The rows kept are the *leading* ones, in the order the source already
/// publishes — `ProcFsSource` sorts by ascending PID — so the published window
/// is a deterministic function of the snapshot alone. An unchanged host keeps
/// exactly the rows it kept last tick: no row churns in and out, and an
/// identical tick still publishes nothing.
fn bound_to_snapshot_frame(rows: &mut Vec<Row>, status_bytes: usize) -> usize {
    let budget = MAX_TRANSACTION_PAYLOAD_BYTES.saturating_sub(SNAPSHOT_SHELL_BYTES + status_bytes);
    let mut spent = 0usize;
    let mut kept = 0usize;
    for row in rows.iter() {
        let mut cost = row_wire_bytes(row);
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
}

enum Effect {
    Status(String),
    Delete(Vec<ItemId>),
    Insert(u64, Vec<Row>),
    Update(Vec<Row>),
}

impl Planned {
    fn new(effect: Effect) -> Self {
        let op = match &effect {
            Effect::Status(text) => {
                Operation::set_property(STATUS, TEXT, Value::String(text.clone()))
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
        Self { op, effect, bytes }
    }
}

impl Effect {
    /// An upper bound on the bytes this effect's operation body carries beyond
    /// its fixed header.
    fn payload_bytes(&self) -> usize {
        match self {
            Self::Status(text) => VALUE_FRAMING_BYTES + text.len(),
            Self::Delete(ids) => ids.len() * ITEM_ID_BYTES,
            Self::Insert(_, rows) | Self::Update(rows) => rows.iter().map(row_wire_bytes).sum(),
        }
    }

    /// Applies this effect to the view's record of what the client holds. Called
    /// only after the transaction carrying the matching operation committed.
    fn record(&self, rows: &mut Vec<Row>, status: &mut String) {
        match self {
            Self::Status(text) => status.clone_from(text),
            Self::Delete(ids) => {
                let gone: HashSet<ItemId> = ids.iter().copied().collect();
                rows.retain(|row| !gone.contains(&row.item_id));
            }
            Self::Insert(index, inserted) => {
                let at = (*index as usize).min(rows.len());
                rows.splice(at..at, inserted.iter().cloned());
            }
            Self::Update(updated) => {
                let values: HashMap<ItemId, &Value> = updated
                    .iter()
                    .map(|row| (row.item_id, &row.value))
                    .collect();
                for row in rows.iter_mut() {
                    if let Some(value) = values.get(&row.item_id) {
                        row.value = (*value).clone();
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
fn record_committed(chunk: &[Planned], rows: &mut Vec<Row>, status: &mut String) {
    let mut index = 0;
    while index < chunk.len() {
        if matches!(chunk[index].effect, Effect::Insert(..)) {
            let start = index;
            while matches!(
                chunk.get(index).map(|planned| &planned.effect),
                Some(Effect::Insert(..))
            ) {
                index += 1;
            }
            insert_run(rows, &chunk[start..index]);
            continue;
        }
        chunk[index].effect.record(rows, status);
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

/// Polls `source` until `shutdown` is cancelled, publishing each difference.
///
/// A failed sample never ends the loop and never empties the table: the error is
/// published over the last-known rows and the next successful scan converges.
pub async fn poll<S>(
    mut view: ProcessView,
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
        if let Err(error) = view.apply(&session, &status, &snapshot) {
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
        Completeness, DisplayName, EnumerationIssue, IssueScope, MissingReason, Observed,
        ProcessKey, ScriptedFakeSource, SkippedRecords, MAX_DISPLAY_NAME_CHARS,
    };
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
        let first = ids.project(&source.snapshot()).unwrap();
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

    /// `rows` rows carrying the widest name this app can publish, with item IDs
    /// no fixture in this module has already published.
    fn widest_rows(rows: u64) -> Vec<Row> {
        let name = widest_display_name();
        let template = crate::source::FakeProcessSource.snapshot().records[0]
            .key
            .clone();
        (0..rows)
            .map(|index| Row {
                key: ProcessKey {
                    pid: Observed::Known(index as u32 + 1),
                    creation: crate::source::CreationToken::Opaque(format!("widest-{index}")),
                    ..template.clone()
                },
                item_id: ItemId::new(1_000 + index),
                value: Value::List(vec![
                    Value::UnsignedInt(index + 1),
                    Value::String(name.as_str().to_string()),
                ]),
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
        let (transactions, outcome) = commit(
            &session,
            &plan,
            1_000,
            ceiling,
            &mut view.rows,
            &mut view.status,
        );
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
        let first = ids.project(&source.snapshot()).unwrap();
        let merged = retain_unconfirmed(&first, vec![first[1].clone()], &everything());
        assert_eq!(keyed(&merged), keyed(&first));
        let merged = retain_unconfirmed(&first, Vec::new(), &everything());
        assert_eq!(
            keyed(&merged),
            keyed(&first),
            "a scan that saw nothing changes nothing"
        );
    }

    #[test]
    fn a_status_clause_is_added_only_for_what_actually_happened() {
        let mut source = ScriptedFakeSource::default();
        let snapshot = source.snapshot();
        let label = ScriptedFakeSource::STATUS_TEXT;
        assert_eq!(refresh_status(label, &snapshot, 0, None, 0), label);
        assert_eq!(
            refresh_status(label, &snapshot, 1, None, 0),
            format!("{label} · 1 row retained from an earlier scan")
        );
        assert_eq!(
            refresh_status(
                label,
                &snapshot,
                2,
                Some("duplicate process instance identity"),
                0
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
            refresh_status(label, &snapshot, 0, None, 1),
            format!("{label} · 1 row beyond the publishable size limit")
        );
        let truncated = refresh_status(label, &snapshot, 2, None, 7);
        assert_eq!(
            truncated,
            format!(
                "{label} · 2 rows retained from an earlier scan · \
                 7 rows beyond the publishable size limit"
            )
        );
        assert!(!truncated.contains("unreadable"), "{truncated}");
        assert!(!truncated.contains("record limit"), "{truncated}");
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
            refresh_status(label, &failed, 3, None, 0),
            format!(
                "{label} · incomplete scan · process list unavailable: permission denied · \
                 3 rows retained from an earlier scan"
            )
        );
    }

    /// The clause truncation adds is charged a fixed allowance before the rows
    /// are budgeted, so that allowance must cover the widest count it can ever
    /// state — otherwise the status could outgrow the frame the rows were
    /// chosen to fit.
    #[test]
    fn the_truncation_clause_never_exceeds_the_allowance_reserved_for_it() {
        let mut source = ScriptedFakeSource::default();
        let snapshot = source.snapshot();
        let label = ScriptedFakeSource::STATUS_TEXT;
        let base = refresh_status(label, &snapshot, 0, None, 0);
        for truncated in [1usize, 9, 6_683, usize::MAX] {
            let status = refresh_status(label, &snapshot, 0, None, truncated);
            assert!(
                status.len() - base.len() <= TRUNCATION_CLAUSE_BYTES,
                "{truncated} rows cost {} bytes of status, above the {TRUNCATION_CLAUSE_BYTES} \
                 reserved for the clause",
                status.len() - base.len()
            );
        }
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
                    item_id: ItemId::new(100 + offset),
                    value: Value::List(vec![
                        Value::UnsignedInt(9000 + offset),
                        Value::String("appended".into()),
                    ]),
                }],
            ))
        };

        let plan: Vec<Planned> = (0..4).map(appended).collect();
        let (transactions, outcome) = commit(
            &session,
            &plan,
            2,
            MAX_TRANSACTION_PAYLOAD_BYTES,
            &mut view.rows,
            &mut view.status,
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
            &mut view.rows,
            &mut view.status,
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
        }
        assert_ne!(rows_of(&view), published, "the script must move the rows");
    }
}
