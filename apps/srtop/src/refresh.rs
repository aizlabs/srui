//! Periodic incremental refresh of the published process collection
//! (design §§8, 12.1, 13, 23; PX-004). The semantic node tree is built once,
//! by [`crate::initialize_from_source`]; every later tick emits nothing but
//! model mutations and — only when it actually changed — the status text.
//!
//! Three rules shape this module:
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

use crate::projection::{Row, SessionItemIds};
use crate::source::{ProcessSnapshot, ProcessSource, Retention};
use crate::{initialize_rows, published_status, MODEL, STATUS};
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

/// What one refresh published.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Refreshed {
    pub inserted: usize,
    pub deleted: usize,
    pub updated: usize,
    /// Rows the scan did not confirm and that were kept rather than deleted.
    pub retained: usize,
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
        let rows = ids.project(&snapshot)?;
        let status = published_status(&source_status, &snapshot);
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
        let confirmed_count = confirmed.len();
        let target = retain_unconfirmed(&self.rows, confirmed, &retention);
        let retained = target.len() - confirmed_count;
        let status = refresh_status(source_status, snapshot, retained, rejected.as_deref());

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
        plan.extend(diff(&self.rows, &target, batch));

        let mut result = Refreshed {
            retained,
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

        let (transactions, outcome) =
            commit(session, &plan, max_ops, &mut self.rows, &mut self.status);
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

/// Commits a plan as consecutive transactions of at most `max_ops` operations.
///
/// §26 bounds the operations one transaction may carry, and a refresh of a large
/// collection can exceed it. Each transaction is still atomic, and each is
/// recorded into `rows` and `status` as it commits, so a plan that stops partway
/// leaves this app's idea of what the client holds equal to what the store
/// holds. Returns how many transactions committed and the first failure.
fn commit(
    session: &Session,
    plan: &[Planned],
    max_ops: usize,
    rows: &mut Vec<Row>,
    status: &mut String,
) -> (usize, Result<(), SessionError>) {
    let mut committed = 0;
    for chunk in plan.chunks(max_ops) {
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
    }
    (committed, Ok(()))
}

/// The published status: what the source says it read, what the scan could not
/// see, and how many rows on screen the scan did not confirm.
///
/// Every clause is derived from counts and from fixed wording. No record's own
/// text ever reaches the status line, so a process cannot write into it.
pub fn refresh_status(
    source_status: &str,
    snapshot: &ProcessSnapshot,
    retained: usize,
    rejected: Option<&str>,
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
    status
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

/// One planned mutation: the wire operation and the effect it has on the
/// published rows, derived from one description so the two cannot disagree.
struct Planned {
    op: Operation,
    effect: Effect,
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
        Self { op, effect }
    }
}

impl Effect {
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
fn diff(published: &[Row], target: &[Row], batch: usize) -> Vec<Planned> {
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

    let mut plan = Vec::new();
    for chunk in deleted.chunks(batch) {
        plan.push(Planned::new(Effect::Delete(chunk.to_vec())));
    }

    let mut run_start: u64 = 0;
    let mut run: Vec<Row> = Vec::new();
    for (index, row) in target.iter().enumerate() {
        if retained.contains(&row.item_id) {
            flush_insert(&mut plan, run_start, &mut run);
            continue;
        }
        if run.is_empty() {
            run_start = index as u64;
        }
        run.push(row.clone());
        if run.len() == batch {
            flush_insert(&mut plan, run_start, &mut run);
        }
    }
    flush_insert(&mut plan, run_start, &mut run);

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
    for chunk in changed.chunks(batch) {
        plan.push(Planned::new(Effect::Update(chunk.to_vec())));
    }
    plan
}

fn flush_insert(plan: &mut Vec<Planned>, start: u64, rows: &mut Vec<Row>) {
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
        Completeness, EnumerationIssue, IssueScope, MissingReason, Observed, ProcessKey,
        ScriptedFakeSource,
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
        assert_eq!(refresh_status(label, &snapshot, 0, None), label);
        assert_eq!(
            refresh_status(label, &snapshot, 1, None),
            format!("{label} · 1 row retained from an earlier scan")
        );
        assert_eq!(
            refresh_status(
                label,
                &snapshot,
                2,
                Some("duplicate process instance identity")
            ),
            format!(
                "{label} · snapshot rejected: duplicate process instance identity · \
                 2 rows retained from an earlier scan"
            )
        );
        let mut failed = snapshot.clone();
        failed.records.clear();
        failed.completeness = Completeness::from_scan(
            0,
            vec![EnumerationIssue {
                scope: IssueScope::Root,
                reason: MissingReason::Denied,
                detail: "scripted failure".into(),
            }],
        );
        assert_eq!(
            refresh_status(label, &failed, 3, None),
            format!(
                "{label} · incomplete scan · process list unavailable: permission denied · \
                 3 rows retained from an earlier scan"
            )
        );
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
        let (transactions, outcome) = commit(&session, &plan, 2, &mut view.rows, &mut view.status);
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
        let (transactions, outcome) = commit(&session, &plan, 2, &mut view.rows, &mut view.status);
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
