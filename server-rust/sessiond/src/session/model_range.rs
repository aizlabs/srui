//! Sparse collection range fulfillment (§8, §12.1, §22.7).
//!
//! Clients request missing model windows with `ClientModelRangeRequest` on the `.ui` lane.
//! The request is idempotent: it is not journaled, not replayed, and not an Event.
//! Nearby pending windows for one model merge; a far jump replaces older pending
//! windows. `observed_revision` in the future is refused; a behind revision is
//! fulfilled against current store state. Successful fulfillment is an authoritative
//! `MODEL_RESET_RANGE` transaction, which is revisioned, journaled, broadcast, and
//! replayable. Server-initiated hydration uses the same commit path via
//! [`Session::push_visible_model_range`].

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use futures::task::AtomicWaker;
use srui_protocol::ClientModelRangeRequest;
use srui_sdk::UiTransaction;
use srui_semantic_tree::{
    AuthoritativeCommit, Model, ModelId, ModelItem, NodeId, Operation, TypeRef,
};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use super::{lock_or_recover, Session, SessionError, SessionInner};

/// Owned query passed to a registered range provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRangeQuery {
    pub node_id: NodeId,
    pub model_id: ModelId,
    pub start_index: u64,
    pub count: u64,
    pub observed_revision: u64,
}

/// Asynchronous provider that materializes a requested window without requiring the full
/// logical collection in memory (§8).
pub type ModelRangeProvider = Arc<
    dyn Fn(
            ModelRangeQuery,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<ModelItem>, ModelRangeError>> + Send>>
        + Send
        + Sync,
>;

/// Outcome of a range request that was well-formed enough to consider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelRangeFulfillment {
    /// An authoritative `MODEL_RESET_RANGE` transaction was committed.
    Committed { new_revision: u64 },
    /// Every requested index is already cached; the provider was not invoked.
    AlreadyCached,
    /// The authoritative revision changed while the provider ran; the result was discarded.
    Stale,
    /// No provider is registered for the model; nothing was committed.
    NoProvider,
}

/// Typed failures for range-request validation and fulfillment. None of these panic.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ModelRangeError {
    #[error("invalid node_id {0}")]
    InvalidNodeId(u64),
    #[error("invalid model_id {0}")]
    InvalidModelId(u64),
    #[error("count must be greater than zero")]
    CountZero,
    #[error("start_index {start} + count {count} overflows")]
    IndexOverflow { start: u64, count: u64 },
    #[error("node {0:?} does not exist")]
    NodeNotFound(NodeId),
    #[error("node {0:?} is not a List, Table, or Tree")]
    NotACollectionNode(NodeId),
    #[error("node {node:?} model_ref {actual:?} does not match requested {requested:?}")]
    ModelRefMismatch {
        node: NodeId,
        requested: ModelId,
        actual: Option<ModelId>,
    },
    #[error("model {0:?} does not exist")]
    ModelNotFound(ModelId),
    #[error("range [{start}, {end}) is outside item_count {item_count}")]
    OutOfBounds {
        start: u64,
        end: u64,
        item_count: u64,
    },
    #[error("count {count} exceeds max_items_per_model_operation {limit}")]
    CountExceedsLimit { count: u64, limit: usize },
    #[error("observed_revision {observed} is ahead of authoritative revision {actual}")]
    StaleRevision { observed: u64, actual: u64 },
    #[error("provider returned {actual} items for requested count {expected}")]
    ProviderItemCountMismatch { expected: u64, actual: usize },
    #[error("provider failed: {0}")]
    Provider(String),
    #[error("session error: {0}")]
    Session(String),
}

impl From<SessionError> for ModelRangeError {
    fn from(error: SessionError) -> Self {
        Self::Session(error.to_string())
    }
}

fn is_collection_type(node_type: TypeRef) -> bool {
    node_type == TypeRef::LIST || node_type == TypeRef::TABLE || node_type == TypeRef::TREE
}

fn range_is_cached(model: &Model, start: u64, count: u64) -> bool {
    (0..count).all(|offset| model.contains_index(start + offset))
}

fn validate_request(
    inner: &SessionInner,
    request: &ClientModelRangeRequest,
) -> Result<ModelRangeQuery, ModelRangeError> {
    if request.node_id == 0 {
        return Err(ModelRangeError::InvalidNodeId(request.node_id));
    }
    if request.model_id == 0 {
        return Err(ModelRangeError::InvalidModelId(request.model_id));
    }
    if request.count == 0 {
        return Err(ModelRangeError::CountZero);
    }
    let end =
        request
            .start_index
            .checked_add(request.count)
            .ok_or(ModelRangeError::IndexOverflow {
                start: request.start_index,
                count: request.count,
            })?;

    let node_id = NodeId::new(request.node_id);
    let model_id = ModelId::new(request.model_id);
    let node = inner
        .store
        .get_node(node_id)
        .ok_or(ModelRangeError::NodeNotFound(node_id))?;
    if !is_collection_type(node.node_type) {
        return Err(ModelRangeError::NotACollectionNode(node_id));
    }
    let actual_ref = node.model_ref();
    if actual_ref != Some(model_id) {
        return Err(ModelRangeError::ModelRefMismatch {
            node: node_id,
            requested: model_id,
            actual: actual_ref,
        });
    }
    let model = inner
        .store
        .get_model(model_id)
        .ok_or(ModelRangeError::ModelNotFound(model_id))?;
    let item_count = model.item_count();
    if end > item_count {
        return Err(ModelRangeError::OutOfBounds {
            start: request.start_index,
            end,
            item_count,
        });
    }
    let limit = inner.store.limits().max_items_per_model_operation;
    if request.count as usize > limit {
        return Err(ModelRangeError::CountExceedsLimit {
            count: request.count,
            limit,
        });
    }
    let actual_revision = inner.store.revision().get();
    if request.observed_revision > actual_revision {
        return Err(ModelRangeError::StaleRevision {
            observed: request.observed_revision,
            actual: actual_revision,
        });
    }

    Ok(ModelRangeQuery {
        node_id,
        model_id,
        start_index: request.start_index,
        count: request.count,
        observed_revision: request.observed_revision,
    })
}

impl Session {
    /// Registers the provider that materializes windows for `model_id` (§8).
    pub fn register_model_range_provider(&self, model_id: ModelId, provider: ModelRangeProvider) {
        let mut guard = lock_or_recover(&self.inner);
        guard.model_range_providers.insert(model_id, provider);
    }

    /// Validates `request`, optionally invokes the registered provider, and commits
    /// `MODEL_RESET_RANGE` when the result is still current (§8, §12.1).
    pub async fn fulfill_model_range_request(
        &self,
        request: ClientModelRangeRequest,
    ) -> Result<ModelRangeFulfillment, ModelRangeError> {
        let (query, provider) = {
            let guard = lock_or_recover(&self.inner);
            let query = validate_request(&guard, &request)?;
            if let Some(model) = guard.store.get_model(query.model_id) {
                if range_is_cached(model, query.start_index, query.count) {
                    return Ok(ModelRangeFulfillment::AlreadyCached);
                }
            }
            let Some(provider) = guard.model_range_providers.get(&query.model_id).cloned() else {
                return Ok(ModelRangeFulfillment::NoProvider);
            };
            (query, provider)
        };

        let items = provider(query.clone()).await?;
        if items.len() as u64 != query.count {
            return Err(ModelRangeError::ProviderItemCountMismatch {
                expected: query.count,
                actual: items.len(),
            });
        }
        self.commit_reset_if_current(&query, items)
    }

    /// Server-initiated hydration of a visible window. Uses the same fulfillment path as a
    /// client request, with `observed_revision` equal to the current authoritative revision.
    pub async fn push_visible_model_range(
        &self,
        node_id: NodeId,
        model_id: ModelId,
        start_index: u64,
        count: u64,
    ) -> Result<ModelRangeFulfillment, ModelRangeError> {
        let observed_revision = self.current_revision();
        self.fulfill_model_range_request(ClientModelRangeRequest {
            node_id: node_id.get(),
            model_id: model_id.get(),
            start_index,
            count,
            observed_revision,
        })
        .await
    }

    fn commit_reset_if_current(
        &self,
        query: &ModelRangeQuery,
        items: Vec<ModelItem>,
    ) -> Result<ModelRangeFulfillment, ModelRangeError> {
        let mut guard = lock_or_recover(&self.inner);
        match validate_request(
            &guard,
            &ClientModelRangeRequest {
                node_id: query.node_id.get(),
                model_id: query.model_id.get(),
                start_index: query.start_index,
                count: query.count,
                observed_revision: query.observed_revision,
            },
        ) {
            Ok(_) => {}
            Err(ModelRangeError::StaleRevision { .. })
            | Err(ModelRangeError::OutOfBounds { .. })
            | Err(ModelRangeError::NodeNotFound(_))
            | Err(ModelRangeError::ModelNotFound(_))
            | Err(ModelRangeError::ModelRefMismatch { .. }) => {
                return Ok(ModelRangeFulfillment::Stale);
            }
            Err(error) => return Err(error),
        }
        if let Some(model) = guard.store.get_model(query.model_id) {
            if range_is_cached(model, query.start_index, query.count) {
                return Ok(ModelRangeFulfillment::AlreadyCached);
            }
        }

        let op = Operation::model_reset_range(query.model_id, query.start_index, items, None);
        commit_ops_locked(self, &mut guard, vec![op])?;
        Ok(ModelRangeFulfillment::Committed {
            new_revision: guard.store.revision().get(),
        })
    }
}

fn commit_ops_locked(
    session: &Session,
    guard: &mut SessionInner,
    ops: Vec<Operation>,
) -> Result<(), ModelRangeError> {
    let base_revision = guard.store.revision();
    let max_ops = guard.store.limits().max_transaction_operations;
    let mut ui = UiTransaction::new(guard.store.clone_staging(), max_ops);
    for op in &ops {
        ui.apply_op(op)
            .map_err(|error| ModelRangeError::Session(SessionError::Store(error).to_string()))?;
    }
    let (staged, recorded) = ui.into_staged_and_ops();
    let commit = AuthoritativeCommit::new(base_revision, recorded);
    let permit = guard.journal.prepare(&commit).map_err(SessionError::from)?;
    let tx_wire = permit.transaction().clone();
    guard.store.commit_staging(staged, commit.new_revision());
    guard.journal.append(permit);
    session.publish_committed(&tx_wire);
    Ok(())
}

/// Per-connection coalescing inbox for [`ClientModelRangeRequest`].
///
/// Nearby or overlapping windows for one model merge into a single pending span so a
/// fragmented viewport is not dropped. A far jump replaces older pending windows (scroll).
/// A single worker drains the inbox so scroll traffic cannot spawn unbounded tasks.
const MAX_PENDING_RANGES_PER_MODEL: usize = 8;
/// Merge pending windows when the bounding span is at most two tracker pages.
const COALESCE_SPAN: u64 = 256;
const MAX_MERGED_COUNT: u64 = 10_000;

#[derive(Clone)]
pub struct ModelRangeRequestInbox {
    inner: Arc<Mutex<CoalescingState>>,
    waker: Arc<AtomicWaker>,
}

struct CoalescingState {
    order: VecDeque<ModelId>,
    pending: HashMap<ModelId, Vec<ClientModelRangeRequest>>,
    closed: bool,
}

fn request_end(request: &ClientModelRangeRequest) -> Option<u64> {
    request.start_index.checked_add(request.count)
}

fn bounding_range(
    left: &ClientModelRangeRequest,
    right: &ClientModelRangeRequest,
) -> Option<ClientModelRangeRequest> {
    let left_end = request_end(left)?;
    let right_end = request_end(right)?;
    let start = left.start_index.min(right.start_index);
    let end = left_end.max(right_end);
    let count = end.checked_sub(start)?;
    if count == 0 || count > MAX_MERGED_COUNT {
        return None;
    }
    Some(ClientModelRangeRequest {
        node_id: right.node_id,
        model_id: right.model_id,
        start_index: start,
        count,
        observed_revision: left.observed_revision.max(right.observed_revision),
    })
}

fn ranges_are_close(left: &ClientModelRangeRequest, right: &ClientModelRangeRequest) -> bool {
    let Some(left_end) = request_end(left) else {
        return false;
    };
    let Some(right_end) = request_end(right) else {
        return false;
    };
    let start = left.start_index.min(right.start_index);
    let end = left_end.max(right_end);
    end.saturating_sub(start) <= COALESCE_SPAN
}

fn integrate_pending(list: &mut Vec<ClientModelRangeRequest>, incoming: ClientModelRangeRequest) {
    let mut merged = incoming;
    let mut absorbed = false;
    list.retain(|existing| {
        if existing.node_id != merged.node_id || existing.model_id != merged.model_id {
            return true;
        }
        if ranges_are_close(existing, &merged) {
            if let Some(bound) = bounding_range(existing, &merged) {
                merged = bound;
                absorbed = true;
                return false;
            }
        }
        true
    });
    if !absorbed && !list.is_empty() {
        // Far jump: the new window replaces older pending holes for this model.
        list.clear();
    }
    list.push(merged);
    if list.len() > MAX_PENDING_RANGES_PER_MODEL {
        let drop = list.len() - MAX_PENDING_RANGES_PER_MODEL;
        list.drain(..drop);
    }
}

impl ModelRangeRequestInbox {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(CoalescingState {
                order: VecDeque::new(),
                pending: HashMap::new(),
                closed: false,
            })),
            waker: Arc::new(AtomicWaker::new()),
        }
    }

    /// Queues `request`, merging nearby windows and replacing far jumps. Never blocks the read loop.
    pub fn submit(&self, request: ClientModelRangeRequest) {
        let model_id = ModelId::new(request.model_id);
        let mut guard = lock_or_recover(&self.inner);
        if guard.closed {
            return;
        }
        let list = guard.pending.entry(model_id).or_default();
        let was_empty = list.is_empty();
        integrate_pending(list, request);
        if was_empty && !list.is_empty() {
            guard.order.push_back(model_id);
        }
        drop(guard);
        self.waker.wake();
    }

    pub fn close(&self) {
        lock_or_recover(&self.inner).closed = true;
        self.waker.wake();
    }

    fn try_pop(&self) -> Option<ClientModelRangeRequest> {
        let mut guard = lock_or_recover(&self.inner);
        while let Some(model_id) = guard.order.pop_front() {
            let remaining = {
                let Some(list) = guard.pending.get_mut(&model_id) else {
                    continue;
                };
                if list.is_empty() {
                    guard.pending.remove(&model_id);
                    continue;
                }
                let request = list.remove(0);
                let remaining = list.len();
                if remaining == 0 {
                    guard.pending.remove(&model_id);
                }
                Some((request, remaining))
            };
            if let Some((request, remaining)) = remaining {
                if remaining > 0 {
                    guard.order.push_back(model_id);
                }
                return Some(request);
            }
        }
        None
    }
}

impl Default for ModelRangeRequestInbox {
    fn default() -> Self {
        Self::new()
    }
}

struct InboxRecv<'a> {
    inbox: &'a ModelRangeRequestInbox,
}

impl Future for InboxRecv<'_> {
    type Output = Option<ClientModelRangeRequest>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(request) = self.inbox.try_pop() {
            return Poll::Ready(Some(request));
        }
        if lock_or_recover(&self.inbox.inner).closed {
            return Poll::Ready(None);
        }
        self.inbox.waker.register(cx.waker());
        if let Some(request) = self.inbox.try_pop() {
            return Poll::Ready(Some(request));
        }
        if lock_or_recover(&self.inbox.inner).closed {
            return Poll::Ready(None);
        }
        Poll::Pending
    }
}

/// Drains [`ModelRangeRequestInbox`] until `shutdown` or `session_cancel` fires.
pub async fn run_model_range_worker(
    session: Arc<Session>,
    inbox: ModelRangeRequestInbox,
    shutdown: CancellationToken,
    session_cancel: CancellationToken,
) {
    loop {
        let next = InboxRecv { inbox: &inbox };
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => break,
            _ = session_cancel.cancelled() => break,
            request = next => {
                let Some(request) = request else { break };
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => break,
                    _ = session_cancel.cancelled() => break,
                    result = session.fulfill_model_range_request(request) => {
                        if let Err(error) = result {
                            tracing::debug!(%error, "dropping invalid collection range request");
                        }
                    }
                }
            }
        }
    }
    inbox.close();
}

#[cfg(test)]
mod tests {
    use super::*;
    use srui_sdk::{NodeId, Surface, Table, Tree};
    use srui_semantic_tree::{ItemId, StoreError, Value};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn table_and_model(item_count: u64) -> (Session, NodeId, ModelId) {
        let session = Session::new("range-unit");
        let node_id = NodeId::new(2);
        let model_id = ModelId::new(7);
        session
            .transaction(|ui| {
                Surface::builder(NodeId::new(1))
                    .label("surface")
                    .create(ui)?;
                ui.apply_op(&Operation::create_model(
                    model_id,
                    TypeRef::TABLE,
                    item_count,
                ))?;
                Table::builder(node_id)
                    .parent(NodeId::new(1))
                    .model_ref(model_id)
                    .create(ui)?;
                Ok(())
            })
            .expect("create table model");
        (session, node_id, model_id)
    }

    fn item_at(index: u64) -> ModelItem {
        ModelItem::with_value(ItemId::new(index + 1), format!("row-{index}"))
    }

    fn counting_provider(calls: Arc<AtomicUsize>) -> ModelRangeProvider {
        Arc::new(move |query: ModelRangeQuery| {
            let calls = Arc::clone(&calls);
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok((0..query.count)
                    .map(|offset| item_at(query.start_index + offset))
                    .collect())
            })
        })
    }

    fn request(
        node_id: NodeId,
        model_id: ModelId,
        start: u64,
        count: u64,
        revision: u64,
    ) -> ClientModelRangeRequest {
        ClientModelRangeRequest {
            node_id: node_id.get(),
            model_id: model_id.get(),
            start_index: start,
            count,
            observed_revision: revision,
        }
    }

    #[tokio::test]
    async fn valid_request_invokes_provider_once_and_commits_reset_range() {
        let (session, node_id, model_id) = table_and_model(1_000);
        let calls = Arc::new(AtomicUsize::new(0));
        session.register_model_range_provider(model_id, counting_provider(Arc::clone(&calls)));
        let revision = session.current_revision();
        let outcome = session
            .fulfill_model_range_request(request(node_id, model_id, 10, 2, revision))
            .await
            .expect("fulfill");
        assert_eq!(
            outcome,
            ModelRangeFulfillment::Committed {
                new_revision: revision + 1
            }
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        session.with_store(|store| {
            let model = store.get_model(model_id).expect("model");
            assert_eq!(
                model.get_item_by_index(10).map(|item| item.value.clone()),
                Some(Value::String("row-10".into()))
            );
            assert_eq!(
                model.get_item_by_index(11).map(|item| item.item_id),
                Some(ItemId::new(12))
            );
        });
        let replayed = session
            .collect_replayed_transactions(revision)
            .expect("replay");
        assert_eq!(replayed.len(), 1);
        assert!(replayed[0].operations.iter().any(|op| {
            matches!(
                op.op,
                Some(srui_protocol::operation::Op::ModelResetRange(_))
            )
        }));
    }

    #[tokio::test]
    async fn validation_failures_never_invoke_the_provider() {
        let (session, node_id, model_id) = table_and_model(100);
        let calls = Arc::new(AtomicUsize::new(0));
        session.register_model_range_provider(model_id, counting_provider(Arc::clone(&calls)));
        let revision = session.current_revision();

        let cases = [
            request(NodeId::new(0), model_id, 0, 1, revision),
            request(node_id, ModelId::new(0), 0, 1, revision),
            request(node_id, model_id, 0, 0, revision),
            ClientModelRangeRequest {
                node_id: node_id.get(),
                model_id: model_id.get(),
                start_index: u64::MAX,
                count: 1,
                observed_revision: revision,
            },
            request(node_id, model_id, 99, 2, revision),
            request(node_id, model_id, 0, 10_001, revision),
            request(node_id, model_id, 0, 1, revision + 1),
            request(NodeId::new(99), model_id, 0, 1, revision),
            request(node_id, ModelId::new(99), 0, 1, revision),
        ];
        for (index, case) in cases.into_iter().enumerate() {
            assert!(
                session.fulfill_model_range_request(case).await.is_err(),
                "case {index} should fail validation"
            );
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn mismatched_node_type_or_model_ref_never_invokes_provider() {
        let session = Session::new("range-mismatch");
        let text_id = NodeId::new(2);
        let model_id = ModelId::new(7);
        session
            .transaction(|ui| {
                Surface::builder(NodeId::new(1)).create(ui)?;
                ui.apply_op(&Operation::create_model(model_id, TypeRef::TABLE, 10))?;
                srui_sdk::Text::builder(text_id)
                    .parent(NodeId::new(1))
                    .text("nope")
                    .create(ui)?;
                Ok(())
            })
            .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        session.register_model_range_provider(model_id, counting_provider(Arc::clone(&calls)));
        let revision = session.current_revision();
        let err = session
            .fulfill_model_range_request(request(text_id, model_id, 0, 1, revision))
            .await
            .expect_err("text is not a collection");
        assert!(matches!(err, ModelRangeError::NotACollectionNode(_)));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn already_cached_range_skips_the_provider() {
        let (session, node_id, model_id) = table_and_model(50);
        let calls = Arc::new(AtomicUsize::new(0));
        session.register_model_range_provider(model_id, counting_provider(Arc::clone(&calls)));
        let revision = session.current_revision();
        session
            .fulfill_model_range_request(request(node_id, model_id, 10, 4, revision))
            .await
            .unwrap();
        let after = session.current_revision();
        let outcome = session
            .fulfill_model_range_request(request(node_id, model_id, 10, 4, after))
            .await
            .unwrap();
        assert_eq!(outcome, ModelRangeFulfillment::AlreadyCached);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn behind_observed_revision_still_fulfills() {
        let (session, node_id, model_id) = table_and_model(50);
        let calls = Arc::new(AtomicUsize::new(0));
        session.register_model_range_provider(model_id, counting_provider(Arc::clone(&calls)));
        session
            .transaction(|ui| {
                ui.set(NodeId::new(1), srui_sdk::LABEL, "moved")?;
                Ok(())
            })
            .unwrap();
        let behind = session.current_revision().saturating_sub(1);
        let outcome = session
            .fulfill_model_range_request(request(node_id, model_id, 0, 4, behind))
            .await
            .unwrap();
        assert!(matches!(outcome, ModelRangeFulfillment::Committed { .. }));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn intervening_scalar_commit_does_not_discard_provider_result() {
        let (session, node_id, model_id) = table_and_model(20);
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let release_rx = Arc::new(tokio::sync::Mutex::new(Some(release_rx)));
        let entered = Arc::new(AtomicUsize::new(0));
        session.register_model_range_provider(
            model_id,
            Arc::new({
                let entered = Arc::clone(&entered);
                move |query: ModelRangeQuery| {
                    let release_rx = Arc::clone(&release_rx);
                    let entered = Arc::clone(&entered);
                    Box::pin(async move {
                        entered.fetch_add(1, Ordering::SeqCst);
                        if let Some(rx) = release_rx.lock().await.take() {
                            let _ = rx.await;
                        }
                        Ok((0..query.count)
                            .map(|offset| item_at(query.start_index + offset))
                            .collect())
                    })
                }
            }),
        );
        let revision = session.current_revision();
        let fulfill = tokio::spawn({
            let session = session.clone();
            async move {
                session
                    .fulfill_model_range_request(request(node_id, model_id, 0, 2, revision))
                    .await
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while entered.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("provider entered");
        session
            .transaction(|ui| {
                ui.set(NodeId::new(1), srui_sdk::LABEL, "moved")?;
                Ok(())
            })
            .unwrap();
        release_tx.send(()).unwrap();
        let outcome = fulfill.await.unwrap().unwrap();
        assert!(matches!(outcome, ModelRangeFulfillment::Committed { .. }));
        session.with_store(|store| {
            let model = store.get_model(model_id).unwrap();
            assert!(model.get_item_by_index(0).is_some());
        });
    }

    #[tokio::test]
    async fn intervening_bounds_change_discards_provider_result() {
        let (session, node_id, model_id) = table_and_model(20);
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let release_rx = Arc::new(tokio::sync::Mutex::new(Some(release_rx)));
        let entered = Arc::new(AtomicUsize::new(0));
        session.register_model_range_provider(
            model_id,
            Arc::new({
                let entered = Arc::clone(&entered);
                move |query: ModelRangeQuery| {
                    let release_rx = Arc::clone(&release_rx);
                    let entered = Arc::clone(&entered);
                    Box::pin(async move {
                        entered.fetch_add(1, Ordering::SeqCst);
                        if let Some(rx) = release_rx.lock().await.take() {
                            let _ = rx.await;
                        }
                        Ok((0..query.count)
                            .map(|offset| item_at(query.start_index + offset))
                            .collect())
                    })
                }
            }),
        );
        let revision = session.current_revision();
        let fulfill = tokio::spawn({
            let session = session.clone();
            async move {
                session
                    .fulfill_model_range_request(request(node_id, model_id, 10, 2, revision))
                    .await
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while entered.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("provider entered");
        session
            .transaction(|ui| {
                ui.apply_op(&Operation::model_reset_range(
                    model_id,
                    0,
                    vec![item_at(0)],
                    Some(1),
                ))?;
                Ok(())
            })
            .unwrap();
        release_tx.send(()).unwrap();
        let outcome = fulfill.await.unwrap().unwrap();
        assert_eq!(outcome, ModelRangeFulfillment::Stale);
        session.with_store(|store| {
            let model = store.get_model(model_id).unwrap();
            assert!(model.get_item_by_index(10).is_none());
        });
    }

    #[tokio::test]
    async fn push_visible_uses_the_same_fulfillment_path() {
        let (session, node_id, model_id) = table_and_model(64);
        let calls = Arc::new(AtomicUsize::new(0));
        session.register_model_range_provider(model_id, counting_provider(Arc::clone(&calls)));
        let outcome = session
            .push_visible_model_range(node_id, model_id, 0, 8)
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            ModelRangeFulfillment::Committed { new_revision: 2 }
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn inbox_merges_nearby_windows_and_replaces_far_jumps() {
        let inbox = ModelRangeRequestInbox::new();
        let model = ModelId::new(3);
        inbox.submit(request(NodeId::new(1), model, 0, 8, 1));
        inbox.submit(request(NodeId::new(1), model, 128, 8, 1));
        {
            let guard = lock_or_recover(&inbox.inner);
            assert_eq!(
                guard.order.len(),
                1,
                "a merge must not enqueue a second drain slot"
            );
        }
        let merged = inbox.try_pop().expect("pending");
        assert_eq!(merged.start_index, 0);
        assert_eq!(merged.count, 136);
        assert!(inbox.try_pop().is_none());

        inbox.submit(request(NodeId::new(1), model, 0, 8, 1));
        inbox.submit(request(NodeId::new(1), model, 10_000, 8, 1));
        let jumped = inbox.try_pop().expect("far jump");
        assert_eq!(jumped.start_index, 10_000);
        assert_eq!(jumped.count, 8);
        assert!(inbox.try_pop().is_none());
    }

    #[tokio::test]
    async fn count_above_operation_limit_is_refused_before_bounds() {
        let (session, node_id, model_id) = table_and_model(20_000);
        let calls = Arc::new(AtomicUsize::new(0));
        session.register_model_range_provider(model_id, counting_provider(Arc::clone(&calls)));
        let revision = session.current_revision();
        let err = session
            .fulfill_model_range_request(request(node_id, model_id, 0, 10_001, revision))
            .await
            .expect_err("count exceeds max_items_per_model_operation");
        assert!(matches!(
            err,
            ModelRangeError::CountExceedsLimit {
                count: 10_001,
                limit: 10_000
            }
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn last_index_and_exact_operation_limit_are_accepted() {
        let (session, node_id, model_id) = table_and_model(20_000);
        let calls = Arc::new(AtomicUsize::new(0));
        session.register_model_range_provider(model_id, counting_provider(Arc::clone(&calls)));
        let revision = session.current_revision();
        let last = session
            .fulfill_model_range_request(request(node_id, model_id, 19_999, 1, revision))
            .await
            .unwrap();
        assert!(matches!(last, ModelRangeFulfillment::Committed { .. }));
        let at_limit = session
            .fulfill_model_range_request(request(
                node_id,
                model_id,
                0,
                10_000,
                session.current_revision(),
            ))
            .await
            .unwrap();
        assert!(matches!(at_limit, ModelRangeFulfillment::Committed { .. }));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn missing_provider_is_a_no_op() {
        let (session, node_id, model_id) = table_and_model(16);
        let revision = session.current_revision();
        let outcome = session
            .fulfill_model_range_request(request(node_id, model_id, 0, 4, revision))
            .await
            .unwrap();
        assert_eq!(outcome, ModelRangeFulfillment::NoProvider);
    }

    #[tokio::test]
    async fn provider_length_mismatch_is_refused() {
        let (session, node_id, model_id) = table_and_model(16);
        session.register_model_range_provider(
            model_id,
            Arc::new(|_| Box::pin(async { Ok(vec![item_at(0)]) })),
        );
        let revision = session.current_revision();
        let err = session
            .fulfill_model_range_request(request(node_id, model_id, 0, 2, revision))
            .await
            .expect_err("provider must return exactly count items");
        assert!(matches!(
            err,
            ModelRangeError::ProviderItemCountMismatch {
                expected: 2,
                actual: 1
            }
        ));
    }

    #[tokio::test]
    async fn provider_error_is_surfaced() {
        let (session, node_id, model_id) = table_and_model(16);
        session.register_model_range_provider(
            model_id,
            Arc::new(|_| Box::pin(async { Err(ModelRangeError::Provider("boom".into())) })),
        );
        let revision = session.current_revision();
        let err = session
            .fulfill_model_range_request(request(node_id, model_id, 0, 2, revision))
            .await
            .expect_err("provider failure");
        assert!(matches!(err, ModelRangeError::Provider(_)));
    }

    #[tokio::test]
    async fn tree_nodes_are_collection_targets() {
        let session = Session::new("range-tree");
        let node_id = NodeId::new(2);
        let model_id = ModelId::new(7);
        session
            .transaction(|ui| {
                Surface::builder(NodeId::new(1)).create(ui)?;
                ui.apply_op(&Operation::create_model(model_id, TypeRef::TREE, 32))?;
                Tree::builder(node_id)
                    .parent(NodeId::new(1))
                    .model_ref(model_id)
                    .create(ui)?;
                Ok(())
            })
            .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        session.register_model_range_provider(model_id, counting_provider(Arc::clone(&calls)));
        let revision = session.current_revision();
        let outcome = session
            .fulfill_model_range_request(request(node_id, model_id, 0, 2, revision))
            .await
            .unwrap();
        assert!(matches!(outcome, ModelRangeFulfillment::Committed { .. }));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn session_error_converts_to_model_range_session() {
        let from_input = ModelRangeError::from(SessionError::InvalidInput("nope".into()));
        assert!(matches!(from_input, ModelRangeError::Session(text) if text.contains("nope")));
        let from_store = ModelRangeError::from(SessionError::Store(StoreError::ModelNotFound(
            ModelId::new(9),
        )));
        assert!(matches!(from_store, ModelRangeError::Session(_)));
    }

    #[test]
    fn session_inner_debug_includes_provider_count() {
        let (session, _, model_id) = table_and_model(4);
        session.register_model_range_provider(
            model_id,
            counting_provider(Arc::new(AtomicUsize::new(0))),
        );
        let guard = lock_or_recover(&session.inner);
        let rendered = format!("{guard:?}");
        assert!(rendered.contains("model_range_provider_count"));
        assert!(rendered.contains("1"));
    }

    #[tokio::test]
    async fn concurrent_fill_of_the_same_range_is_already_cached_after_commit() {
        let (session, node_id, model_id) = table_and_model(20);
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let release_rx = Arc::new(tokio::sync::Mutex::new(Some(release_rx)));
        let entered = Arc::new(AtomicUsize::new(0));
        session.register_model_range_provider(
            model_id,
            Arc::new({
                let entered = Arc::clone(&entered);
                move |query: ModelRangeQuery| {
                    let release_rx = Arc::clone(&release_rx);
                    let entered = Arc::clone(&entered);
                    Box::pin(async move {
                        let n = entered.fetch_add(1, Ordering::SeqCst);
                        if n == 0 {
                            if let Some(rx) = release_rx.lock().await.take() {
                                let _ = rx.await;
                            }
                        }
                        Ok((0..query.count)
                            .map(|offset| item_at(query.start_index + offset))
                            .collect())
                    })
                }
            }),
        );
        let revision = session.current_revision();
        let first = tokio::spawn({
            let session = session.clone();
            async move {
                session
                    .fulfill_model_range_request(request(node_id, model_id, 0, 2, revision))
                    .await
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while entered.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("first provider entered");
        let second = session
            .fulfill_model_range_request(request(node_id, model_id, 0, 2, revision))
            .await
            .unwrap();
        assert!(matches!(second, ModelRangeFulfillment::Committed { .. }));
        release_tx.send(()).unwrap();
        let first = first.await.unwrap().unwrap();
        assert_eq!(first, ModelRangeFulfillment::AlreadyCached);
    }

    #[tokio::test]
    async fn node_type_change_during_provider_is_a_hard_error() {
        let (session, node_id, model_id) = table_and_model(20);
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let release_rx = Arc::new(tokio::sync::Mutex::new(Some(release_rx)));
        let entered = Arc::new(AtomicUsize::new(0));
        session.register_model_range_provider(
            model_id,
            Arc::new({
                let entered = Arc::clone(&entered);
                move |query: ModelRangeQuery| {
                    let release_rx = Arc::clone(&release_rx);
                    let entered = Arc::clone(&entered);
                    Box::pin(async move {
                        entered.fetch_add(1, Ordering::SeqCst);
                        if let Some(rx) = release_rx.lock().await.take() {
                            let _ = rx.await;
                        }
                        Ok((0..query.count)
                            .map(|offset| item_at(query.start_index + offset))
                            .collect())
                    })
                }
            }),
        );
        let revision = session.current_revision();
        let fulfill = tokio::spawn({
            let session = session.clone();
            async move {
                session
                    .fulfill_model_range_request(request(node_id, model_id, 0, 2, revision))
                    .await
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while entered.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("provider entered");
        {
            let mut guard = lock_or_recover(&session.inner);
            guard
                .store
                .get_node_mut(node_id)
                .expect("table node")
                .node_type = TypeRef::TEXT;
        }
        release_tx.send(()).unwrap();
        let err = fulfill
            .await
            .unwrap()
            .expect_err("type change is not stale");
        assert!(matches!(err, ModelRangeError::NotACollectionNode(_)));
    }

    #[test]
    fn inbox_default_submit_after_close_and_overflow_helpers() {
        let inbox = ModelRangeRequestInbox::default();
        inbox.close();
        inbox.submit(request(NodeId::new(1), ModelId::new(3), 0, 8, 1));
        assert!(inbox.try_pop().is_none());

        let zero = request(NodeId::new(1), ModelId::new(3), 40, 0, 1);
        assert!(bounding_range(&zero, &zero).is_none());
        let left = request(NodeId::new(1), ModelId::new(3), 0, 1, 1);
        let right = request(NodeId::new(1), ModelId::new(3), MAX_MERGED_COUNT, 2, 1);
        assert!(bounding_range(&left, &right).is_none());
        let exact = request(NodeId::new(1), ModelId::new(3), MAX_MERGED_COUNT - 1, 1, 1);
        let bound = bounding_range(&left, &exact).expect("span of MAX_MERGED_COUNT is allowed");
        assert_eq!(bound.start_index, 0);
        assert_eq!(bound.count, MAX_MERGED_COUNT);

        let overflow = ClientModelRangeRequest {
            node_id: 1,
            model_id: 3,
            start_index: u64::MAX,
            count: 1,
            observed_revision: 1,
        };
        assert!(!ranges_are_close(&overflow, &overflow));
        assert!(!ranges_are_close(
            &request(NodeId::new(1), ModelId::new(3), 0, 1, 1),
            &overflow
        ));

        let model = ModelId::new(3);
        let mut packed: Vec<_> = (0..10)
            .map(|i| request(NodeId::new(i + 1), model, i * 300, 8, 1))
            .collect();
        integrate_pending(&mut packed, request(NodeId::new(1), model, 0, 8, 1));
        assert_eq!(packed.len(), MAX_PENDING_RANGES_PER_MODEL);
    }

    #[test]
    fn try_pop_skips_stale_order_and_requeues_remaining() {
        let inbox = ModelRangeRequestInbox::new();
        let stale = ModelId::new(99);
        let live = ModelId::new(3);
        {
            let mut guard = lock_or_recover(&inbox.inner);
            guard.order.push_back(stale);
            guard.order.push_back(live);
            guard.pending.insert(live, Vec::new());
            guard.order.push_back(ModelId::new(4));
            guard.pending.insert(
                ModelId::new(4),
                vec![
                    request(NodeId::new(1), ModelId::new(4), 0, 2, 1),
                    request(NodeId::new(1), ModelId::new(4), 10, 2, 1),
                ],
            );
        }
        let first = inbox.try_pop().expect("live after stale/empty");
        assert_eq!(first.model_id, 4);
        assert_eq!(first.start_index, 0);
        let second = inbox.try_pop().expect("requeued remainder");
        assert_eq!(second.start_index, 10);
        assert!(inbox.try_pop().is_none());
        {
            let guard = lock_or_recover(&inbox.inner);
            assert!(
                guard.order.is_empty(),
                "popping the last remainder must not requeue an empty drain slot"
            );
        }
    }

    #[tokio::test]
    #[allow(clippy::redundant_async_block)]
    async fn inbox_recv_observes_close_and_late_submit() {
        let closed = ModelRangeRequestInbox::new();
        closed.close();
        assert!(InboxRecv { inbox: &closed }.await.is_none());

        let inbox = ModelRangeRequestInbox::new();
        let pending = inbox.clone();
        let waiter = tokio::spawn(async move { InboxRecv { inbox: &pending }.await });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        inbox.submit(request(NodeId::new(1), ModelId::new(3), 0, 4, 1));
        let got = waiter.await.unwrap().expect("submitted after park");
        assert_eq!(got.count, 4);

        let closing = ModelRangeRequestInbox::new();
        let pending = closing.clone();
        let waiter = tokio::spawn(async move { InboxRecv { inbox: &pending }.await });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        closing.close();
        assert!(waiter.await.unwrap().is_none());
    }

    #[tokio::test]
    async fn worker_exits_on_shutdown_and_closes_the_inbox() {
        let (session, node_id, model_id) = table_and_model(8);
        let session = Arc::new(session);
        let inbox = ModelRangeRequestInbox::new();
        let shutdown = CancellationToken::new();
        let session_cancel = CancellationToken::new();
        let worker = tokio::spawn(run_model_range_worker(
            Arc::clone(&session),
            inbox.clone(),
            shutdown.clone(),
            session_cancel,
        ));
        tokio::task::yield_now().await;
        shutdown.cancel();
        worker.await.unwrap();
        inbox.submit(request(node_id, model_id, 0, 1, session.current_revision()));
        assert!(inbox.try_pop().is_none());
    }
}
