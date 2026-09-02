//! # Bounded Outbound Transaction Queues & Coalescing (§20.2)
//!
//! Provides [`OutboundHub`], [`OutboundQueue`], and [`OutboundReceiver`] for bounded,
//! per-connection transaction streaming with scalar property coalescing and lossless
//! structural barriers.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

use srui_protocol::{
    operation::Op, value::Value as WireValInner, Operation as WireOp, Transaction,
    Value as WireValue,
};

use crate::session::SessionError;

/// Default capacity for per-connection outbound transaction queues (§20.2).
pub const DEFAULT_OUTBOUND_QUEUE_CAPACITY: usize = 128;

/// Diagnostic queue metrics for tracking buffer depth and peak pressure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutboundMetrics {
    /// Current count of pending transactions in the client queue.
    pub current_depth: usize,
    /// Maximum count of pending transactions observed in this queue.
    pub peak_depth: usize,
    /// Configured maximum queue capacity.
    pub capacity: usize,
}

/// Errors occurring when receiving transactions from an [`OutboundReceiver`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboundRecvError {
    /// The client queue overflowed its configured capacity; connection must detach for resync.
    Lagged(String),
    /// The outbound transaction queue or session has closed cleanly.
    Closed,
}

impl std::fmt::Display for OutboundRecvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lagged(reason) => write!(f, "outbound queue overflow: {reason}"),
            Self::Closed => write!(f, "outbound queue closed"),
        }
    }
}

impl std::error::Error for OutboundRecvError {}

/// Non-blocking receive errors for [`OutboundReceiver::try_recv`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboundTryRecvError {
    /// No transaction is currently queued.
    Empty,
    /// The client queue overflowed its configured capacity; connection must detach for resync.
    Lagged(String),
    /// The outbound transaction queue or session has closed cleanly.
    Closed,
}

impl std::fmt::Display for OutboundTryRecvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "queue is empty"),
            Self::Lagged(reason) => write!(f, "outbound queue overflow: {reason}"),
            Self::Closed => write!(f, "outbound queue closed"),
        }
    }
}

impl std::error::Error for OutboundTryRecvError {}

/// Result of an enqueue operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueOutcome {
    /// Enqueued as a new discrete transaction in the queue.
    Enqueued,
    /// Coalesced into the existing tail transaction without increasing queue length.
    Coalesced,
}

/// Error returned when an unmergeable transaction cannot fit into a full queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnqueueError {
    /// The client queue reached capacity; queue backlog was discarded and marked stale.
    Overflow {
        /// Client instance identity to mark for Task 23 snapshot resync.
        client_instance_id: Vec<u8>,
    },
    /// The queue has already been closed.
    Closed,
}

/// Returns `true` if the wire value is scalar (§7.6).
///
/// Composite types (`ListValue` and `RecordValue`) are non-scalar and serve as barriers.
fn is_scalar_value(val: &WireValue) -> bool {
    !matches!(
        &val.value,
        Some(WireValInner::ListValue(_)) | Some(WireValInner::RecordValue(_)) | None
    )
}

/// Returns `true` if this operation is a scalar `Operation::SetProperty`.
fn is_scalar_set_property(op: &WireOp) -> bool {
    match &op.op {
        Some(Op::SetProperty(sp)) => sp.value.as_ref().is_some_and(is_scalar_value),
        _ => false,
    }
}

/// Returns `true` if this transaction is composed entirely of scalar `SetProperty` operations.
///
/// Mixed, structural, model, clear, batch, and non-scalar transactions return `false`
/// and act as FIFO barriers.
pub fn is_coalesceable_tx(tx: &Transaction) -> bool {
    !tx.operations.is_empty() && tx.operations.iter().all(is_scalar_set_property)
}

type PropKey = (u64, (u32, u32));

fn get_prop_key(op: &WireOp) -> Option<PropKey> {
    match &op.op {
        Some(Op::SetProperty(sp)) => {
            let prop = sp.property.as_ref()?;
            Some((sp.node_id, (prop.namespace_id, prop.local_id)))
        }
        _ => None,
    }
}

/// Attempts to coalesce `incoming` into `tail` if contiguous, same priority, and within `max_ops`.
fn try_coalesce(tail: &mut Transaction, incoming: &Transaction, max_ops: usize) -> bool {
    let mut key_map: HashMap<PropKey, usize> = HashMap::with_capacity(tail.operations.len());
    for (idx, op) in tail.operations.iter().enumerate() {
        if let Some(key) = get_prop_key(op) {
            key_map.insert(key, idx);
        }
    }

    let mut new_keys_count = 0;
    for op in &incoming.operations {
        if let Some(key) = get_prop_key(op) {
            key_map.entry(key).or_insert_with(|| {
                new_keys_count += 1;
                usize::MAX
            });
        }
    }

    if tail.operations.len() + new_keys_count > max_ops {
        return false;
    }

    key_map.clear();
    for (idx, op) in tail.operations.iter().enumerate() {
        if let Some(key) = get_prop_key(op) {
            key_map.insert(key, idx);
        }
    }

    for op in &incoming.operations {
        if let Some(key) = get_prop_key(op) {
            if let Some(&idx) = key_map.get(&key) {
                tail.operations[idx] = op.clone();
            } else {
                let new_idx = tail.operations.len();
                tail.operations.push(op.clone());
                key_map.insert(key, new_idx);
            }
        }
    }

    tail.new_revision = incoming.new_revision;
    true
}

#[derive(Debug)]
struct QueueInner {
    client_instance_id: Vec<u8>,
    capacity: usize,
    max_ops: usize,
    items: VecDeque<Transaction>,
    current_depth: usize,
    peak_depth: usize,
    is_closed: bool,
    stale_reason: Option<String>,
}

/// A bounded, per-connection queue for outbound transaction deltas.
#[derive(Debug)]
pub struct OutboundQueue {
    inner: Mutex<QueueInner>,
    notify: Arc<Notify>,
}

impl OutboundQueue {
    /// Creates a new `OutboundQueue` for a client connection.
    pub fn new(client_instance_id: Vec<u8>, capacity: usize, max_ops: usize) -> Self {
        assert!(capacity > 0, "outbound queue capacity must be positive");
        assert!(max_ops > 0, "max_transaction_operations must be positive");

        Self {
            inner: Mutex::new(QueueInner {
                client_instance_id,
                capacity,
                max_ops,
                items: VecDeque::with_capacity(capacity),
                current_depth: 0,
                peak_depth: 0,
                is_closed: false,
                stale_reason: None,
            }),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Enqueues a committed transaction synchronously.
    ///
    /// If `tx` is contiguous, same-priority, and composed entirely of scalar `SetProperty`
    /// operations, it coalesces into the tail item. Otherwise, it is pushed as a distinct item.
    /// When capacity is exceeded, the queue backlog is purged, closed with a stale marker,
    /// and wakes blocked receivers immediately.
    pub fn enqueue(&self, tx: &Transaction) -> Result<EnqueueOutcome, EnqueueError> {
        let mut guard = self.inner.lock().unwrap();
        if guard.is_closed {
            return Err(EnqueueError::Closed);
        }

        let max_ops = guard.max_ops;
        if let Some(tail) = guard.items.back_mut() {
            if tail.priority == tx.priority
                && tail.new_revision == tx.base_revision
                && is_coalesceable_tx(tail)
                && is_coalesceable_tx(tx)
                && try_coalesce(tail, tx, max_ops)
            {
                self.notify.notify_one();
                return Ok(EnqueueOutcome::Coalesced);
            }
        }

        if guard.items.len() < guard.capacity {
            guard.items.push_back(tx.clone());
            guard.current_depth = guard.items.len();
            guard.peak_depth = guard.peak_depth.max(guard.current_depth);
            self.notify.notify_one();
            Ok(EnqueueOutcome::Enqueued)
        } else {
            let reason = format!(
                "outbound queue capacity ({}) exceeded; resync required",
                guard.capacity
            );
            guard.is_closed = true;
            guard.stale_reason = Some(reason);
            guard.items.clear();
            guard.current_depth = 0;
            self.notify.notify_waiters();
            Err(EnqueueError::Overflow {
                client_instance_id: guard.client_instance_id.clone(),
            })
        }
    }

    /// Closes the queue cleanly without dropping already queued transactions.
    pub fn close(&self) {
        let mut guard = self.inner.lock().unwrap();
        guard.is_closed = true;
        self.notify.notify_waiters();
    }

    /// Returns `true` if this queue has overflowed and is marked stale.
    pub fn is_stale(&self) -> bool {
        let guard = self.inner.lock().unwrap();
        guard.stale_reason.is_some()
    }

    /// Returns `true` if this queue has been closed.
    pub fn is_closed(&self) -> bool {
        let guard = self.inner.lock().unwrap();
        guard.is_closed
    }

    /// Current queue depth.
    pub fn current_depth(&self) -> usize {
        let guard = self.inner.lock().unwrap();
        guard.current_depth
    }

    /// Peak queue depth observed over the lifetime of this queue.
    pub fn peak_depth(&self) -> usize {
        let guard = self.inner.lock().unwrap();
        guard.peak_depth
    }

    /// Configured queue capacity.
    pub fn capacity(&self) -> usize {
        let guard = self.inner.lock().unwrap();
        guard.capacity
    }

    /// Returns queue diagnostics.
    pub fn metrics(&self) -> OutboundMetrics {
        let guard = self.inner.lock().unwrap();
        OutboundMetrics {
            current_depth: guard.current_depth,
            peak_depth: guard.peak_depth,
            capacity: guard.capacity,
        }
    }
}

/// The consumer handle for an outbound transaction queue.
#[derive(Debug)]
pub struct OutboundReceiver {
    subscription_id: u64,
    queue: Arc<OutboundQueue>,
    hub: Option<Arc<OutboundHub>>,
}

impl OutboundReceiver {
    /// Creates an unlinked receiver for standalone unit tests.
    #[must_use]
    pub fn new_unlinked(queue: Arc<OutboundQueue>) -> Self {
        Self {
            subscription_id: 0,
            queue,
            hub: None,
        }
    }

    /// Asynchronously waits for and removes the next transaction from the queue.
    ///
    /// Conforms to `async-cancel-safety`: if cancelled at an `.await` point, no item is lost.
    pub async fn recv(&mut self) -> Result<Transaction, OutboundRecvError> {
        loop {
            let notified = self.queue.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            {
                let mut guard = self.queue.inner.lock().unwrap();
                if let Some(reason) = &guard.stale_reason {
                    return Err(OutboundRecvError::Lagged(reason.clone()));
                }
                if let Some(tx) = guard.items.pop_front() {
                    guard.current_depth = guard.items.len();
                    return Ok(tx);
                }
                if guard.is_closed {
                    return Err(OutboundRecvError::Closed);
                }
            }

            notified.await;
        }
    }

    /// Non-blocking check for the next transaction in the queue.
    pub fn try_recv(&mut self) -> Result<Transaction, OutboundTryRecvError> {
        let mut guard = self.queue.inner.lock().unwrap();
        if let Some(reason) = &guard.stale_reason {
            return Err(OutboundTryRecvError::Lagged(reason.clone()));
        }
        if let Some(tx) = guard.items.pop_front() {
            guard.current_depth = guard.items.len();
            return Ok(tx);
        }
        if guard.is_closed {
            return Err(OutboundTryRecvError::Closed);
        }
        Err(OutboundTryRecvError::Empty)
    }

    /// Current queue depth.
    pub fn current_depth(&self) -> usize {
        self.queue.current_depth()
    }

    /// Peak queue depth observed over the lifetime of this queue.
    pub fn peak_depth(&self) -> usize {
        self.queue.peak_depth()
    }

    /// Configured queue capacity.
    pub fn capacity(&self) -> usize {
        self.queue.capacity()
    }

    /// Diagnostic metrics for this receiver's queue.
    pub fn metrics(&self) -> OutboundMetrics {
        self.queue.metrics()
    }

    /// Returns `true` if this queue has overflowed and is marked stale.
    pub fn is_stale(&self) -> bool {
        self.queue.is_stale()
    }

    /// Returns `true` if this queue has been closed.
    pub fn is_closed(&self) -> bool {
        self.queue.is_closed()
    }
}

impl Drop for OutboundReceiver {
    fn drop(&mut self) {
        if let Some(hub) = &self.hub {
            hub.unsubscribe(self.subscription_id);
        }
    }
}

#[derive(Debug)]
struct HubInner {
    next_sub_id: u64,
    subscriptions: HashMap<u64, Arc<OutboundQueue>>,
    stale_clients: HashSet<Vec<u8>>,
    is_closed: bool,
}

/// The session-level outbound transaction distribution hub.
///
/// Distributes committed transactions to active client connections while enforcing
/// per-connection queue capacities and tracking stale client identities for Task 23 resync.
#[derive(Debug)]
pub struct OutboundHub {
    inner: Mutex<HubInner>,
}

impl Default for OutboundHub {
    fn default() -> Self {
        Self::new()
    }
}

impl OutboundHub {
    /// Creates a new `OutboundHub`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HubInner {
                next_sub_id: 1,
                subscriptions: HashMap::new(),
                stale_clients: HashSet::new(),
                is_closed: false,
            }),
        }
    }

    /// Subscribes a client connection to the hub, returning an [`OutboundReceiver`].
    pub fn subscribe(
        self: &Arc<Self>,
        client_instance_id: Vec<u8>,
        capacity: usize,
        max_ops: usize,
    ) -> Result<OutboundReceiver, SessionError> {
        let mut guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;
        if guard.is_closed {
            return Err(SessionError::BroadcastClosed);
        }

        let sub_id = guard.next_sub_id;
        guard.next_sub_id = guard.next_sub_id.wrapping_add(1);

        let queue = Arc::new(OutboundQueue::new(client_instance_id, capacity, max_ops));
        guard.subscriptions.insert(sub_id, Arc::clone(&queue));

        Ok(OutboundReceiver {
            subscription_id: sub_id,
            queue,
            hub: Some(Arc::clone(self)),
        })
    }

    /// Publishes a committed transaction synchronously to all attached client queues.
    ///
    /// Never blocks on client I/O. Any client whose queue cannot accommodate the transaction
    /// is closed with a stale marker, its backlog purged, and its `client_instance_id` recorded
    /// for mandatory snapshot resync on reconnect.
    pub fn publish(&self, tx: &Transaction) -> Vec<Vec<u8>> {
        let mut guard = self.inner.lock().unwrap();
        if guard.is_closed {
            return Vec::new();
        }

        let mut overflowed = Vec::new();
        for queue in guard.subscriptions.values() {
            if let Err(EnqueueError::Overflow { client_instance_id }) = queue.enqueue(tx) {
                if !client_instance_id.is_empty() {
                    overflowed.push(client_instance_id);
                }
            }
        }
        for client_id in &overflowed {
            guard.stale_clients.insert(client_id.clone());
        }
        overflowed
    }

    /// Returns `true` if `client_instance_id` was marked stale due to queue overflow.
    pub fn is_client_stale(&self, client_instance_id: &[u8]) -> bool {
        let guard = self.inner.lock().unwrap();
        guard.stale_clients.contains(client_instance_id)
    }

    /// Explicitly marks a client instance as stale (forcing Task 23 resync).
    pub fn mark_client_stale(&self, client_instance_id: Vec<u8>) {
        let mut guard = self.inner.lock().unwrap();
        guard.stale_clients.insert(client_instance_id);
    }

    /// Clears the stale marker for `client_instance_id` after a snapshot is sent.
    pub fn clear_stale_client(&self, client_instance_id: &[u8]) {
        let mut guard = self.inner.lock().unwrap();
        guard.stale_clients.remove(client_instance_id);
    }

    /// Closes all active outbound queues and marks the hub as closed.
    pub fn close(&self) {
        let mut guard = self.inner.lock().unwrap();
        guard.is_closed = true;
        for queue in guard.subscriptions.values() {
            queue.close();
        }
        guard.subscriptions.clear();
    }

    /// Returns the peak depth observed for a client instance queue, if currently attached.
    pub fn peak_depth_for_client(&self, client_instance_id: &[u8]) -> Option<usize> {
        let guard = self.inner.lock().unwrap();
        for q in guard.subscriptions.values() {
            let inner = q.inner.lock().unwrap();
            if inner.client_instance_id.as_slice() == client_instance_id {
                return Some(inner.peak_depth);
            }
        }
        None
    }

    /// Returns the maximum peak depth observed across all active client queues.
    pub fn max_peak_depth(&self) -> usize {
        let guard = self.inner.lock().unwrap();
        guard
            .subscriptions
            .values()
            .map(|q| q.peak_depth())
            .max()
            .unwrap_or(0)
    }

    /// Returns `true` if the hub is closed.
    pub fn is_closed(&self) -> bool {
        let guard = self.inner.lock().unwrap();
        guard.is_closed
    }

    /// Returns the number of currently active subscriptions.
    pub fn active_subscriptions(&self) -> usize {
        let guard = self.inner.lock().unwrap();
        guard.subscriptions.len()
    }

    pub(crate) fn unsubscribe(&self, sub_id: u64) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.subscriptions.remove(&sub_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use srui_protocol::{CreateNodeOp, NodeRecord, PropertyRef as WirePropRef, SetPropertyOp};

    fn make_set_prop_op(node_id: u64, prop_id: u32, val: &str) -> WireOp {
        WireOp {
            op: Some(Op::SetProperty(SetPropertyOp {
                node_id,
                property: Some(WirePropRef {
                    namespace_id: 0,
                    local_id: prop_id,
                }),
                value: Some(WireValue {
                    value: Some(WireValInner::StringValue(val.to_string())),
                }),
            })),
        }
    }

    fn make_scalar_tx(
        base: u64,
        new_rev: u64,
        node_id: u64,
        prop_id: u32,
        val: &str,
    ) -> Transaction {
        Transaction {
            base_revision: base,
            new_revision: new_rev,
            priority: 1,
            operations: vec![make_set_prop_op(node_id, prop_id, val)],
        }
    }

    fn make_create_node_tx(base: u64, new_rev: u64, node_id: u64) -> Transaction {
        Transaction {
            base_revision: base,
            new_revision: new_rev,
            priority: 1,
            operations: vec![WireOp {
                op: Some(Op::CreateNode(CreateNodeOp {
                    node: Some(NodeRecord {
                        node_id,
                        r#type: None,
                        parent_id: 0,
                        child_index: 0,
                        properties: vec![],
                    }),
                })),
            }],
        }
    }

    #[test]
    fn test_scalar_coalescing_retains_latest_value_and_revision_span() {
        let queue = OutboundQueue::new(vec![1], 4, 10);

        let tx1 = make_scalar_tx(0, 1, 10, 1, "v1");
        assert_eq!(queue.enqueue(&tx1), Ok(EnqueueOutcome::Enqueued));
        assert_eq!(queue.current_depth(), 1);

        let tx2 = make_scalar_tx(1, 2, 10, 1, "v2");
        assert_eq!(queue.enqueue(&tx2), Ok(EnqueueOutcome::Coalesced));
        assert_eq!(queue.current_depth(), 1);

        let tx3 = make_scalar_tx(2, 5, 10, 1, "v5");
        assert_eq!(queue.enqueue(&tx3), Ok(EnqueueOutcome::Coalesced));
        assert_eq!(queue.current_depth(), 1);
        assert_eq!(queue.peak_depth(), 1);

        let mut rx = OutboundReceiver::new_unlinked(Arc::new(queue));
        let merged = rx.try_recv().expect("merged tx");
        assert_eq!(merged.base_revision, 0);
        assert_eq!(merged.new_revision, 5);
        assert_eq!(merged.operations.len(), 1);
        match &merged.operations[0].op {
            Some(Op::SetProperty(sp)) => {
                assert_eq!(
                    sp.value.as_ref().unwrap().value,
                    Some(WireValInner::StringValue("v5".to_string()))
                );
            }
            other => panic!("expected SetProperty, got {:?}", other),
        }
    }

    #[test]
    fn test_structural_transaction_acts_as_barrier() {
        let queue = OutboundQueue::new(vec![1], 4, 10);

        let tx1 = make_scalar_tx(0, 1, 10, 1, "v1");
        assert_eq!(queue.enqueue(&tx1), Ok(EnqueueOutcome::Enqueued));

        let tx_barrier = make_create_node_tx(1, 2, 20);
        assert_eq!(queue.enqueue(&tx_barrier), Ok(EnqueueOutcome::Enqueued));
        assert_eq!(queue.current_depth(), 2);

        let tx3 = make_scalar_tx(2, 3, 10, 1, "v3");
        assert_eq!(queue.enqueue(&tx3), Ok(EnqueueOutcome::Enqueued));
        assert_eq!(queue.current_depth(), 3);

        let tx4 = make_scalar_tx(3, 4, 10, 1, "v4");
        assert_eq!(queue.enqueue(&tx4), Ok(EnqueueOutcome::Coalesced));
        assert_eq!(queue.current_depth(), 3);
        assert_eq!(queue.peak_depth(), 3);
    }

    #[test]
    fn test_overflow_discards_backlog_and_marks_stale() {
        let queue = OutboundQueue::new(vec![7, 7], 2, 10);

        let b1 = make_create_node_tx(0, 1, 1);
        let b2 = make_create_node_tx(1, 2, 2);
        let b3 = make_create_node_tx(2, 3, 3);

        assert_eq!(queue.enqueue(&b1), Ok(EnqueueOutcome::Enqueued));
        assert_eq!(queue.enqueue(&b2), Ok(EnqueueOutcome::Enqueued));
        assert_eq!(queue.current_depth(), 2);

        let overflow_err = queue.enqueue(&b3);
        assert_eq!(
            overflow_err,
            Err(EnqueueError::Overflow {
                client_instance_id: vec![7, 7]
            })
        );
        assert!(queue.is_stale());
        assert!(queue.is_closed());
        assert_eq!(queue.current_depth(), 0);
        assert_eq!(queue.peak_depth(), 2);

        let mut rx = OutboundReceiver::new_unlinked(Arc::new(queue));
        assert!(matches!(
            rx.try_recv(),
            Err(OutboundTryRecvError::Lagged(_))
        ));
    }
}
